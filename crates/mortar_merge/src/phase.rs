//! Phase 60 over the link's tables (`spec/synth.md` §2): which sections
//! are split, their bytes, and the `.eh_frame` relocations classified and
//! resolved, then [`merge()`](crate::merge()) and [`eh::parse`].
//!
//! The architecture enters through [`Relocs`], which [`ArchRelocs`]
//! implements for any `mortar_arch::Arch` (a test gives a table for a
//! target without one).

use std::borrow::Cow;

use mortar_arch::{Arch, DemandClass, Field, Site};
use mortar_core::inputs::{CopyError, Reader, RelocForm};
use mortar_core::layout::EntryDef;
use mortar_core::par::Pool;
use mortar_core::resolution::{Def, SectionKind};
use mortar_core::{Assignment, FileId, Inputs, OutSecId, Resolution, SectionId, Split, SymbolId};
use mortar_diag::{Diagnostic, Key};
use mortar_elf::consts::{SHF_COMPRESSED, SHF_STRINGS, STT_SECTION};
use mortar_elf::raw::{Rel, Rela, Table};
use mortar_elf::{Class, Object, Section, SectionIndex, SymbolIndex, SymbolSection};

use crate::cut::{Whole, align_log2, placed_whole};
use crate::eh::{self, EhFrames, EhIn, EhReloc, RelocClass, Target};
use crate::merge::{MergeIn, MergeMeta, merge};
use crate::problem::{Problem, What};

/// A relocation type as `.eh_frame` parsing needs it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct RelocInfo {
    pub name: &'static str,
    pub class: RelocClass,
    /// The data field's width; 0 for an instruction or a marker.
    pub width: u8,
}

/// The architecture's view of relocations (`spec/arch.md` §3.2, §3.3).
pub trait Relocs: Sync {
    /// Classifies `r_type` at `offset` of a section holding `bytes`.
    fn classify(&self, r_type: u32, bytes: &[u8], offset: u64) -> Result<RelocInfo, What>;
    /// REL only: the implicit addend of the classified relocation.
    fn addend(&self, r_type: u32, bytes: &[u8], offset: u64) -> i64;
}

/// [`Relocs`] by a `mortar_arch::Arch`.
#[derive(Clone, Copy)]
pub struct ArchRelocs<'a>(pub &'a dyn Arch);

impl std::fmt::Debug for ArchRelocs<'_> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("ArchRelocs")
    }
}

impl Relocs for ArchRelocs<'_> {
    fn classify(&self, r_type: u32, bytes: &[u8], offset: u64) -> Result<RelocInfo, What> {
        let h = self
            .0
            .classify(r_type, Site { bytes, offset })
            .map_err(|r| What::Refused(r.class, r.name.to_string()))?;
        let class = match h.class {
            DemandClass::None => RelocClass::None,
            DemandClass::Pc => RelocClass::Pc,
            DemandClass::AbsWord | DemandClass::AbsNarrow => RelocClass::Abs,
            _ => RelocClass::Other,
        };
        let width = match h.field {
            Field::Data { bytes } => bytes,
            _ => 0,
        };
        Ok(RelocInfo { name: h.name, class, width })
    }

    fn addend(&self, r_type: u32, bytes: &[u8], offset: u64) -> i64 {
        let Ok(h) = self.0.classify(r_type, Site { bytes, offset }) else { return 0 };
        let at = offset as usize;
        let w = usize::from(h.field.width());
        bytes.get(at..at + w).map_or(0, |field| self.0.read_addend(&h, field))
    }
}

/// What phase 60 produces.
#[derive(Debug, Clone)]
pub struct Phase {
    /// `spec/model.md` §3.5.
    pub split: Split,
    /// Per `split.merge_of` row: its pieces' identity (S-13).
    pub meta: Box<[MergeMeta]>,
    /// Every kept, assigned `.eh_frame`, parsed.
    pub eh: EhFrames,
    /// Per `eh.sections` row: its relocations, sorted by offset (the
    /// records' `relocs` spans index these).
    pub eh_relocs: Box<[Box<[EhReloc]>]>,
    /// The `Merge` sections placed whole (S-6), and why. Layout places
    /// them as `Regular` (`Split` has no row for them).
    pub whole: Box<[(SectionId, Whole)]>,
}

