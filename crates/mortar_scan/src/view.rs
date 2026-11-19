//! What the scan reads, as `mortar_arch`'s traits see it: symbol facts
//! and names ([`ScanTables`], [`SymNames`]) over `Inputs` and
//! `Resolution`, and a relocation section's records ([`Records`]).
//!
//! Per file the view keeps, built on first use by whichever unit needs
//! it (a pure function of the file, so the schedule never shows), the
//! parsed symbol table and the file's mapping marks.

use std::cell::RefCell;
use std::ops::Range;
use std::sync::OnceLock;

use mortar_arch::{
    Arch, OutputKind, Records, Reloc, ScanTables, Site, SymFacts, SymNames, TargetKind,
};
use mortar_core::assignment::Assignment;
use mortar_core::inputs::{CopyError, Inputs, Reader};
use mortar_core::resolution::{
    Assigned, Def, DropReason, InternalDef, RefFlags, Resolution, SectionKind, SectionState,
    Verdict,
};
use mortar_core::{FileId, IdSet, Liveness, SectionId, SymEntry, SymKey, SymKind, SymbolId};
use mortar_diag::{Diagnostic, FileRef, Kind};
use mortar_elf::consts::{SHT_REL, SHT_RELA, STT_FUNC, STT_GNU_IFUNC, STT_SECTION, STT_TLS};
use mortar_elf::{
    Machine, Object, Rel, Rela, Section, SectionIndex, Sym, SymbolIndex, SymbolSection,
    SymbolTable, Table,
};

use crate::{CodeProof, EhOwner, Mark, Mismatch, ScanInputs, ScanOptions};

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

/// The whole link's program: what [`Whole`] answers in a full link.
#[derive(Clone, Copy)]
pub struct Program<'a, 's> {
    pub inputs: &'a Inputs<'s>,
    pub resolution: &'a Resolution,
    pub liveness: &'a Liveness,
    pub assignment: Option<&'a Assignment<'a>>,
}

impl std::fmt::Debug for Program<'_, '_> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Program").finish_non_exhaustive()
    }
}

impl Program<'_, '_> {
    /// Key `id`'s definition, as `Assignment::def` leaves it.
    pub fn def(&self, id: SymbolId) -> Def {
        let d = self.assignment.and_then(|a| a.def(self.resolution, id));
        d.or_else(|| self.resolution.def.get(id).copied()).unwrap_or(Def::Undefined { weak: false })
    }
}

