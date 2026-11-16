//! `Resolution` (`spec/model.md` §3.3): which definition wins, which files
//! take part, which sections exist and are kept. Written by
//! `mortar_resolve`.

use mortar_elf::SectionIndex;

use crate::column::Column;
use crate::flags::flag_set;
use crate::id::{ComdatId, FileId, Id, IdRange, OptId, SectionId, SymEntry, SymbolId};
use crate::idset::IdSet;
use crate::inputs::{FileKind, InputFlags, Inputs, Origin};
use crate::table::{Broken, ensure, unsorted_at};

/// Whether a file takes part in the link, and why (§3.3 `file_live`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FileLive {
    /// Not in the link: a lazy object never extracted; a loaded shared
    /// object that is neither needed nor used (`spec/dyn.md` D-12: an
    /// as-needed object nothing makes needed, a closure file no needed or
    /// used object leads to).
    Dead,
    /// Always in the link: the internal file, an object named directly, a
    /// `--whole-archive` member.
    Always,
    /// A lazy object extracted by the reference `(FileId, st_index)`; by a
    /// root (`-u`, `--require-defined`, the entry: `spec/resolve.md` §4.2),
    /// `file` is 0 and `index` the root's argument index (0 for the
    /// default entry).
    Extracted(SymEntry),
    /// A loaded shared object that takes part in the link (`spec/dyn.md`
    /// D-11, D-12; request R-M1). Exactly one flag is set:
    ///
    /// - `needed`: a member of N (D-11), `DT_NEEDED` of the output, in
    ///   `Slots.needed` (D-19a order). Only a shared object of the command
    ///   line or a script (C) is ever needed;
    /// - `used`: a member of U = reach(N) \ N (D-12): reached by a
    ///   `DT_NEEDED` edge from a needed or used object, whatever loaded it
    ///   (the command line, a script's `AS_NEEDED`, the closure walk of
    ///   `spec/input.md` §8.6). Never `DT_NEEDED`; its definitions satisfy
    ///   the shlib-undefined check (D-13), its references and definitions
    ///   count for `ref_dynamic`/`defined_dynamic`.
    ///
    /// Every other loaded shared object is `Dead`. Computed by
    /// `mortar_resolve` (WP-M2-RES) after the needed fixed point; a static
    /// link has none.
    Shared { needed: bool, used: bool },
}

impl FileLive {
    #[must_use]
    pub fn is_live(self) -> bool {
        self != Self::Dead
    }

    /// A needed shared object (D-11): `DT_NEEDED` of the output.
    #[must_use]
    pub fn is_needed(self) -> bool {
        matches!(self, Self::Shared { needed: true, .. })
    }
}

/// How the linker defines a key it defines (§3.3 `def`: `Internal`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InternalDef {
    /// An assignment of rank 0 (`spec/resolve.md` §3.1, §7.2): the
    /// argument index of the `--defsym`, or a script assignment's position
    /// (M4).
    Assign(u32),
    /// `--defsym SYM=B`: an alias of `B` (`spec/resolve.md` §7.2).
    Alias(SymbolId),
    /// A linker-defined symbol of `spec/resolve.md` §8.1 (rank 5).
    /// `conditional` for the ones whose existence depends on output
    /// sections (`__start_`/`__stop_<sec>`, `_TLS_MODULE_BASE_`,
    /// `__GNU_EH_FRAME_HDR`): `Assignment` confirms or withdraws them
    /// (§3.4).
    Provide { conditional: bool },
}