/// A kept section that has an output section, as phase 60 sees it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LinkSection {
    pub id: SectionId,
    pub kind: SectionKind,
    pub out: OutSecId,
    /// The relocation section applying to it; 0 for none.
    pub reloc: SectionIndex,
}

/// What phase 60 reads of a link. [`Tables`] implements it over
/// `Inputs`, `Resolution` and `Assignment`; a test implements it over
/// bare objects (a library scan).
pub trait Link<'s>: Sync {
    /// The number of files.
    fn files(&self) -> u32;
    /// The relocatable object of `file`, if it is one.
    fn object(&self, file: FileId) -> Option<Object<'s>>;
    /// The id of section `index` of `file`, kept or not.
    fn section_id(&self, file: FileId, index: SectionIndex) -> Option<SectionId>;
    /// Section `index` of `file`, when it is kept and assigned an output
    /// section.
    fn section(&self, file: FileId, index: SectionIndex) -> Option<LinkSection>;
    /// What global symbol `index` (at or after `first_global`) of `file`
    /// resolves to.
    fn global(&self, file: FileId, index: SymbolIndex, first_global: u32) -> Result<Target, What>;
    /// The bytes of section `s` of `file`.
    fn bytes(&self, file: FileId, s: &Section<'s>) -> Result<Cow<'s, [u8]>, What>;
    fn class(&self) -> Class;
    fn form(&self) -> RelocForm;
}

/// The link's tables phase 60 reads.
#[derive(Debug, Clone, Copy)]
pub struct Tables<'t, 's> {
    pub inputs: &'t Inputs<'s>,
    pub resolution: &'t Resolution,
    pub assignment: &'t Assignment<'t>,
}

impl<'s> Link<'s> for Tables<'_, 's> {
    fn files(&self) -> u32 {
        self.inputs.files.len()
    }

    fn object(&self, file: FileId) -> Option<Object<'s>> {
        match self.inputs.files.get(file)?.reader {
            Reader::Object(o) => Some(o),
            _ => None,
        }
    }

    fn section_id(&self, file: FileId, index: SectionIndex) -> Option<SectionId> {
        self.resolution.section_id(file, index)
    }

    fn section(&self, file: FileId, index: SectionIndex) -> Option<LinkSection> {
        let id = self.resolution.section_id(file, index)?;
        let row = self.resolution.sections[id];
        let out = self.assignment.out(id).filter(|_| row.is_kept())?;
        Some(LinkSection { id, kind: row.kind, out, reloc: row.reloc })
    }

    fn global(&self, file: FileId, index: SymbolIndex, first_global: u32) -> Result<Target, What> {
        let map = self.inputs.sym_map_of(file);
        let Some(&id) = map.get((index.0 - first_global) as usize) else {
            return Err(What::Malformed(format!("symbol {} has no key", index.0)));
        };
        let id: SymbolId = self.resolution.redirect.get(id).copied().unwrap_or(id);
        Ok(match self.resolution.def.get(id).copied() {
            Some(Def::Defined(e)) => match self.inputs.entry_def(e) {
                Some(EntryDef::Section(i, v)) => match self.resolution.section_id(e.file, i) {
                    Some(section) => Target::Section { section, value: v },
                    None => Target::Elsewhere(id),
                },
                Some(EntryDef::Absolute(v)) => Target::Absolute(v),
                None => Target::Elsewhere(id),
            },
            Some(Def::Undefined { weak }) => Target::Undefined { symbol: id, weak },
            _ => Target::Elsewhere(id),
        })
    }

    fn bytes(&self, file: FileId, s: &Section<'s>) -> Result<Cow<'s, [u8]>, What> {
        read(self.inputs, file, s)
    }

    fn class(&self) -> Class {
        self.inputs.target.class
    }

    fn form(&self) -> RelocForm {
        self.inputs.target.reloc
    }
}