/// The whole-program reads of the scan's walk of ONE file (WP-255d5; the
/// rows of `spec/landed/incr/WP-255d4.md` M1, "Whole-program reads"):
/// the whole link's [`Program`], or an update's narrow view of a file,
/// answered from the stored state and the update's own deltas. The walk
/// is generic over it (monomorphized: no `dyn` in the loop).
pub trait Whole<'s>: Sync {
    /// `Some` for the whole link. The scan then computes the per-key
    /// facts, `Weak0`, the event keys and the COMDAT-mismatch keys itself
    /// from the program, and never asks [`Whole::key_facts`],
    /// [`Whole::is_weak0`], [`Whole::event_keys`] or
    /// [`Whole::is_mismatch`].
    fn program(&self) -> Option<&Program<'_, 's>>;
    /// The `FileId`s are below this.
    fn files(&self) -> usize;
    /// File `file`'s reader; `None`: a file the view does not hold.
    fn reader(&self, file: FileId) -> Option<Reader<'s>>;
    /// The file's name in a diagnostic.
    fn display(&self, file: FileId) -> FileRef;
    /// The link's target machine.
    fn machine(&self) -> Machine;
    /// Whether the file's bytes can be read (`Inputs::copy_to`).
    fn has_bytes(&self, file: FileId) -> bool;
    /// `Inputs::copy_to`: `range` of the file into `out`.
    ///
    /// # Errors
    /// The store's.
    fn copy_to(&self, file: FileId, range: Range<u64>, out: &mut [u8]) -> Result<(), CopyError>;
    /// The id of section `index` of `file`.
    fn section_id(&self, file: FileId, index: SectionIndex) -> Option<SectionId>;
    /// Section `sid`'s relocation section index (0: none) and its kind.
    fn reloc_of(&self, sid: SectionId) -> (u32, SectionKind);
    /// Whether section `sid` is live.
    fn is_live(&self, sid: SectionId) -> bool;
    /// `spec/resolve.md` §10.1 P-1: whether section `index` of `file` has
    /// a place.
    fn has_place(&self, file: FileId, index: SectionIndex) -> bool;
    /// The keys of the file's non-local symbol-table entries, in order.
    fn globals(&self, file: FileId) -> &[SymbolId];
    /// `spec/model.md` §3.3 `redirect` of key `id` (`id` itself without).
    fn redirect(&self, id: SymbolId) -> SymbolId;
    /// Whether key `id`'s definition is `Def::Undefined`.
    fn def_undefined(&self, id: SymbolId) -> bool;
    /// Whether section `index` of `file` was dropped by COMDAT
    /// deduplication.
    fn dropped_by_comdat(&self, file: FileId, index: SectionIndex) -> bool;
    /// Key `id`'s name and version.
    fn key_name(&self, id: SymbolId) -> Option<(&'s [u8], Option<&'s [u8]>)>;
    /// The per-key facts of `id` (`View::key_facts`, `comdat_only` and
    /// `gnu_warning` included) and whether its definition is undefined.
    /// Asked only when [`Whole::program`] is `None`.
    fn key_facts(&self, id: SymbolId) -> (SymFacts, bool);
    /// Whether key `id`'s kind is `TargetKind::Weak0`. Asked only when
    /// [`Whole::program`] is `None`.
    fn is_weak0(&self, id: SymbolId) -> bool;
    /// `View::event_keys`, sorted. Asked only when [`Whole::program`] is
    /// `None`.
    fn event_keys(&self) -> Vec<SymbolId>;
    /// Whether key `id` is a `comdat-mismatch` key. Asked only when
    /// [`Whole::program`] is `None`.
    fn is_mismatch(&self, id: SymbolId) -> bool;
}

impl<'s> Whole<'s> for Program<'_, 's> {
    #[inline]
    fn program(&self) -> Option<&Program<'_, 's>> {
        Some(self)
    }
    fn files(&self) -> usize {
        self.inputs.files.len() as usize
    }
    #[inline]
    fn reader(&self, file: FileId) -> Option<Reader<'s>> {
        Some(self.inputs.files.get(file)?.reader)
    }
    fn display(&self, file: FileId) -> FileRef {
        self.inputs.files[file].display()
    }
    fn machine(&self) -> Machine {
        self.inputs.target.machine
    }
    #[inline]
    fn has_bytes(&self, file: FileId) -> bool {
        self.inputs.files[file].bytes.is_some()
    }
    #[inline]
    fn copy_to(&self, file: FileId, range: Range<u64>, out: &mut [u8]) -> Result<(), CopyError> {
        self.inputs.copy_to(file, range, out)
    }
    #[inline]
    fn section_id(&self, file: FileId, index: SectionIndex) -> Option<SectionId> {
        self.resolution.section_id(file, index)
    }
    #[inline]
    fn reloc_of(&self, sid: SectionId) -> (u32, SectionKind) {
        let row = self.resolution.sections[sid];
        (row.reloc.0, row.kind)
    }
    #[inline]
    fn is_live(&self, sid: SectionId) -> bool {
        self.liveness.is_live(sid)
    }
    #[inline]
    fn has_place(&self, file: FileId, index: SectionIndex) -> bool {
        self.resolution.section_id(file, index).is_some_and(|s| {
            mortar_core::place::section_place(self.resolution, self.assignment, self.liveness, s)
                .is_ok()
        })
    }
    #[inline]
    fn globals(&self, file: FileId) -> &[SymbolId] {
        self.inputs.sym_map_of(file)
    }
    #[inline]
    fn redirect(&self, id: SymbolId) -> SymbolId {
        self.resolution.redirect.get(id).copied().unwrap_or(id)
    }
    #[inline]
    fn def_undefined(&self, id: SymbolId) -> bool {
        matches!(self.def(id), Def::Undefined { .. })
    }
    fn dropped_by_comdat(&self, file: FileId, index: SectionIndex) -> bool {
        self.resolution
            .section_id(file, index)
            .and_then(|s| self.resolution.sections.get(s))
            .is_some_and(|row| row.state == SectionState::Dropped(DropReason::Comdat))
    }
    #[inline]
    fn key_name(&self, id: SymbolId) -> Option<(&'s [u8], Option<&'s [u8]>)> {
        self.inputs.symbols.get(id).map(|k| (k.name, k.version))
    }
    fn key_facts(&self, _: SymbolId) -> (SymFacts, bool) {
        unreachable!("the whole link's facts are the view's (Whole::program)")
    }
    fn is_weak0(&self, _: SymbolId) -> bool {
        unreachable!("the whole link's facts are the view's (Whole::program)")
    }
    fn event_keys(&self) -> Vec<SymbolId> {
        unreachable!("the whole link's facts are the view's (Whole::program)")
    }
    fn is_mismatch(&self, _: SymbolId) -> bool {
        unreachable!("the whole link's facts are the view's (Whole::program)")
    }
}