/// The resolved definition of a key (§3.3 `def`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Def {
    /// Nothing defines the key.
    Undefined { weak: bool },
    /// A regular definition.
    Defined(SymEntry),
    /// A common symbol; its allocation is a `sym_place` row (§4.4).
    Common(SymEntry),
    /// A definition in a shared object: the entry of `bind(k)`
    /// (`spec/dyn.md` D-11), the first member of N ∪ U in loader order
    /// with a visible definition at the version the key asks for, never
    /// `giver(k)` (which decides neededness only). Every fact of a shared
    /// definition (provider, `COPY` size and alignment, version,
    /// `STV_PROTECTED`, `STT_GNU_IFUNC`, `STT_TLS`) is read from this
    /// entry; [`Resolution::bind`].
    Shared(SymEntry),
    /// Linker-defined (`_end`, `__start_<name>`, `--defsym`, script
    /// assignments): its value is a `defined` row (§4.4).
    Internal(InternalDef),
}

impl Def {
    /// The entry that defines the key, if a file does.
    #[must_use]
    pub fn entry(self) -> Option<SymEntry> {
        match self {
            Self::Defined(e) | Self::Common(e) | Self::Shared(e) => Some(e),
            Self::Undefined { .. } | Self::Internal(_) => None,
        }
    }
}

flag_set! {
    /// The merged properties of a key that are unions over its live
    /// mentions (§3.3; `spec/resolve.md` §3.6, §9.6).
    pub struct RefFlags: u8 {
        /// `ref_regular`: referenced from a live relocatable object, or a
        /// root names it.
        const FROM_OBJECT = 1 << 0;
        /// `ref_dynamic`: a needed shared object has an undefined mention.
        const FROM_SHARED = 1 << 1;
        /// `ref_strong`: some reference is not weak. An undefined key with
        /// it is `Undefined { weak: false }`.
        const STRONG = 1 << 2;
        /// `defined_dynamic`: a live shared object defines the key.
        const DEFINED_SHARED = 1 << 3;
        /// The key goes in `.dynsym` as a definition of the output
        /// (`spec/resolve.md` §9.6).
        const EXPORTED = 1 << 4;
        /// The definition can be preempted at run time; always for a
        /// `Shared` definition (`spec/resolve.md` §9.6).
        const PREEMPTIBLE = 1 << 5;
        /// A live reference in a relocatable object (an undefined or
        /// demoted mention) has bit 7 of `st_other`, AArch64's
        /// `STO_AARCH64_VARIANT_PCS`: an undefined key's `.symtab` entry
        /// carries it (`spec/resolve.md` §3.7).
        const VARIANT_PCS = 1 << 6;
    }
}

/// What a surviving reference to the key gets (`spec/resolve.md` §5.2),
/// decided by resolution and reported by the scan.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
#[repr(u8)]
pub enum Verdict {
    /// Nothing to report: the key is defined, or every reference is weak
    /// (a weak undefined is zero).
    #[default]
    Bound,
    /// `undefined-symbol`, an error.
    Error,
    /// `undefined-symbol`, a warning (`--warn-unresolved-symbols`).
    Warning,
    /// Nothing, the value zero (`--unresolved-symbols=ignore-*`,
    /// `--ignore-unresolved-symbol`).
    Ignored,
}

/// The properties merged over every live mention of a key (§3.3 `def`,
/// second half). Kept in a column beside `def` (§9: a phase reads the
/// columns it needs).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct SymProps {
    /// `STV_*`, the most constraining.
    pub visibility: u8,
    /// `STB_*` for export.
    pub binding: u8,
    pub refs: RefFlags,
    /// What a surviving reference gets.
    pub verdict: Verdict,
    /// The version the definition carries: an index whose meaning is
    /// `spec/resolve.md`'s (0: none).
    pub version: u16,
}

/// The size and alignment of a common allocation (§3.3 `common`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CommonShape {
    pub size: u64,
    /// log2 of the alignment.
    pub align: u8,
    /// x86-64: every common mention of the key is `SHN_X86_64_LCOMMON`
    /// (§3.3, `spec/x64.md` X-55), so it is allocated in `.lbss`
    /// (`spec/layout.md` L-88); a key with one `SHN_COMMON` mention is a
    /// small common. Always `false` on another machine.
    pub large: bool,
}

