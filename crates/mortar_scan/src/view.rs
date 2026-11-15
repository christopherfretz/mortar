//! What the scan reads, as `mortar_arch`'s traits see it: symbol facts
//! and names ([`ScanTables`], [`SymNames`]) over `Inputs` and
//! `Resolution`, and a relocation section's records ([`Records`]).
//!
//! Per file the view keeps, built on first use by whichever unit needs
//! it (a pure function of the file, so the schedule never shows), the
//! parsed symbol table and the file's mapping marks.

use std::cell::RefCell;
use std::sync::OnceLock;

use mortar_arch::{Arch, Records, Reloc, ScanTables, Site, SymFacts, SymNames, TargetKind};
use mortar_core::assignment::Assignment;
use mortar_core::inputs::{CopyError, Inputs, Reader};
use mortar_core::resolution::{Def, InternalDef, RefFlags, Resolution, Verdict};
use mortar_core::{FileId, IdSet, Liveness, SectionId, SymEntry, SymKey, SymKind, SymbolId};
use mortar_diag::{Diagnostic, FileRef, Kind};
use mortar_elf::consts::{SHT_REL, SHT_RELA, STT_FUNC, STT_GNU_IFUNC, STT_SECTION, STT_TLS};
use mortar_elf::{
    Object, Rel, Rela, Section, SectionIndex, Sym, SymbolIndex, SymbolSection, SymbolTable, Table,
};

use crate::{CodeProof, Mark, Mismatch, ScanInputs, ScanOptions};

/// A file as the scan reads it.
pub(crate) struct Prep<'s> {
    pub obj: Option<Object<'s>>,
    /// The symbol table (`.symtab`; `.dynsym` of a shared object), or why
    /// it cannot be read.
    pub symtab: Result<Option<SymbolTable<'s>>, String>,
    /// The mapping marks of the file's sections, sorted by `(section,
    /// value, index)`, and per section index where its marks start.
    marks: OnceLock<FileMarks>,
}

impl<'s> Prep<'s> {
    pub fn table(&self) -> Option<&SymbolTable<'s>> {
        self.symtab.as_ref().ok().and_then(Option::as_ref)
    }

    pub fn sym(&self, index: SymbolIndex) -> Option<Sym> {
        self.table()?.get(index).ok()
    }
}

/// The scan's view of the link.
pub(crate) struct View<'a, 's> {
    pub inputs: &'a Inputs<'s>,
    pub resolution: &'a Resolution,
    pub liveness: &'a Liveness,
    pub assignment: Option<&'a Assignment<'a>>,
    pub arch: &'a dyn Arch,
    pub opts: ScanOptions<'a>,
    preps: Vec<OnceLock<Prep<'s>>>,
    /// The keys of `comdat_mismatch`, sorted.
    mismatch: Vec<SymbolId>,
    /// `comdat_mismatch`, sorted by key.
    pub mismatches: Vec<Mismatch>,
    /// The keys with a `.gnu.warning.<key>` section, sorted.
    warned: Vec<SymbolId>,
    /// `gnu_warnings`, sorted.
    pub gnu_warnings: Vec<(SymbolId, FileId, SectionIndex)>,
    /// `spec/dyn.md` D-25: the `WeakDyn` keys ([`crate::weak::classes`]);
    /// empty until it ran, and in every static link.
    pub weak_dyn: IdSet<SymbolId>,
    /// The undefined weak keys refused by D-25: the walk drops what the
    /// architecture's scan says of their relocations.
    pub weak_refused: IdSet<SymbolId>,
}

/// The name of the linker-defined TLS symbol (`spec/resolve.md` §8.1).
const TLS_MODULE_BASE: &[u8] = b"_TLS_MODULE_BASE_";