/// A section of a file to split, with its data.
enum Unit<'a> {
    /// A `MergeIn` without its bytes, and the bytes.
    Merge(MergeIn<'static>, Cow<'a, [u8]>),
    Whole(SectionId, Whole),
    Eh {
        section: SectionId,
        index: u32,
        bytes: Cow<'a, [u8]>,
        relocs: Vec<EhReloc>,
    },
}

/// The bytes of section `s` of `file`: borrowed when the store holds
/// them resident (`Inputs::resident`), else read into a buffer of their
/// own with `Inputs::copy_to` (the staged store: bulk bytes are not
/// resident); for a file without a store entry (tests), the parsed
/// object's own bytes. Under the staged store the sections phase 60
/// reads (`Merge`, `.eh_frame` and the relocations of an allocated
/// section) are resident (`spec/input.md` §11.1), so the copy is the
/// fallback; a failed copy is `input-changed` or `cannot-open-file`,
/// never `malformed-input`.
pub fn read<'s>(inputs: &Inputs<'s>, file: FileId, s: &Section<'s>) -> Result<Cow<'s, [u8]>, What> {
    let h = s.header();
    if h.sh_type == mortar_elf::consts::SHT_NOBITS || h.sh_size == 0 {
        return Ok(Cow::Borrowed(&[]));
    }
    let range = h.sh_offset..h.sh_offset.saturating_add(h.sh_size);
    if inputs.files[file].bytes.is_none() {
        return s.data().map(Cow::Borrowed).map_err(elf_what);
    }
    if let Some(b) = inputs.resident(file, range.clone()) {
        return Ok(Cow::Borrowed(b));
    }
    let len = usize::try_from(h.sh_size).map_err(|_| What::SectionTooLarge)?;
    let mut buf = vec![0u8; len];
    inputs.copy_to(file, range, &mut buf).map_err(|e| match e {
        CopyError::Changed => What::InputChanged,
        CopyError::Read(errno) => What::ReadFailed(errno.0),
        // The section table's reader checked every section is inside
        // its file (`spec/elf.md` §3.2).
        CopyError::OutOfRange => What::Malformed("section extends past the end of the file".into()),
    })?;
    Ok(Cow::Owned(buf))
}

/// What a reader's error is: `malformed-input` with its text, except a
/// [`mortar_elf::Error::NotResident`], which is the store's fault
/// ([`What::NotResident`]).
fn elf_what(e: mortar_elf::Error) -> What {
    match e {
        mortar_elf::Error::NotResident { .. } => What::NotResident(e.to_string()),
        e => What::Malformed(e.to_string()),
    }
}