/// How a section is handled, classified once (§3.3 `kind`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[repr(u8)]
pub enum SectionKind {
    /// Bytes copied and relocated.
    Regular,
    /// `SHT_NOBITS`.
    Nobits,
    /// `SHF_MERGE`: strings or fixed-size constants, split into pieces.
    Merge,
    /// `.eh_frame`, split into CIE and FDE records.
    EhFrame,
    /// `SHT_NOTE`.
    Note,
    /// Non-alloc, handled as data (`.debug_*`).
    Debug,
    // -- no output of their own ---------------------------------------
    /// Section 0.
    Null,
    Symtab,
    Strtab,
    /// `SHT_REL`, `SHT_RELA`.
    Reloc,
    /// `SHT_GROUP`.
    Group,
    /// `.note.GNU-stack`.
    Stack,
    /// `SHF_EXCLUDE`.
    Excluded,
    /// A Mason metadata container (`spec/meta.md` MT-6): read by the
    /// checks, never output (`Dropped(Consumed)`).
    Meta,
}

impl SectionKind {
    /// Whether the kind carries output of its own.
    #[must_use]
    pub const fn carries_output(self) -> bool {
        !matches!(
            self,
            Self::Null
                | Self::Symtab
                | Self::Strtab
                | Self::Reloc
                | Self::Group
                | Self::Stack
                | Self::Excluded
                | Self::Meta
        )
    }

    /// Whether a live section of this kind is placed as a whole (§4.4
    /// `place`). `Merge` and `EhFrame` sections are not: their pieces are
    /// (`Split`, `MergeMap`, the `.eh_frame` chunk of `Slots`).
    #[must_use]
    pub const fn is_placed(self) -> bool {
        matches!(self, Self::Regular | Self::Nobits | Self::Note | Self::Debug)
    }
}

/// Why a section is not kept (§3.3 `state`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[repr(u8)]
pub enum DropReason {
    /// Its COMDAT group is not kept; the keeping file is `comdat` of the
    /// section's group.
    Comdat,
    /// `SHF_EXCLUDE`.
    Excluded,
    /// A kind without output: the linker consumes it.
    Consumed,
    /// Stripped by an option.
    Stripped,
    /// A script's `/DISCARD/` (§3.4).
    Script,
}

/// Whether a section is kept (§3.3 `state`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum SectionState {
    Kept,
    Dropped(DropReason),
}

/// One section row (§3.3 `sections[SectionId]`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SectionRow {
    pub kind: SectionKind,
    pub state: SectionState,
    /// The relocation section applying to it, by index in its file;
    /// `SectionIndex(0)` for none (section 0 is never one). A second
    /// relocation section for one section is refused by the phase that
    /// finds it.
    pub reloc: SectionIndex,
    /// Its COMDAT group's signature, if it is in one.
    pub comdat: OptId<ComdatId>,
}

impl SectionRow {
    /// Whether the section is kept and carries output.
    #[must_use]
    pub fn is_kept(&self) -> bool {
        self.state == SectionState::Kept
    }
}