impl<'a, 's> View<'a, 's> {
    pub fn new(arch: &'a dyn Arch, t: &ScanInputs<'a, 's>, opts: ScanOptions<'a>) -> Self {
        let files = t.inputs.files.len() as usize;
        let mut mismatch: Vec<SymbolId> = t.comdat_mismatch.iter().map(|m| m.key).collect();
        mismatch.sort_unstable();
        mismatch.dedup();
        let mut warned: Vec<SymbolId> = t.gnu_warnings.iter().map(|w| w.0).collect();
        warned.sort_unstable();
        warned.dedup();
        Self {
            inputs: t.inputs,
            resolution: t.resolution,
            liveness: t.liveness,
            assignment: t.assignment,
            arch,
            opts,
            preps: (0..files).map(|_| OnceLock::new()).collect(),
            mismatch,
            mismatches: {
                let mut v = t.comdat_mismatch.to_vec();
                v.sort_unstable();
                v
            },
            warned,
            gnu_warnings: {
                let mut v = t.gnu_warnings.to_vec();
                v.sort_unstable();
                v
            },
            weak_dyn: IdSet::new(),
            weak_refused: IdSet::new(),
        }
    }

    pub fn prep(&self, file: FileId) -> Option<&Prep<'s>> {
        let cell = self.preps.get(file.0 as usize)?;
        Some(cell.get_or_init(|| {
            let (obj, symtab) = match self.inputs.files[file].reader {
                Reader::Object(o) => (Some(o), o.symbols().map_err(|e| e.to_string())),
                Reader::Shared(s) => (None, s.symbols().map_err(|e| e.to_string())),
                Reader::Internal => (None, Ok(None)),
            };
            Prep { obj, symtab, marks: OnceLock::new() }
        }))
    }

    pub fn sym(&self, file: FileId, index: SymbolIndex) -> Option<Sym> {
        self.prep(file)?.sym(index)
    }

    /// The mapping marks of section `section` of `file`, in `(value,
    /// index)` order.
    pub fn marks(&self, file: FileId, section: u32) -> Vec<Mark> {
        let Some(p) = self.prep(file) else { return Vec::new() };
        let all = p.marks.get_or_init(|| file_marks(p, self.opts.proof));
        // `starts[s]..starts[s + 1]`: the marks of section `s` (an index
        // per section, not a search per call: the scan, the sites and the
        // fixed point ask for every code section, some many times).
        let s = section as usize;
        match (all.starts.get(s), all.starts.get(s + 1)) {
            (Some(&a), Some(&b)) => all.marks[a as usize..b as usize].to_vec(),
            _ => Vec::new(),
        }
    }

    pub fn is_mismatch(&self, id: SymbolId) -> bool {
        self.mismatch.binary_search(&id).is_ok()
    }

    pub fn is_warned(&self, id: SymbolId) -> bool {
        !self.warned.is_empty() && self.warned.binary_search(&id).is_ok()
    }

    pub fn def(&self, id: SymbolId) -> Def {
        let d = self.assignment.and_then(|a| a.def(self.resolution, id));
        d.or_else(|| self.resolution.def.get(id).copied()).unwrap_or(Def::Undefined { weak: false })
    }

    pub fn is_live(&self, file: FileId, index: SectionIndex) -> bool {
        self.resolution.section_id(file, index).is_some_and(|s| self.liveness.is_live(s))
    }

    /// The key record symbol `r_sym` of `file` names (`spec/model.md`
    /// §3.3 `redirect`: applied to undefined mentions). `None` when the
    /// index is outside the symbol table.
    pub fn key_of(
        &self,
        file: FileId,
        table: Option<&SymbolTable<'s>>,
        r_sym: u32,
    ) -> Option<SymKey> {
        let local = |index| SymKey::local(SymEntry { file, index: SymbolIndex(index) });
        let Some(t) = table else {
            return (r_sym == 0).then(|| local(0));
        };
        if r_sym >= t.len() {
            return None;
        }
        let first = t.first_global().0;
        if r_sym < first {
            return Some(local(r_sym));
        }
        let id = *self.inputs.sym_map_of(file).get((r_sym - first) as usize)?;
        let to = self.resolution.redirect.get(id).copied().unwrap_or(id);
        if to != id {
            let s = t.get(SymbolIndex(r_sym)).ok()?;
            if t.section_of(SymbolIndex(r_sym), &s).ok()? == SymbolSection::Undefined {
                return Some(SymKey::global(to));
            }
        }
        Some(SymKey::global(id))
    }

    /// What `arch.scan` knows of `key` (`spec/arch.md` §3.4).
    /// `mention`: the referencing entry, whose type an undefined key
    /// takes.
    ///
    /// The three event facts (R-1): `dead`, a local of a section of its
    /// file that is not live (§6.1); `comdat_only`, a key only dropped
    /// COMDAT copies define (`ComdatMismatch`); `gnu_warning`, a key with
    /// a `.gnu.warning` section in a live file.
    pub fn facts(&self, key: SymKey, mention: Option<SymEntry>) -> SymFacts {
        match key.kind() {
            SymKind::Local(e) => self.local_facts(e),
            SymKind::Global(id) => {
                let mut f = self.global_facts(id, mention, 0);
                f.comdat_only = self.is_mismatch(id);
                f.gnu_warning = self.is_warned(id);
                f
            }
        }
    }

    fn local_facts(&self, e: SymEntry) -> SymFacts {
        let abs = SymFacts {
            kind: TargetKind::Abs,
            sym_type: 0,
            st_other: 0,
            tls: false,
            dead: false,
            comdat_only: false,
            gnu_warning: false,
        };
        let Some(prep) = self.prep(e.file) else { return abs };
        let Some(table) = prep.table() else { return abs };
        let Ok(sym) = table.get(e.index) else { return abs };
        let section = table.section_of(e.index, &sym);
        let kind = match section {
            Ok(SymbolSection::Section(_)) if sym.sym_type() == STT_GNU_IFUNC => TargetKind::Ifunc,
            Ok(SymbolSection::Section(_)) => TargetKind::Local,
            _ => TargetKind::Abs,
        };
        // §6.1: a local of a section of its file that is not live (the
        // symbol read once for the kind and for this).
        let dead = e.index.0 != 0
            && matches!(section, Ok(SymbolSection::Section(sec)) if !self.is_live(e.file, sec));
        SymFacts {
            kind,
            sym_type: sym.sym_type(),
            st_other: sym.st_other,
            tls: sym.sym_type() == STT_TLS,
            dead,
            comdat_only: false,
            gnu_warning: false,
        }
    }

    fn global_facts(&self, id: SymbolId, mention: Option<SymEntry>, depth: u32) -> SymFacts {
        let of = |kind, s: Option<Sym>| SymFacts {
            kind,
            sym_type: s.map_or(0, |s| s.sym_type()),
            st_other: s.map_or(0, |s| s.st_other),
            tls: s.is_some_and(|s| s.sym_type() == STT_TLS),
            dead: false,
            comdat_only: false,
            gnu_warning: false,
        };
        let props = self.resolution.props.get(id).copied().unwrap_or_default();
        let preemptible = props.refs.contains(RefFlags::PREEMPTIBLE);
        match self.def(id) {
            Def::Defined(e) => {
                let s = self.sym(e.file, e.index);
                let place =
                    s.and_then(|s| self.prep(e.file)?.table()?.section_of(e.index, &s).ok());
                let kind = match (place, s.map(|s| s.sym_type())) {
                    (Some(SymbolSection::Absolute), _) => TargetKind::Abs,
                    (_, Some(STT_GNU_IFUNC)) if !preemptible => TargetKind::Ifunc,
                    _ if preemptible => TargetKind::Preemptible,
                    _ => TargetKind::Local,
                };
                // Under `--gc-sections`, a definition in a kept section the
                // mark did not reach is dead (§6.1, `spec/image.md` I-7):
                // live code cannot refer to it by construction, a debug
                // section gets the tombstone (I-47), any other section is
                // `discarded-section-reference` (I-12). Without it every
                // kept section is live and this is `false`.
                let dead = match place {
                    Some(SymbolSection::Section(i)) => {
                        self.resolution.section_id(e.file, i).is_some_and(|sid| {
                            self.resolution.sections[sid].is_kept() && !self.liveness.is_live(sid)
                        })
                    }
                    _ => false,
                };
                SymFacts { dead, ..of(kind, s) }
            }
            Def::Common(e) => of(TargetKind::Local, self.sym(e.file, e.index)),
            Def::Shared(e) => {
                let s = self.sym(e.file, e.index);
                let func = s.is_some_and(|s| matches!(s.sym_type(), STT_FUNC | STT_GNU_IFUNC));
                of(TargetKind::Shlib { func }, s)
            }
            Def::Internal(InternalDef::Alias(to)) if to != id && depth < 8 => {
                self.global_facts(to, None, depth + 1)
            }
            Def::Internal(InternalDef::Assign(_) | InternalDef::Alias(_)) => {
                of(TargetKind::Abs, None)
            }
            Def::Internal(InternalDef::Provide { .. }) => {
                let tls = self.inputs.symbols.get(id).is_some_and(|k| k.name == TLS_MODULE_BASE);
                let sym_type = if tls { STT_TLS } else { 0 };
                SymFacts {
                    kind: TargetKind::Local,
                    sym_type,
                    st_other: 0,
                    tls,
                    dead: false,
                    comdat_only: false,
                    gnu_warning: false,
                }
            }
            Def::Undefined { weak } => {
                let s = mention.and_then(|m| self.sym(m.file, m.index));
                of(self.undefined_kind(id, weak, props.verdict), s)
            }
        }
    }

    /// The keys a reference to which can make an event (§4.4): those
    /// whose target kind is `Undefined` (an undefined key, or an alias of
    /// one), those only dropped COMDAT copies define, and those with a
    /// `.gnu.warning` section; sorted. A record whose key is none of
    /// these, at a place where a dead target is the tombstone, makes no
    /// event (`spec/image.md` I-14: the sections fill checks).
    pub fn event_keys(&self) -> Vec<SymbolId> {
        let mut v: Vec<SymbolId> = Vec::new();
        for i in 0..self.inputs.symbols.len() {
            let id = SymbolId(i);
            let undefined = match self.def(id) {
                Def::Defined(_) | Def::Common(_) | Def::Shared(_) => false,
                Def::Undefined { weak } => {
                    let verdict = self.resolution.props.get(id).map(|p| p.verdict);
                    self.undefined_kind(id, weak, verdict.unwrap_or_default())
                        == TargetKind::Undefined
                }
                Def::Internal(_) => self.global_facts(id, None, 0).kind == TargetKind::Undefined,
            };
            if undefined {
                v.push(id);
            }
        }
        v.extend_from_slice(&self.mismatch);
        v.extend_from_slice(&self.warned);
        v.sort_unstable();
        v.dedup();
        v
    }

    /// The target kind of an undefined key (§4.4, I-19): a weak one is
    /// zero, or dynamic when D-25 made it so (`weak_dyn`, per key); a
    /// strong one is `Undefined` unless its verdict is "nothing"
    /// (`--unresolved-symbols=ignore-*`), which makes it zero.
    fn undefined_kind(&self, id: SymbolId, weak: bool, verdict: Verdict) -> TargetKind {
        let zero = if weak && self.weak_dyn.contains(id) {
            TargetKind::WeakDyn
        } else {
            TargetKind::Weak0
        };
        match (weak, verdict) {
            (true, _) | (false, Verdict::Ignored) => zero,
            _ => TargetKind::Undefined,
        }
    }

    /// A key's name as its symbol table has it.
    pub fn name(&self, key: SymKey) -> &'s [u8] {
        match key.kind() {
            SymKind::Global(id) => self.inputs.symbols.get(id).map_or(&[][..], |k| k.name),
            SymKind::Local(e) => {
                let Some(t) = self.prep(e.file).and_then(Prep::table) else { return &[] };
                t.get(e.index).ok().and_then(|s| t.name(&s).ok()).unwrap_or_default()
            }
        }
    }

    /// The name a diagnostic shows for `key`: the symbol's name with its
    /// version, or a section symbol's section name.
    pub fn shown_name(&self, key: SymKey) -> String {
        match key.kind() {
            SymKind::Global(id) => {
                let Some(k) = self.inputs.symbols.get(id) else { return String::new() };
                let name = String::from_utf8_lossy(k.name);
                match k.version {
                    Some(v) => format!("{name}@{}", String::from_utf8_lossy(v)),
                    None => name.into_owned(),
                }
            }
            SymKind::Local(e) => {
                let Some(p) = self.prep(e.file) else { return String::new() };
                let Some(t) = p.table() else { return String::new() };
                let Ok(s) = t.get(e.index) else { return String::new() };
                if s.sym_type() == STT_SECTION
                    && let Ok(SymbolSection::Section(i)) = t.section_of(e.index, &s)
                {
                    return self.section_name(e.file, i.0);
                }
                String::from_utf8_lossy(t.name(&s).unwrap_or_default()).into_owned()
            }
        }
    }

    pub fn section_name(&self, file: FileId, index: u32) -> String {
        let name = self
            .prep(file)
            .and_then(|p| p.obj)
            .and_then(|o| o.section(SectionIndex(index)).ok())
            .and_then(|s| s.name().ok())
            .unwrap_or_default();
        String::from_utf8_lossy(name).into_owned()
    }

    pub fn section_of(&self, file: FileId, index: u32) -> Option<SectionId> {
        self.resolution.section_id(file, SectionIndex(index))
    }
}