/// The scan's view of the link.
pub(crate) struct View<'a, 's, W = Program<'a, 's>> {
    pub whole: W,
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
    /// `closure_only` (D-12), sorted by key.
    pub closure_only: &'a [(SymbolId, FileId)],
    /// WP-177: the owners of `.eh_frame` and `.gcc_except_table` fields.
    pub eh: Option<&'a dyn crate::EhOwners>,
    /// `spec/dyn.md` D-25: the `WeakDyn` keys ([`crate::weak::classes`]);
    /// empty until it ran, and in every static link.
    pub weak_dyn: IdSet<SymbolId>,
    /// The undefined weak keys refused by D-25: the walk drops what the
    /// architecture's scan says of their relocations.
    pub weak_refused: IdSet<SymbolId>,
    /// `spec/dyn.md` D-117's address-level rule (`crate::bound`): per
    /// file, its facts, built on first use; empty (no cost) unless the
    /// output is a `Shared` object of an architecture with a scope
    /// (`crate::bound::Scope`: AArch64, x86-64).
    bound: Vec<OnceLock<Option<crate::bound::FileFacts>>>,
}

/// The two sections whose fields WP-177 judges by their record
/// (`spec/resolve.md` §6.3): exactly `.eh_frame` and exactly
/// `.gcc_except_table`, alloc or not (`mortar_arch::dead_reference`'s
/// `DeadFde` and `Zero`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum EhPlace {
    Frame,
    ExceptTable,
}

impl EhPlace {
    pub fn of(name: &[u8]) -> Option<Self> {
        match name {
            b".eh_frame" => Some(Self::Frame),
            b".gcc_except_table" => Some(Self::ExceptTable),
            _ => None,
        }
    }
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
            whole: Program {
                inputs: t.inputs,
                resolution: t.resolution,
                liveness: t.liveness,
                assignment: t.assignment,
            },
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
            closure_only: t.closure_only,
            eh: t.eh,
            weak_dyn: IdSet::new(),
            weak_refused: IdSet::new(),
            bound: if opts.output == OutputKind::Shared
                && crate::bound::Scope::of(arch.facts().machine).is_some()
            {
                (0..files).map(|_| OnceLock::new()).collect()
            } else {
                Vec::new()
            },
        }
    }

    pub fn def(&self, id: SymbolId) -> Def {
        self.whole.def(id)
    }
}