/// The table (§3.3).
#[derive(Debug, Clone, Default)]
pub struct Resolution {
    pub file_live: Column<FileId, FileLive>,
    pub def: Column<SymbolId, Def>,
    pub props: Column<SymbolId, SymProps>,
    /// The keys with a common mention, sorted by id, with the maximum size
    /// and alignment over their live common mentions.
    pub common: Box<[(SymbolId, CommonShape)]>,
    /// Whether two or more live definitions tie at the strongest rank.
    pub clash: Column<SymbolId, bool>,
    /// The row the key's **undefined** mentions bind to (`--wrap`); for a
    /// `--defsym` alias, every mention. A defined mention binds to its own
    /// row. Itself otherwise; a consumer applies it once.
    pub redirect: Column<SymbolId, SymbolId>,
    /// The file whose copy of the group is kept: the least live `FileId`
    /// with the signature (`spec/resolve.md` §6.1); none if no live file
    /// has it.
    pub comdat: Column<ComdatId, OptId<FileId>>,
    /// The live copies of every group, sorted by `(ComdatId, FileId)`,
    /// for `mortar_odr` (`spec/resolve.md` §6.7). The keeper is the first
    /// of each group.
    pub comdat_copies: Box<[(ComdatId, FileId)]>,
    /// Per file, its section ids: `section_base` and `e_shnum` (§2.2). Empty
    /// for files that are not live objects.
    pub section_base: Column<FileId, IdRange<SectionId>>,
    pub sections: Column<SectionId, SectionRow>,
    /// The Mason containers of the live objects (`spec/model.md` §3.3,
    /// `spec/meta.md` MT-6: every section of kind `Meta`), sorted by
    /// `(FileId, SectionIndex)`, so that `mortar_odr` finds a file's units
    /// without a rescan. Empty in a link without containers.
    pub meta: Box<[(FileId, SectionIndex)]>,
    // ---- the dynamic columns (`spec/dyn.md` D-11, D-12, D-21; request
    // R-R3): computed by `mortar_resolve` after the needed/used fixed
    // point (D-11, D-12). Each is empty (no room, `IdSet::new`) in a
    // static link, else has one bit per key. The `WeakDyn` decision is
    // not here: it is the scan's per-key column (`spec/dyn.md` D-25,
    // `mortar_scan::weak`).
    /// The program definitions in `.dynsym` (D-21 item 2; resolve §9.6):
    /// a key defined by the program (`Defined`, `Common`, `Internal`)
    /// that `ref_dynamic` names, or that `--export-dynamic`/
    /// `--export-dynamic-symbol` exports; a linker-defined key of file 0
    /// only when `ref_dynamic` (request R-R5).
    pub exported: IdSet<SymbolId>,
    /// The imports (D-21 item 1): a key whose `def` is `Shared` (from a
    /// needed object, or a used one through a weak reference, D-11) and
    /// that a live relocatable object mentions (`ref_regular`).
    pub imported: IdSet<SymbolId>,
    /// `ref_dynamic` (D-12): some member of N ∪ U references the key
    /// (strong or weak). Objects outside N ∪ U never count.
    pub ref_dynamic: IdSet<SymbolId>,
    /// `defined_dynamic` (D-12): some member of N ∪ U defines the key.
    pub defined_dynamic: IdSet<SymbolId>,
}

impl Resolution {
    /// A table for `files` files, `symbols` keys and `comdats` signatures,
    /// with no file live, every key undefined and no sections.
    #[must_use]
    pub fn empty(files: u32, symbols: u32, comdats: u32) -> Self {
        Self {
            file_live: Column::filled(files, FileLive::Dead),
            def: Column::filled(symbols, Def::Undefined { weak: false }),
            props: Column::filled(symbols, SymProps::default()),
            common: Box::new([]),
            clash: Column::filled(symbols, false),
            redirect: Column::from_vec((0..symbols).map(SymbolId).collect()).unwrap_or_default(),
            comdat: Column::filled(comdats, OptId::NONE),
            comdat_copies: Box::new([]),
            section_base: Column::filled(files, IdRange::empty(SectionId(0))),
            sections: Column::new(),
            meta: Box::new([]),
            exported: IdSet::new(),
            imported: IdSet::new(),
            ref_dynamic: IdSet::new(),
            defined_dynamic: IdSet::new(),
        }
    }

    /// `bind(k)` (`spec/dyn.md` D-11): the shared definition key `k` binds
    /// to, when its `def` is `Shared`. `None` for a key the program
    /// defines or nothing defines.
    #[must_use]
    pub fn bind(&self, k: SymbolId) -> Option<SymEntry> {
        match self.def.get(k)? {
            Def::Shared(e) => Some(*e),
            _ => None,
        }
    }