/// A file's mapping marks: `marks` sorted by `(section, value, index)`;
/// those of section `s` are `starts[s]..starts[s + 1]` (`starts` ends
/// after the last section with a mark).
#[derive(Debug, Default)]
pub(crate) struct FileMarks {
    marks: Box<[Mark]>,
    starts: Box<[u32]>,
}

fn file_marks(p: &Prep<'_>, proof: &dyn CodeProof) -> FileMarks {
    let Some(t) = p.table() else { return FileMarks::default() };
    let mut out = Vec::new();
    for (index, s) in t.locals() {
        let Ok(name) = t.name(&s) else { continue };
        let Some(code) = proof.mark(name, s.st_info) else { continue };
        if let Ok(SymbolSection::Section(sec)) = t.section_of(index, &s) {
            out.push((sec.0, Mark { index: index.0, value: s.st_value, code }));
        }
    }
    out.sort_unstable_by_key(|m| (m.0, m.1.value, m.1.index));
    let sections = out.last().map_or(0, |m| m.0 as usize + 1);
    let mut starts = vec![0u32; sections + 1];
    for m in &out {
        starts[m.0 as usize + 1] += 1;
    }
    for s in 1..starts.len() {
        starts[s] += starts[s - 1];
    }
    FileMarks { marks: out.into_iter().map(|m| m.1).collect(), starts: starts.into() }
}