impl<'a, 's, W: Whole<'s>> View<'a, 's, W> {
    /// The view of an update's narrow `whole` (WP-255d5): no COMDAT
    /// mismatch or warning lists of its own (`whole` answers them), and
    /// none of D-25's or D-117's facts (an update falls back by name
    /// before they could matter: `update-not-implemented (scan)`).
    pub fn narrow(
        arch: &'a dyn Arch,
        whole: W,
        opts: ScanOptions<'a>,
        eh: Option<&'a dyn crate::EhOwners>,
    ) -> Self {
        let files = whole.files();
        Self {
            whole,
            arch,
            opts,
            preps: (0..files).map(|_| OnceLock::new()).collect(),
            mismatch: Vec::new(),
            mismatches: Vec::new(),
            warned: Vec::new(),
            gnu_warnings: Vec::new(),
            closure_only: &[],
            eh,
            weak_dyn: IdSet::new(),
            weak_refused: IdSet::new(),
            bound: Vec::new(),
        }
    }

    /// D-117's facts of object `file` (`crate::bound`): `None` outside a
    /// `Shared` link with a scope, for a file without a preemptible data
    /// definition, and in a [`View::narrow`].
    pub fn bound_facts(&self, file: FileId) -> Option<&crate::bound::FileFacts> {
        let cell = self.bound.get(file.0 as usize)?;
        cell.get_or_init(|| crate::bound::FileFacts::of(self, file)).as_ref()
    }

    pub fn prep(&self, file: FileId) -> Option<&Prep<'s>> {
        let cell = self.preps.get(file.0 as usize)?;
        Some(cell.get_or_init(|| {
            let (obj, symtab) = match self.whole.reader(file) {
                Some(Reader::Object(o)) => (Some(o), o.symbols().map_err(|e| e.to_string())),
                Some(Reader::Shared(s)) => (None, s.symbols().map_err(|e| e.to_string())),
                Some(Reader::Internal) | None => (None, Ok(None)),
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
        match self.whole.program() {
            Some(_) => !self.mismatch.is_empty() && self.mismatch.binary_search(&id).is_ok(),
            None => self.whole.is_mismatch(id),
        }
    }

    /// Whether a record whose symbol is `mention` refers to `id` through
    /// a mention COMDAT deduplication demoted (`spec/resolve.md` §6.2:
    /// defined in a dropped copy of its group) while nothing live
    /// defines `id` (a `comdat-mismatch` key, §6.3 case (b)). Such a
    /// reference is `comdat-mismatch` whatever the mention's binding
    /// (WP-176): the file had a definition, so its reference is not a
    /// weak reference, and the undefined-weak zero would be a wrong
    /// program. Another file's genuine undefined mention of `id` is not
    /// one.
    pub fn is_demoted_mismatch(&self, id: SymbolId, mention: SymEntry) -> bool {
        if !self.is_mismatch(id) || !self.whole.def_undefined(id) {
            return false;
        }
        let Some(table) = self.prep(mention.file).and_then(Prep::table) else { return false };
        let Ok(sym) = table.get(mention.index) else { return false };
        let Ok(SymbolSection::Section(sec)) = table.section_of(mention.index, &sym) else {
            return false;
        };
        self.whole.dropped_by_comdat(mention.file, sec)
    }

    pub fn is_warned(&self, id: SymbolId) -> bool {
        !self.warned.is_empty() && self.warned.binary_search(&id).is_ok()
    }

    /// WP-177 (`spec/resolve.md` §6.3): the owner of the field at
    /// `offset` of section `sid`, an `.eh_frame` or exactly
    /// `.gcc_except_table` ([`EhPlace::of`]); `None` without the tables
    /// (`ScanInputs::eh`).
    pub fn eh_owner(&self, place: EhPlace, sid: SectionId, offset: u64) -> Option<EhOwner> {
        let eh = self.eh?;
        Some(match place {
            EhPlace::Frame => eh.frame(sid, offset),
            EhPlace::ExceptTable => eh.lsda(sid, offset),
        })
    }

    /// Whether section `index` of `file` has a place (`spec/resolve.md`
    /// §10.1 P-1: `Kept`, live, assigned an output section): a symbol
    /// defined in one that has none is `dead` to the scan, whatever the
    /// reason (WP-136). The image asks the same predicate.
    pub fn has_place(&self, file: FileId, index: SectionIndex) -> bool {
        self.whole.has_place(file, index)
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
        let id = *self.whole.globals(file).get((r_sym - first) as usize)?;
        let to = self.whole.redirect(id);
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
    ///
    /// For a global it is the composition of its two steps (WP-239
    /// §13.6): [`View::key_facts`], then [`View::at_mention`].
    pub fn facts(&self, key: SymKey, mention: Option<SymEntry>) -> SymFacts {
        match key.kind() {
            SymKind::Local(e) => self.local_facts(e),
            SymKind::Global(id) => {
                let (f, undefined) = self.key_facts_def(id);
                self.mention_facts(f, undefined, id, mention)
            }
        }
    }

    /// The per-key part of a global's facts: [`View::facts`] with no
    /// mention (`Walker::facts(SymKey::global(id))`). What an incremental
    /// link's state stores, packed (`SymFacts::pack`, `spec/incr.md`
    /// IN-18e `RESOLVED.facts`; WP-239 R-4a).
    pub fn key_facts(&self, id: SymbolId) -> SymFacts {
        self.key_facts_def(id).0
    }

    /// [`View::key_facts`], and whether the key's definition is
    /// `Def::Undefined` (the one case where the mention's own entry
    /// gives the type and `st_other`).
    #[inline]
    fn key_facts_def(&self, id: SymbolId) -> (SymFacts, bool) {
        let Some(p) = self.whole.program() else { return self.whole.key_facts(id) };
        let mut f = self.global_facts(p, id, 0);
        f.comdat_only = self.is_mismatch(id);
        f.gnu_warning = self.is_warned(id);
        (f, matches!(p.def(id), Def::Undefined { .. }))
    }

    /// The facts of global `id` for a record whose symbol is `mention`,
    /// from its per-key facts `f` ([`View::key_facts`]): an undefined key
    /// takes the mention's type and `st_other`; a reference through a
    /// demoted mention is `Undefined` ([`View::is_demoted_mismatch`]).
    /// `None`: `f` itself.
    pub fn at_mention(&self, f: SymFacts, id: SymbolId, mention: Option<SymEntry>) -> SymFacts {
        let undefined = self.whole.def_undefined(id);
        self.mention_facts(f, undefined, id, mention)
    }

    fn mention_facts(
        &self,
        mut f: SymFacts,
        undefined: bool,
        id: SymbolId,
        mention: Option<SymEntry>,
    ) -> SymFacts {
        let Some(m) = mention else { return f };
        if undefined {
            let s = self.sym(m.file, m.index);
            f.sym_type = s.map_or(0, |s| s.sym_type());
            f.st_other = s.map_or(0, |s| s.st_other);
            f.tls = s.is_some_and(|s| s.sym_type() == STT_TLS);
        }
        // WP-176: a reference through a demoted mention of a key nothing
        // live defines is `comdat-mismatch` in every output kind, never
        // the undefined-weak zero nor (in a shared object) an import
        // ([`Self::is_demoted_mismatch`]).
        if self.is_demoted_mismatch(id, m) {
            f.kind = TargetKind::Undefined;
        }
        f
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
        // §6.1: a local of a section of its file that has no place
        // (resolve.md §10.1 P-1: dropped, consumed, or not live; the
        // symbol read once for the kind and for this).
        let dead = e.index.0 != 0
            && matches!(section, Ok(SymbolSection::Section(sec)) if !self.has_place(e.file, sec));
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

    fn global_facts(&self, p: &Program<'_, 's>, id: SymbolId, depth: u32) -> SymFacts {
        let of = |kind, s: Option<Sym>| SymFacts {
            kind,
            sym_type: s.map_or(0, |s| s.sym_type()),
            st_other: s.map_or(0, |s| s.st_other),
            tls: s.is_some_and(|s| s.sym_type() == STT_TLS),
            dead: false,
            comdat_only: false,
            gnu_warning: false,
        };
        let props = p.resolution.props.get(id).copied().unwrap_or_default();
        // A shared definition's merged flag (resolution rule 6), or, in a
        // shared object, the key's D-112 bit (`Resolution.preemptible`,
        // `spec/dyn.md` D-113: the target kind `Preemptible`).
        let preemptible =
            props.refs.contains(RefFlags::PREEMPTIBLE) || p.resolution.preemptible.contains(id);
        match p.def(id) {
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
                // A definition whose section has no place is dead
                // (resolve.md §10.1 P-1, WP-136): dropped by a named rule
                // (`SHF_EXCLUDE`, a consumed or stripped kind), kept but
                // consumed (`.note.gnu.property`), or under
                // `--gc-sections` kept but not reached by the mark (§6.1,
                // `spec/image.md` I-7). A debug section's reference to it
                // gets the tombstone (I-47), any other section's is
                // `discarded-section-reference` (I-12). A global defined
                // in a dropped COMDAT copy is demoted (resolve §6.2), so
                // its `def` is never that copy.
                let dead = match place {
                    Some(SymbolSection::Section(i)) => !self.has_place(e.file, i),
                    _ => false,
                };
                SymFacts { dead, ..of(kind, s) }
            }
            Def::Common(e) if preemptible => of(TargetKind::Preemptible, self.sym(e.file, e.index)),
            Def::Common(e) => of(TargetKind::Local, self.sym(e.file, e.index)),
            Def::Shared(e) => {
                let s = self.sym(e.file, e.index);
                let func = s.is_some_and(|s| matches!(s.sym_type(), STT_FUNC | STT_GNU_IFUNC));
                of(TargetKind::Shlib { func }, s)
            }
            // `--defsym SYM=B` (`spec/resolve.md` §7.2): `B`'s address and
            // type under `SYM`'s own name. A preemptible `SYM` (a
            // `-shared` export) is bound through its own dynamic symbol;
            // otherwise a reference reaches `B`'s definition in this
            // output, even when `B` itself is preemptible (GNU ld, lld).
            // resolve refuses an alias of an undefined, a shared or a
            // `--defsym` symbol, so `B` is a definition of this output.
            Def::Internal(InternalDef::Alias(to)) if to != id && depth < 8 => {
                let b = self.global_facts(p, to, depth + 1);
                let kind = match b.kind {
                    TargetKind::Abs => TargetKind::Abs,
                    _ if preemptible => TargetKind::Preemptible,
                    TargetKind::Preemptible => TargetKind::Local,
                    k => k,
                };
                SymFacts { kind, ..b }
            }
            // `SYM=B+N` (§7.2): section-relative, `STT_NOTYPE`; a number
            // is absolute in every output kind (GNU ld relocates it in a
            // PIE, lld and mold do not: resolve.md §16).
            Def::Internal(InternalDef::Assign(i)) => match p.resolution.assigned(i) {
                Some(Assigned::Offset { to, .. }) if to != id && depth < 8 => {
                    let b = self.global_facts(p, to, depth + 1);
                    let kind = match b.kind {
                        TargetKind::Abs => TargetKind::Abs,
                        _ if preemptible => TargetKind::Preemptible,
                        TargetKind::Undefined => TargetKind::Undefined,
                        _ => TargetKind::Local,
                    };
                    SymFacts { kind, sym_type: 0, st_other: 0, tls: false, ..b }
                }
                _ => of(TargetKind::Abs, None),
            },
            Def::Internal(InternalDef::Alias(_)) => of(TargetKind::Abs, None),
            Def::Internal(InternalDef::Provide { .. }) => {
                let tls = p.inputs.symbols.get(id).is_some_and(|k| k.name == TLS_MODULE_BASE);
                let sym_type = if tls { STT_TLS } else { 0 };
                SymFacts {
                    // `_end` and its kind, exported because a shared
                    // input references it (D-110 item 2), stay
                    // preemptible in a shared object (D-112).
                    kind: if preemptible { TargetKind::Preemptible } else { TargetKind::Local },
                    sym_type,
                    st_other: 0,
                    tls,
                    dead: false,
                    comdat_only: false,
                    gnu_warning: false,
                }
            }
            // The mention's type and `st_other`: `mention_facts`.
            Def::Undefined { weak } => of(self.undefined_kind(id, weak, props.verdict), None),
        }
    }

    /// The keys a reference to which can make an event (§4.4): those
    /// whose target kind is `Undefined` (an undefined key, or an alias of
    /// one), those only dropped COMDAT copies define, and those with a
    /// `.gnu.warning` section; sorted. A record whose key is none of
    /// these, at a place where a dead target is the tombstone, makes no
    /// event (`spec/image.md` I-14: the sections fill checks).
    pub fn event_keys(&self) -> Vec<SymbolId> {
        let Some(p) = self.whole.program() else { return self.whole.event_keys() };
        let mut v: Vec<SymbolId> = Vec::new();
        for i in 0..p.inputs.symbols.len() {
            let id = SymbolId(i);
            let undefined = match p.def(id) {
                Def::Defined(_) | Def::Common(_) | Def::Shared(_) => false,
                Def::Undefined { weak } => {
                    let verdict = p.resolution.props.get(id).map(|p| p.verdict);
                    self.undefined_kind(id, weak, verdict.unwrap_or_default())
                        == TargetKind::Undefined
                }
                Def::Internal(_) => self.global_facts(p, id, 0).kind == TargetKind::Undefined,
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

    /// Whether `facts(key, mention).kind` is `TargetKind::Weak0`, found
    /// from the definitions alone (a local is never `Weak0`); the
    /// mention is read only for a `comdat-mismatch` key, whose demoted
    /// mention makes it `Undefined` (WP-176).
    pub fn is_weak0(&self, key: SymKey, mention: Option<SymEntry>) -> bool {
        let SymKind::Global(mut id) = key.kind() else { return false };
        if mention.is_some_and(|m| self.is_demoted_mismatch(id, m)) {
            return false;
        }
        let Some(p) = self.whole.program() else { return self.whole.is_weak0(id) };
        // As `global_facts`: an alias is followed to depth 8.
        for depth in 0..=8 {
            match p.def(id) {
                Def::Undefined { weak } => {
                    let verdict = p.resolution.props.get(id).copied().unwrap_or_default().verdict;
                    return self.undefined_kind(id, weak, verdict) == TargetKind::Weak0;
                }
                Def::Internal(InternalDef::Alias(to)) if to != id && depth < 8 => id = to,
                _ => return false,
            }
        }
        false
    }

    /// The target kind of an undefined key (§4.4, I-19): a weak one is
    /// zero, or dynamic when D-25 made it so (`weak_dyn`, per key); a
    /// strong one is `Undefined` unless its verdict is "nothing"
    /// (`--unresolved-symbols=ignore-*`), which makes it zero.
    ///
    /// In a shared object (`spec/dyn.md` D-113, D-118) a weak one is
    /// always `WeakDyn`, and a strong one the loader may resolve
    /// (resolution's verdict is not an error: without `-z defs`, and not
    /// hidden) an import of unknown type, `Shlib { func: false }`.
    fn undefined_kind(&self, id: SymbolId, weak: bool, verdict: Verdict) -> TargetKind {
        if self.opts.output == OutputKind::Shared {
            return match (weak, verdict) {
                (true, _) => TargetKind::WeakDyn,
                (false, Verdict::Bound | Verdict::Ignored) => TargetKind::Shlib { func: false },
                _ => TargetKind::Undefined,
            };
        }
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
            SymKind::Global(id) => self.whole.key_name(id).map_or(&[][..], |k| k.0),
            SymKind::Local(e) => {
                let Some(t) = self.prep(e.file).and_then(Prep::table) else { return &[] };
                t.get(e.index).ok().and_then(|s| t.name(&s).ok()).unwrap_or_default()
            }
        }
    }

    /// Whether `key` is a local section symbol (`STT_SECTION`).
    pub fn is_section_symbol(&self, key: SymKey) -> bool {
        matches!(key.kind(), SymKind::Local(e)
            if self.sym(e.file, e.index).is_some_and(|s| s.sym_type() == STT_SECTION))
    }

    /// The name a diagnostic shows for `key`: the symbol's name with its
    /// version, or a section symbol's section name.
    pub fn shown_name(&self, key: SymKey) -> String {
        match key.kind() {
            SymKind::Global(id) => {
                let Some((name, version)) = self.whole.key_name(id) else { return String::new() };
                let name = String::from_utf8_lossy(name);
                match version {
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
        self.whole.section_id(file, SectionIndex(index))
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
fn copy<'s, W: Whole<'s>>(
    view: &View<'_, 's, W>,
    file: FileId,
    offset: u64,
    size: u64,
    buf: &mut Vec<u8>,
) -> Result<(), ReadFail> {
    if !view.whole.has_bytes(file) {
        return Err(ReadFail::Internal(format!(
            "scan: {size} bytes at offset {offset} of file {} are not resident and it has no store entry",
            file.0
        )));
    }
    let len = usize::try_from(size).map_err(|_| ReadFail::Copy(CopyError::OutOfRange))?;
    buf.clear();
    buf.resize(len, 0);
    let end = offset.checked_add(size).ok_or(ReadFail::Copy(CopyError::OutOfRange))?;
    view.whole.copy_to(file, offset..end, buf).map_err(ReadFail::Copy)
}

/// The contents of section `s` of `file`: the resident slice, or one
/// `copy_to` into `buf` when they are not resident (`spec/input.md`
/// §11.3).
pub(crate) fn contents<'b, 's: 'b, W: Whole<'s>>(
    view: &View<'_, 's, W>,
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
pub(crate) fn rec_table<'b, 's: 'b, W: Whole<'s>>(
    view: &View<'_, 's, W>,
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
pub(crate) fn rec_offset<'s, W: Whole<'s>>(
    view: &View<'_, 's, W>,
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
pub(crate) struct Recs<'v, 'a, 's, 'b, W = Program<'a, 's>> {
    pub view: &'v View<'a, 's, W>,
    pub file: FileId,
    pub table: Option<&'v SymbolTable<'s>>,
    pub records: RecTable<'b>,
    /// The contents of the section the records apply to: resident, or
    /// copied into the worker's [`Bufs`].
    pub bytes: &'b [u8],
}

impl<'s, W: Whole<'s>> Recs<'_, '_, 's, '_, W> {
    /// The addend of a record: explicit, or read from the field (REL).
    pub fn addend(&self, raw: &Raw, h: &mortar_arch::Howto) -> i64 {
        raw.addend.unwrap_or_else(|| {
            let from = usize::try_from(raw.r_offset).unwrap_or(usize::MAX);
            self.view.arch.read_addend(h, self.bytes.get(from..).unwrap_or_default())
        })
    }
}

impl<'s, W: Whole<'s>> Records for Recs<'_, '_, 's, '_, W> {
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
pub(crate) struct RecTables<'v, 'a, 's, W = Program<'a, 's>> {
    pub view: &'v View<'a, 's, W>,
    pub key: SymKey,
    pub mention: SymEntry,
}

impl<'s, W: Whole<'s>> SymNames for RecTables<'_, '_, 's, W> {
    fn name(&self, key: SymKey) -> &[u8] {
        self.view.name(key)
    }
}

impl<'s, W: Whole<'s>> ScanTables for RecTables<'_, '_, 's, W> {
    fn sym(&self, key: SymKey) -> SymFacts {
        self.view.facts(key, (key == self.key).then_some(self.mention))
    }
}
