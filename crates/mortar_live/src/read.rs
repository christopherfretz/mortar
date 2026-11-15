//! Reading what the mark reads (`spec/image.md` I-7): a section's header,
//! a relocation section's records, and a file's symbol table.
//!
//! The discipline is `mortar_scan`'s (`spec/input.md` §11.1, §11.3): the
//! store keeps the relocations of allocated sections and of `.eh_frame`
//! resident, so the records are borrowed; when they are not, they are
//! copied with one `Inputs::copy_to` into the caller's buffer. A read that
//! fails is a diagnostic, never "no records" (a section without edges
//! would be collected and its referents with it).

use mortar_core::FileId;
use mortar_core::inputs::{CopyError, Inputs};
use mortar_diag::{Diagnostic, Kind};
use mortar_elf::consts::{SHT_REL, SHT_RELA};
use mortar_elf::raw::{Rel, Rela, Table};
use mortar_elf::{Object, SectionIndex};

/// One record: the explicit addend of `SHT_RELA`, none for `SHT_REL`
/// (the implicit addend is in the relocated bytes, which only the
/// architecture reads: [`implicit`], when a piece selection needs it).
#[derive(Debug, Clone, Copy)]
pub(crate) struct Raw {
    pub r_offset: u64,
    pub r_sym: u32,
    pub r_type: u32,
    pub addend: Option<i64>,
}

/// A relocation section's records, either form.
#[derive(Debug, Clone, Copy)]
pub(crate) enum Records<'b> {
    Rela(Table<'b, Rela>),
    Rel(Table<'b, Rel>),
}

impl Records<'_> {
    pub fn len(&self) -> usize {
        match self {
            Self::Rela(t) => t.len(),
            Self::Rel(t) => t.len(),
        }
    }

    pub fn get(&self, i: usize) -> Option<Raw> {
        match self {
            Self::Rela(t) => t.get(i).map(|r| Raw {
                r_offset: r.r_offset,
                r_sym: r.r_sym,
                r_type: r.r_type,
                addend: Some(r.r_addend),
            }),
            Self::Rel(t) => t.get(i).map(|r| Raw {
                r_offset: r.r_offset,
                r_sym: r.r_sym,
                r_type: r.r_type,
                addend: None,
            }),
        }
    }
}

/// Why the records of a section could not be had.
#[derive(Debug, Clone)]
pub(crate) enum ReadFail {
    /// `mortar_elf`'s statement: `malformed-input`.
    Malformed(String),
    /// The copy of bytes that are not resident failed.
    Copy(CopyError),
    /// A file without a store entry whose bytes are not resident.
    Internal(String),
}

impl ReadFail {
    /// The diagnostic, before its place (`mortar_scan`'s wording).
    pub fn diagnostic(&self, path: &str) -> Diagnostic {
        match self {
            Self::Malformed(what) => Diagnostic::error(Kind::Malformed { what: what.clone() }),
            Self::Copy(CopyError::Changed) => Diagnostic::error(Kind::InputChanged),
            Self::Copy(CopyError::Read(errno)) => {
                let reason = if errno.0 == 0 {
                    "the read failed".to_owned()
                } else {
                    mortar_diag::os_reason(&std::io::Error::from_raw_os_error(errno.0))
                };
                Diagnostic::error(Kind::CannotOpenFile { path: path.to_owned(), reason })
            }
            Self::Copy(CopyError::OutOfRange) => Diagnostic::error(Kind::InternalError {
                what: format!("liveness: a copy outside {path}"),
            }),
            Self::Internal(what) => Diagnostic::error(Kind::InternalError { what: what.clone() }),
        }
    }
}

/// The number of records of relocation section `rel`, from its header
/// (the unit cut of I-8 needs no bytes).
pub(crate) fn count(obj: &Object<'_>, rel: SectionIndex) -> Result<usize, ReadFail> {
    let s = obj.section(rel).map_err(|e| ReadFail::Malformed(e.to_string()))?;
    let h = s.header();
    if h.sh_entsize == 0 {
        return Err(ReadFail::Malformed(format!("relocation section {rel} has sh_entsize 0")));
    }
    usize::try_from(h.sh_size / h.sh_entsize)
        .map_err(|_| ReadFail::Malformed(format!("relocation section {rel} is too large")))
}