/// One decoded record: the explicit addend of `SHT_RELA`, none for
/// `SHT_REL`.
#[derive(Debug, Clone, Copy)]
pub(crate) struct Raw {
    pub r_offset: u64,
    pub r_sym: u32,
    pub r_type: u32,
    pub addend: Option<i64>,
}

/// A relocation section's records, either form.
#[derive(Debug, Clone, Copy)]
pub(crate) enum RecTable<'s> {
    Rela(Table<'s, Rela>),
    Rel(Table<'s, Rel>),
}

impl RecTable<'_> {
    pub fn len(&self) -> usize {
        match self {
            Self::Rela(t) => t.len(),
            Self::Rel(t) => t.len(),
        }
    }

    pub fn raw(&self, i: u32) -> Option<Raw> {
        match self {
            Self::Rela(t) => t.get(i as usize).map(|r| Raw {
                r_offset: r.r_offset,
                r_sym: r.r_sym,
                r_type: r.r_type,
                addend: Some(r.r_addend),
            }),
            Self::Rel(t) => t.get(i as usize).map(|r| Raw {
                r_offset: r.r_offset,
                r_sym: r.r_sym,
                r_type: r.r_type,
                addend: None,
            }),
        }
    }
}

/// Why the bytes of a section could not be had (`spec/input.md` §11.3,
/// §11.5). None of these is ever read as "no bytes".
#[derive(Debug, Clone)]
pub(crate) enum ReadFail {
    /// `mortar_elf`'s statement: `malformed-input`.
    Malformed(String),
    /// The copy of bytes that are not resident failed: `input-changed`,
    /// or `cannot-open-file` for a read error.
    Copy(CopyError),
    /// Bytes the store's rules keep resident, or a file without a store
    /// entry, were not resident: `internal-error`.
    Internal(String),
}