/// Phase 60: splits every kept `Merge` and `EhFrame` section that is
/// assigned an output section, merges, and parses `.eh_frame`. Problems
/// (sorted by key) when any input is malformed or refused.
pub fn run(pool: &Pool, t: &dyn Link<'_>, relocs: &dyn Relocs) -> Result<Phase, Vec<Problem>> {
    let files = t.files() as usize;
    let mut steps = mortar_core::wp032b_steps::Steps::new("split");
    let per_file: Vec<Result<Vec<Unit<'_>>, Problem>> =
        pool.unit_cost(480).map(files, |f| units(t, FileId(f as u32), relocs));
    let mut problems: Vec<Problem> =
        per_file.iter().filter_map(|u| u.as_ref().err().cloned()).collect();
    if !problems.is_empty() {
        problems.sort();
        return Err(problems);
    }
    steps.step("units", files as u64, "file");
    let mut merge_in = Vec::new();
    let mut whole = Vec::new();
    let mut eh_units = Vec::new();
    for (f, units) in per_file.into_iter().enumerate() {
        let Ok(units) = units else { unreachable!("problems returned above") };
        for u in units {
            match u {
                Unit::Merge(m, bytes) => merge_in.push((m, bytes)),
                Unit::Whole(s, w) => whole.push((s, w)),
                Unit::Eh { section, index, bytes, relocs } => {
                    eh_units.push((section, FileId(f as u32), index, bytes, relocs));
                }
            }
        }
    }
    let word = match t.class() {
        Class::Elf32 => 4,
        Class::Elf64 => 8,
    };
    let eh_in: Vec<EhIn<'_>> = eh_units
        .iter()
        .map(|(section, file, index, bytes, relocs)| EhIn {
            section: *section,
            file: *file,
            index: *index,
            bytes,
            relocs,
        })
        .collect();
    let merge_in: Vec<MergeIn<'_>> =
        merge_in.iter().map(|(m, bytes)| MergeIn { bytes, ..*m }).collect();
    steps.step("collect", (merge_in.len() + eh_in.len()) as u64, "section");
    let merged = merge(pool, &merge_in);
    steps.step("merge", 0, "");
    let frames = eh::parse(pool, word, &eh_in);
    steps.step("eh", eh_in.len() as u64, "section");
    let (merged, frames) = match (merged, frames) {
        (Ok(m), Ok(e)) => (m, e),
        (m, e) => {
            let mut problems: Vec<Problem> =
                m.err().into_iter().flatten().chain(e.err().into_iter().flatten()).collect();
            problems.sort();
            return Err(problems);
        }
    };
    let meta = merged.meta.clone();
    let split = merged.into_split(frames.split_rows());
    drop(eh_in);
    let eh_relocs = eh_units.into_iter().map(|u| u.4.into_boxed_slice()).collect();
    steps.step("finish", 0, "");
    Ok(Phase { split, meta, eh: frames, eh_relocs, whole: whole.into() })
}

/// The units of one file, in section order.
fn units<'s>(
    t: &dyn Link<'s>,
    file: FileId,
    relocs: &dyn Relocs,
) -> Result<Vec<Unit<'s>>, Problem> {
    let Some(object) = t.object(file) else { return Ok(Vec::new()) };
    let mut out = Vec::new();
    for index in 0..object.section_count() {
        let Some(row) = t.section(file, SectionIndex(index)) else { continue };
        let section = row.id;
        let problem = |what: What| Problem { file, section: index, piece: 0, offset: 0, what };
        let malformed = |e: mortar_elf::Error| problem(elf_what(e));
        match row.kind {
            SectionKind::Merge => {
                let s = object.section(SectionIndex(index)).map_err(malformed)?;
                let h = s.header();
                let w = if h.sh_flags & SHF_COMPRESSED != 0 {
                    // Placing whole is always correct (S-5); the bytes are
                    // compressed and not the pieces'.
                    Some(Whole::Compressed)
                } else {
                    placed_whole(h.sh_flags, h.sh_addralign, h.sh_entsize, row.reloc.0 != 0)
                };
                if let Some(w) = w {
                    out.push(Unit::Whole(section, w));
                    continue;
                }
                let out_sec = row.out;
                let bytes = t.bytes(file, &s).map_err(problem)?;
                out.push(Unit::Merge(
                    MergeIn {
                        section,
                        file,
                        index,
                        out: out_sec,
                        strings: h.sh_flags & SHF_STRINGS != 0,
                        entsize: h.sh_entsize as u32,
                        align: align_log2(h.sh_addralign),
                        bytes: &[],
                    },
                    bytes,
                ));
            }
            SectionKind::EhFrame => {
                let s = object.section(SectionIndex(index)).map_err(malformed)?;
                if s.flags() & SHF_COMPRESSED != 0 {
                    return Err(problem(What::Refused(
                        mortar_diag::RefusalClass::Feature,
                        "SHF_COMPRESSED .eh_frame".into(),
                    )));
                }
                let bytes = t.bytes(file, &s).map_err(problem)?;
                let rel = if row.reloc.0 == 0 {
                    Vec::new()
                } else {
                    eh_relocs(t, file, &object, row.reloc, &bytes, relocs)
                        .map_err(|(offset, what)| Problem { offset, ..problem(what) })?
                };
                out.push(Unit::Eh { section, index, bytes, relocs: rel });
            }
            _ => {}
        }
    }
    Ok(out)
}