    /// The four dynamic columns by name, for checks and dumps.
    #[must_use]
    pub fn dynamic_columns(&self) -> [(&'static str, &IdSet<SymbolId>); 4] {
        [
            ("exported", &self.exported),
            ("imported", &self.imported),
            ("ref_dynamic", &self.ref_dynamic),
            ("defined_dynamic", &self.defined_dynamic),
        ]
    }

    /// The file a section belongs to.
    #[must_use]
    pub fn file_of(&self, section: SectionId) -> Option<FileId> {
        let ranges = self.section_base.as_slice();
        let after = ranges.partition_point(|range| range.end <= section);
        ranges.get(after).filter(|range| range.contains(section)).map(|_| FileId(after as u32))
    }

    /// The section's index in its file's section header table (§2.2).
    #[must_use]
    pub fn sh_index(&self, section: SectionId) -> Option<(FileId, SectionIndex)> {
        let file = self.file_of(section)?;
        let base = self.section_base[file].start;
        Some((file, SectionIndex(section.0 - base.0)))
    }

    /// The id of section `index` of `file`, if the file has one.
    #[must_use]
    pub fn section_id(&self, file: FileId, index: SectionIndex) -> Option<SectionId> {
        self.section_base.get(file)?.nth(index.0)
    }

    /// The common shape of a key, if it has a common mention.
    #[must_use]
    pub fn common_of(&self, symbol: SymbolId) -> Option<CommonShape> {
        let at = self.common.binary_search_by_key(&symbol, |row| row.0).ok()?;
        Some(self.common[at].1)
    }

    /// The live copies of a group, in `FileId` order; the keeper first.
    #[must_use]
    pub fn copies_of(&self, group: ComdatId) -> &[(ComdatId, FileId)] {
        let start = self.comdat_copies.partition_point(|row| row.0 < group);
        let end = self.comdat_copies.partition_point(|row| row.0 <= group);
        &self.comdat_copies[start..end]
    }

    /// The invariants of §2.2 and §3.3 that the table alone shows:
    ///
    /// 1. the per-key columns have one row per key, the per-file columns
    ///    one per file;
    /// 2. section ranges are contiguous in `FileId` order (§2.2) and cover
    ///    `sections`; only live files have sections;
    /// 3. definitions, extracting references and kept groups name live
    ///    files in range; `redirect` and aliases stay in range; `common` is
    ///    sorted;
    /// 4. a section of a kind without output is not `Kept`; a section
    ///    dropped for its group is in one;
    /// 5. `comdat_copies` is sorted, names live files and groups in range,
    ///    and the keeper of a group is its least copy (none without one);
    /// 6. an undefined key with a strong reference is not weak; a `Shared`
    ///    definition is preemptible;
    /// 7. each dynamic column (`exported`, `imported`, `ref_dynamic`,
    ///    `defined_dynamic`) is empty or has one bit per key; an import's
    ///    `def` is `Shared`; an export's is the program's (`Defined`,
    ///    `Common` or `Internal`);
    /// 8. `meta` is strictly sorted and lists exactly the sections of kind
    ///    `Meta`.
    ///
    /// [`Resolution::check_against`] adds what needs `Inputs`.
    #[allow(clippy::too_many_lines)] // the rules in order, one pass over sections
    pub fn check(&self) -> Result<(), Broken> {
        let t = "Resolution";
        let keys = self.def.len();
        let files = self.file_live.len();
        ensure(
            self.props.len() == keys && self.clash.len() == keys && self.redirect.len() == keys,
            t,
            || "per-key columns differ in length".to_owned(),
        )?;
        ensure(self.section_base.len() == files, t, || "section_base is not per file".to_owned())?;
        let live = |file: FileId| self.file_live.get(file).is_some_and(|l| l.is_live());
        let mut next = SectionId(0);
        for (file, range) in self.section_base.iter() {
            ensure(range.start == next, t, || {
                format!("file {}: section range not contiguous", file.0)
            })?;
            ensure(range.is_empty() || live(file), t, || {
                format!("file {}: sections of a dead file", file.0)
            })?;
            next = range.end;
        }
        ensure(next.0 == self.sections.len(), t, || {
            "section ranges do not cover sections".to_owned()
        })?;
        for (file, why) in self.file_live.iter() {
            if let FileLive::Extracted(by) = why {
                ensure(live(by.file), t, || format!("file {}: extracted by a dead file", file.0))?;
            }
        }
        for (symbol, def) in self.def.iter() {
            if let Some(entry) = def.entry() {
                ensure(live(entry.file), t, || {
                    format!("symbol {}: defined in dead or unknown file {}", symbol.0, entry.file.0)
                })?;
            }
            let to = self.redirect[symbol];
            ensure(to.index() < keys, t, || format!("symbol {}: redirect out of range", symbol.0))?;
            if let Def::Internal(InternalDef::Alias(to)) = def {
                ensure(to.index() < keys, t, || {
                    format!("symbol {}: alias out of range", symbol.0)
                })?;
            }
            let refs = self.props[symbol].refs;
            if let Def::Undefined { weak: true } = def {
                ensure(!refs.contains(RefFlags::STRONG), t, || {
                    format!("rule 6: symbol {}: weak undefined with a strong reference", symbol.0)
                })?;
            }
            if let Def::Shared(_) = def {
                ensure(refs.contains(RefFlags::PREEMPTIBLE), t, || {
                    format!(
                        "rule 6: symbol {}: a shared definition that is not preemptible",
                        symbol.0
                    )
                })?;
            }
        }
        self.check_dynamic_columns(keys)?;
        if let Some(at) = unsorted_at(&self.common, |row| row.0) {
            return Err(Broken { table: t, what: format!("common[{at}] out of order") });
        }
        ensure(self.common.iter().all(|row| row.0.index() < keys), t, || {
            "common key out of range".to_owned()
        })?;
        for (group, keeper) in self.comdat.iter() {
            ensure(keeper.get().is_none_or(live), t, || {
                format!("comdat {}: kept by a dead file", group.0)
            })?;
        }
        if let Some(at) = unsorted_at(&self.comdat_copies, |row| *row) {
            return Err(Broken {
                table: t,
                what: format!("rule 5: comdat_copies[{at}] out of order"),
            });
        }
        for &(group, file) in &self.comdat_copies {
            ensure(self.comdat.has(group) && live(file), t, || {
                format!("rule 5: comdat {}: a copy in a dead file or out of range", group.0)
            })?;
        }
        for (group, keeper) in self.comdat.iter() {
            let least = self.copies_of(group).first().map(|row| row.1);
            ensure(keeper.get() == least, t, || {
                format!("rule 5: comdat {}: the keeper is not the least live copy", group.0)
            })?;
        }
        let mut metas = 0usize;
        for (section, row) in self.sections.iter() {
            metas += usize::from(row.kind == SectionKind::Meta);
            ensure(row.kind.carries_output() || !row.is_kept(), t, || {
                format!("section {}: a kind without output is kept", section.0)
            })?;
            ensure(row.comdat.get().is_none_or(|c| self.comdat.has(c)), t, || {
                format!("section {}: comdat out of range", section.0)
            })?;
            ensure(
                row.state != SectionState::Dropped(DropReason::Comdat) || row.comdat.is_some(),
                t,
                || format!("section {}: dropped for a group it is not in", section.0),
            )?;
        }
        self.check_meta(metas)
    }

    /// Rule 8 of [`Resolution::check`]: `meta` (`spec/meta.md` MT-6);
    /// `metas` is the number of `Meta` sections.
    fn check_meta(&self, metas: usize) -> Result<(), Broken> {
        let t = "Resolution";
        if let Some(at) = unsorted_at(&self.meta, |row| *row) {
            return Err(Broken { table: t, what: format!("rule 8: meta[{at}] out of order") });
        }
        for &(file, index) in &self.meta {
            let kind = self.section_id(file, index).and_then(|id| self.sections.get(id));
            ensure(kind.is_some_and(|row| row.kind == SectionKind::Meta), t, || {
                format!("rule 8: file {} section {}: in meta, not a Meta section", file.0, index.0)
            })?;
        }
        ensure(metas == self.meta.len(), t, || {
            "rule 8: a Meta section missing from meta".to_owned()
        })
    }

    /// Rule 7 of [`Resolution::check`]: the dynamic columns (`spec/dyn.md`
    /// D-12, D-21).
    fn check_dynamic_columns(&self, keys: u32) -> Result<(), Broken> {
        let t = "Resolution";
        for (name, set) in self.dynamic_columns() {
            ensure(set.is_empty() || set.len() == keys, t, || {
                format!("rule 7: {name} is neither empty nor per key")
            })?;
        }
        for k in self.imported.iter() {
            ensure(matches!(self.def.get(k), Some(Def::Shared(_))), t, || {
                format!("rule 7: symbol {}: imported without a shared definition", k.0)
            })?;
        }
        for k in self.exported.iter() {
            let ours = matches!(
                self.def.get(k),
                Some(Def::Defined(_) | Def::Common(_) | Def::Internal(_))
            );
            ensure(ours, t, || {
                format!("rule 7: symbol {}: exported without a program definition", k.0)
            })?;
        }
        Ok(())
    }

    /// The invariants of §3.3 that need `Inputs` beside the table:
    ///
    /// 1. one row per file, key and signature of `inputs`;
    /// 2. `Shared` liveness only for a shared object, with exactly one of
    ///    `needed` and `used`, and `needed` only for one not of the closure
    ///    (`spec/dyn.md` D-12: a closure file is at most used); `Extracted`
    ///    only for a lazy object; `Always` for a lazy object only under
    ///    `--whole-archive`;
    /// 3. a `Defined` or `Common` definition is in an object, a `Shared`
    ///    one in a shared object.
    pub fn check_against(&self, inputs: &Inputs<'_>) -> Result<(), Broken> {
        let t = "Resolution";
        ensure(
            self.file_live.len() == inputs.files.len()
                && self.def.len() == inputs.symbols.len()
                && self.comdat.len() == inputs.comdats.len(),
            t,
            || "rule 1: not one row per file, key and signature of Inputs".to_owned(),
        )?;
        for (id, live) in self.file_live.iter() {
            let file = &inputs.files[id];
            let shared = file.kind() == FileKind::Shared;
            let closure = matches!(file.origin, Origin::Closure { .. });
            let ok = match live {
                FileLive::Dead => true,
                FileLive::Always => {
                    !file.is_lazy() || file.state.contains(InputFlags::WHOLE_ARCHIVE)
                }
                FileLive::Extracted(_) => file.is_lazy(),
                FileLive::Shared { needed, used } => {
                    shared && needed != used && !(*needed && closure)
                }
            };
            ensure(ok, t, || format!("rule 2: file {}: {live:?} for this kind of file", id.0))?;
        }
        for (symbol, def) in self.def.iter() {
            let kind = |e: SymEntry| inputs.files.get(e.file).map(crate::inputs::File::kind);
            let ok = match *def {
                Def::Defined(e) | Def::Common(e) => kind(e) == Some(FileKind::Object),
                Def::Shared(e) => kind(e) == Some(FileKind::Shared),
                Def::Undefined { .. } | Def::Internal(_) => true,
            };
            ensure(ok, t, || {
                format!("rule 3: symbol {}: defined in a file of the wrong kind", symbol.0)
            })?;
        }
        Ok(())
    }
}

const _: () = assert!(size_of::<FileLive>() == 12);
const _: () = assert!(size_of::<Def>() == 12);
const _: () = assert!(size_of::<SymProps>() == 6);
const _: () = assert!(size_of::<SectionRow>() == 12);
const _: () = assert!(size_of::<(SymbolId, CommonShape)>() == 24);