impl ReadFail {
    fn elf(e: mortar_elf::Error) -> Self {
        Self::Malformed(e.to_string())
    }

    /// The diagnostic, before its place.
    pub fn diagnostic(&self, path: &FileRef) -> Diagnostic {
        match self {
            Self::Malformed(what) => Diagnostic::error(Kind::Malformed { what: what.clone() }),
            Self::Copy(CopyError::Changed) => Diagnostic::error(Kind::InputChanged),
            Self::Copy(CopyError::Read(errno)) => {
                let reason = if errno.0 == 0 {
                    "the read failed".to_owned()
                } else {
                    mortar_diag::os_reason(&std::io::Error::from_raw_os_error(errno.0))
                };
                Diagnostic::error(Kind::CannotOpenFile { path: path.path().to_owned(), reason })
            }
            Self::Copy(CopyError::OutOfRange) => Diagnostic::error(Kind::InternalError {
                what: format!("scan: a copy outside {}", path.path()),
            }),
            Self::Internal(what) => Diagnostic::error(Kind::InternalError { what: what.clone() }),
        }
    }
}

/// The scan's buffers for bytes the store does not hold resident: the
/// records of a relocation section and the contents of the section they
/// apply to. One per worker, reused unit after unit ([`with_bufs`]).
#[derive(Debug, Default)]
pub(crate) struct Bufs {
    pub rel: Vec<u8>,
    pub sec: Vec<u8>,
}