/// The records of relocation section `rel` of `file`: resident, or copied
/// into `buf`.
pub(crate) fn records<'b, 's: 'b>(
    inputs: &Inputs<'s>,
    file: FileId,
    obj: &Object<'s>,
    rel: SectionIndex,
    buf: &'b mut Vec<u8>,
) -> Result<Records<'b>, ReadFail> {
    let elf = |e: mortar_elf::Error| ReadFail::Malformed(e.to_string());
    let s = obj.section(rel).map_err(elf)?;
    let rela = match s.sh_type() {
        SHT_RELA => true,
        SHT_REL => false,
        t => {
            return Err(ReadFail::Malformed(format!(
                "section {rel} applies relocations but has type {t:#x}"
            )));
        }
    };
    let resident = if rela {
        s.rela().map(|r| Records::Rela(r.records()))
    } else {
        s.rel().map(|r| Records::Rel(r.records()))
    };
    match resident {
        Ok(t) => Ok(t),
        Err(mortar_elf::Error::NotResident { offset, size }) => {
            if inputs.files[file].bytes.is_none() {
                return Err(ReadFail::Internal(format!(
                    "liveness: {size} bytes at offset {offset} of file {} are not resident and it has no store entry",
                    file.0
                )));
            }
            let len = usize::try_from(size).map_err(|_| ReadFail::Copy(CopyError::OutOfRange))?;
            buf.clear();
            buf.resize(len, 0);
            let end = offset.checked_add(size).ok_or(ReadFail::Copy(CopyError::OutOfRange))?;
            inputs.copy_to(file, offset..end, buf).map_err(ReadFail::Copy)?;
            let buf: &'b Vec<u8> = buf;
            if rela {
                Ok(Records::Rela(s.rela_in(buf).map_err(elf)?.records()))
            } else {
                Ok(Records::Rel(s.rel_in(buf).map_err(elf)?.records()))
            }
        }
        Err(e) => Err(elf(e)),
    }
}

/// The bytes a REL record's addend is read from: `READ_BACK` before
/// `r_offset` and `READ_FWD` from it, clipped to the section (an
/// architecture's read window, X86-40, is within these).
const READ_BACK: u64 = 8;
const READ_FWD: u64 = 16;

/// The implicit addend of REL record `raw` of section `index` of `file`
/// (x86.md X86-4, X86-5: synth S-17's piece selection of a reference into
/// a split section is by `st_value + A`, I-7): `arch`'s `read_addend`
/// over the field's input bytes, as `classify` reads the type at them.
/// `None` when the type is refused or the field is not in the section
/// (the scan reports both): the caller then marks every piece.
pub(crate) fn implicit(
    arch: &dyn mortar_arch::Arch,
    inputs: &Inputs<'_>,
    file: FileId,
    obj: &Object<'_>,
    index: SectionIndex,
    raw: Raw,
    buf: &mut Vec<u8>,
) -> Option<i64> {
    let s = obj.section(index).ok()?;
    let lo = raw.r_offset.saturating_sub(READ_BACK);
    let hi = raw.r_offset.checked_add(READ_FWD)?.min(s.size());
    if raw.r_offset >= hi {
        return None;
    }
    let bytes: &[u8] = match s.data() {
        Ok(d) => d.get(lo as usize..hi as usize)?,
        Err(mortar_elf::Error::NotResident { offset, .. }) => {
            buf.clear();
            buf.resize((hi - lo) as usize, 0);
            inputs.copy_to(file, offset + lo..offset + hi, buf).ok()?;
            buf
        }
        Err(_) => return None,
    };
    let site = mortar_arch::Site { bytes, offset: raw.r_offset - lo };
    let h = arch.classify(raw.r_type, site).ok()?;
    let field = site.at(usize::from(h.field.width()))?;
    Some(arch.read_addend(&h, field))
}