/// The relocations of an `.eh_frame`, classified, resolved and sorted by
/// offset.
fn eh_relocs<'s>(
    t: &dyn Link<'s>,
    file: FileId,
    object: &Object<'s>,
    reloc: SectionIndex,
    bytes: &[u8],
    relocs: &dyn Relocs,
) -> Result<Vec<EhReloc>, (u64, What)> {
    let malformed = |e: mortar_elf::Error| (0, elf_what(e));
    let rs: Section<'s> = object.section(reloc).map_err(malformed)?;
    let records = t.bytes(file, &rs).map_err(|w| (0, w))?;
    let class = object.class();
    let raw: Vec<(u64, u32, u32, Option<i64>)> = match t.form() {
        RelocForm::Rela => Table::<Rela>::new(&records, class)
            .iter()
            .map(|r| (r.r_offset, r.r_sym, r.r_type, Some(r.r_addend)))
            .collect(),
        RelocForm::Rel => Table::<Rel>::new(&records, class)
            .iter()
            .map(|r| (r.r_offset, r.r_sym, r.r_type, None))
            .collect(),
    };
    let symbols = object.symbols().map_err(malformed)?;
    let mut out = Vec::with_capacity(raw.len());
    for (offset, r_sym, r_type, addend) in raw {
        let fail = |what: What| (offset, what);
        let info = relocs.classify(r_type, bytes, offset).map_err(fail)?;
        let addend = match addend {
            Some(a) => a,
            None if info.class == RelocClass::None => 0,
            None => relocs.addend(r_type, bytes, offset),
        };
        let target = if r_sym == 0 || info.class == RelocClass::None {
            Target::Absolute(0)
        } else {
            let symbols = symbols
                .as_ref()
                .ok_or_else(|| fail(What::Malformed("relocation without a symbol table".into())))?;
            target(t, file, symbols, SymbolIndex(r_sym)).map_err(|w| (offset, w))?
        };
        out.push(EhReloc {
            offset,
            r_type,
            name: info.name,
            class: info.class,
            width: info.width,
            addend,
            target,
        });
    }
    out.sort_by_key(|r| r.offset);
    Ok(out)
}

/// What symbol `index` of `file` resolves to (resolve.md: locals by their
/// section, globals by [`Link::global`]).
fn target(
    t: &dyn Link<'_>,
    file: FileId,
    symbols: &mortar_elf::SymbolTable<'_>,
    index: SymbolIndex,
) -> Result<Target, What> {
    let malformed = elf_what;
    let sym = symbols.get(index).map_err(malformed)?;
    let first_global = symbols.first_global().0;
    if index.0 >= first_global && sym.st_info & 0xf != STT_SECTION {
        return t.global(file, index, first_global);
    }
    match symbols.section_of(index, &sym).map_err(malformed)? {
        SymbolSection::Section(i) => match t.section_id(file, i) {
            Some(section) => Ok(Target::Section { section, value: sym.st_value }),
            None => Err(What::Malformed(format!("symbol in section {} which does not exist", i.0))),
        },
        SymbolSection::Absolute | SymbolSection::Undefined => Ok(Target::Absolute(sym.st_value)),
        SymbolSection::Common | SymbolSection::Reserved(_) => {
            Err(What::Malformed("local symbol in a reserved section index".into()))
        }
    }
}

/// The diagnostics of `problems`, keyed (`spec/model.md` §8), with the
/// file and section they are at.
#[must_use]
pub fn diagnostics(inputs: &Inputs<'_>, problems: &[Problem]) -> Vec<(Key, Diagnostic)> {
    problems
        .iter()
        .map(|p| {
            let f = &inputs.files[p.file];
            let name = match f.reader {
                Reader::Object(o) => o
                    .section(SectionIndex(p.section))
                    .ok()
                    .and_then(|s| s.name().ok())
                    .map(|n| String::from_utf8_lossy(n).into_owned()),
                _ => None,
            };
            let name = name.unwrap_or_else(|| format!("section {}", p.section));
            (p.key(), p.diagnostic(f.display(), &name))
        })
        .collect()
}