thread_local! {
    static BUFS: RefCell<Bufs> = RefCell::default();
}

/// Runs `f` with this worker's [`Bufs`] (fresh ones if they are in use).
pub(crate) fn with_bufs<T>(f: impl FnOnce(&mut Bufs) -> T) -> T {
    BUFS.with(|cell| match cell.try_borrow_mut() {
        Ok(mut b) => f(&mut b),
        Err(_) => f(&mut Bufs::default()),
    })
}

/// Copies `offset..offset + size` of `file` into `buf` (one `copy_to`).
fn copy(
    view: &View<'_, '_>,
    file: FileId,
    offset: u64,
    size: u64,
    buf: &mut Vec<u8>,
) -> Result<(), ReadFail> {
    if view.inputs.files[file].bytes.is_none() {
        return Err(ReadFail::Internal(format!(
            "scan: {size} bytes at offset {offset} of file {} are not resident and it has no store entry",
            file.0
        )));
    }
    let len = usize::try_from(size).map_err(|_| ReadFail::Copy(CopyError::OutOfRange))?;
    buf.clear();
    buf.resize(len, 0);
    let end = offset.checked_add(size).ok_or(ReadFail::Copy(CopyError::OutOfRange))?;
    view.inputs.copy_to(file, offset..end, buf).map_err(ReadFail::Copy)
}

/// The contents of section `s` of `file`: the resident slice, or one
/// `copy_to` into `buf` when they are not resident (`spec/input.md`
/// §11.3).
pub(crate) fn contents<'b, 's: 'b>(
    view: &View<'_, 's>,
    file: FileId,
    s: &Section<'s>,
    buf: &'b mut Vec<u8>,
) -> Result<&'b [u8], ReadFail> {
    match s.data() {
        Ok(d) => Ok(d),
        Err(mortar_elf::Error::NotResident { offset, size }) => {
            copy(view, file, offset, size, buf)?;
            let buf: &'b Vec<u8> = buf;
            Ok(buf)
        }
        Err(e) => Err(ReadFail::elf(e)),
    }
}

/// The records of relocation section `rel` of `file`, with its `sh_link`
/// and `sh_info`, or why they cannot be read: resident, or copied into
/// `buf` (one `copy_to`) when the store does not hold them (the
/// relocations of a non-alloc section, `spec/input.md` §11.1).
pub(crate) fn rec_table<'b, 's: 'b>(
    view: &View<'_, 's>,
    file: FileId,
    obj: &Object<'s>,
    rel: SectionIndex,
    buf: &'b mut Vec<u8>,
) -> Result<(RecTable<'b>, SectionIndex, SectionIndex), ReadFail> {
    let s = obj.section(rel).map_err(ReadFail::elf)?;
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
        s.rela().map(|r| (RecTable::Rela(r.records()), r.symtab(), r.target()))
    } else {
        s.rel().map(|r| (RecTable::Rel(r.records()), r.symtab(), r.target()))
    };
    match resident {
        Ok(t) => Ok(t),
        Err(mortar_elf::Error::NotResident { offset, size }) => {
            copy(view, file, offset, size, buf)?;
            let buf: &'b Vec<u8> = buf;
            if rela {
                let r = s.rela_in(buf).map_err(ReadFail::elf)?;
                Ok((RecTable::Rela(r.records()), r.symtab(), r.target()))
            } else {
                let r = s.rel_in(buf).map_err(ReadFail::elf)?;
                Ok((RecTable::Rel(r.records()), r.symtab(), r.target()))
            }
        }
        Err(e) => Err(ReadFail::elf(e)),
    }
}

/// The `r_offset` of record `record` of relocation section `rel` of
/// `file`: from the resident records, or one `copy_to` of that record.
pub(crate) fn rec_offset(
    view: &View<'_, '_>,
    file: FileId,
    obj: &Object<'_>,
    rel: SectionIndex,
    record: u32,
) -> Result<Option<u64>, ReadFail> {
    let s = obj.section(rel).map_err(ReadFail::elf)?;
    let resident = match s.sh_type() {
        SHT_RELA => s.rela().map(|r| RecTable::Rela(r.records())),
        SHT_REL => s.rel().map(|r| RecTable::Rel(r.records())),
        t => {
            return Err(ReadFail::Malformed(format!(
                "section {rel} applies relocations but has type {t:#x}"
            )));
        }
    };
    match resident {
        Ok(t) => Ok(t.raw(record).map(|r| r.r_offset)),
        Err(mortar_elf::Error::NotResident { offset, size }) => {
            let entsize = s.header().sh_entsize;
            let at = u64::from(record) * entsize;
            if at >= size {
                return Ok(None);
            }
            let mut buf = Vec::new();
            copy(view, file, offset + at, entsize, &mut buf)?;
            let t = if s.sh_type() == SHT_RELA {
                RecTable::Rela(Table::new(&buf, obj.class()))
            } else {
                RecTable::Rel(Table::new(&buf, obj.class()))
            };
            Ok(t.raw(0).map(|r| r.r_offset))
        }
        Err(e) => Err(ReadFail::elf(e)),
    }
}

/// The records of one relocation section as `mortar_arch` reads them
/// (§3.5): each record's symbol resolved to its key.
pub(crate) struct Recs<'v, 'a, 's, 'b> {
    pub view: &'v View<'a, 's>,
    pub file: FileId,
    pub table: Option<&'v SymbolTable<'s>>,
    pub records: RecTable<'b>,
    /// The contents of the section the records apply to: resident, or
    /// copied into the worker's [`Bufs`].
    pub bytes: &'b [u8],
}

impl Recs<'_, '_, '_, '_> {
    /// The addend of a record: explicit, or read from the field (REL).
    pub fn addend(&self, raw: &Raw, h: &mortar_arch::Howto) -> i64 {
        raw.addend.unwrap_or_else(|| {
            let from = usize::try_from(raw.r_offset).unwrap_or(usize::MAX);
            self.view.arch.read_addend(h, self.bytes.get(from..).unwrap_or_default())
        })
    }
}

impl Records for Recs<'_, '_, '_, '_> {
    fn len(&self) -> u32 {
        u32::try_from(self.records.len()).unwrap_or(u32::MAX)
    }

    fn get(&self, i: u32) -> Option<Reloc> {
        let raw = self.records.raw(i)?;
        let key = self.view.key_of(self.file, self.table, raw.r_sym)?;
        let addend = if let Some(a) = raw.addend {
            a
        } else {
            let site = Site { bytes: self.bytes, offset: raw.r_offset };
            let h = self.view.arch.classify(raw.r_type, site).ok()?;
            self.addend(&raw, &h)
        };
        Some(Reloc { r_offset: raw.r_offset, key, r_type: raw.r_type, addend })
    }
}

/// The tables as one record's scan sees them: an undefined key takes
/// the type of the record's own mention (`SymFacts::sym_type`).
pub(crate) struct RecTables<'v, 'a, 's> {
    pub view: &'v View<'a, 's>,
    pub key: SymKey,
    pub mention: SymEntry,
}

impl SymNames for RecTables<'_, '_, '_> {
    fn name(&self, key: SymKey) -> &[u8] {
        self.view.name(key)
    }
}

impl ScanTables for RecTables<'_, '_, '_> {
    fn sym(&self, key: SymKey) -> SymFacts {
        self.view.facts(key, (key == self.key).then_some(self.mention))
    }
}
