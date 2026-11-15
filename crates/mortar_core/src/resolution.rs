//! `Resolution` (`spec/model.md` §3.3): which definition wins, which files
//! take part, which sections exist and are kept. Written by
//! `mortar_resolve`.

use mortar_elf::SectionIndex;

use crate::column::Column;
use crate::flags::flag_set;
use crate::id::{ComdatId, FileId, Id, IdRange, OptId, SectionId, SymEntry, SymbolId};
use crate::table::{Broken, ensure, unsorted_at};

/// Whether a file takes part in the link, and why (§3.3 `file_live`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FileLive {
    /// Not in the link: an archive member never extracted, a shared object
    /// not needed.
    Dead,
    /// Always in the link: the internal file, an object named directly, a
    /// `--whole-archive` member.
    Always,
    /// An archive member extracted by the reference `(FileId, st_index)`.
    Extracted(SymEntry),
    /// A shared object that is needed.
    Needed,
}

impl FileLive {
    #[must_use]
    pub fn is_live(self) -> bool {
        self != Self::Dead
    }
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
    /// A definition in a shared object.
    Shared(SymEntry),
    /// Linker-defined (`_end`, `__start_<name>`, `--defsym`, script
    /// assignments): its value is a `defined` row (§4.4).
    Internal,
}

impl Def {
    /// The entry that defines the key, if a file does.
    #[must_use]
    pub fn entry(self) -> Option<SymEntry> {
        match self {
            Self::Defined(e) | Self::Common(e) | Self::Shared(e) => Some(e),
            Self::Undefined { .. } | Self::Internal => None,
        }
    }
}

flag_set! {
    /// Where a key is referenced from (§3.3, the merged properties).
    pub struct RefFlags: u8 {
        /// Referenced from a relocatable object.
        const FROM_OBJECT = 1 << 0;
        /// Referenced from a shared object.
        const FROM_SHARED = 1 << 1;
    }
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
    /// Mason records (`spec/meta.md`).
    Metadata,
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
        )
    }

    /// Whether a live section of this kind is placed as a whole (§4.4
    /// `place`). `Merge` and `EhFrame` sections are not: their pieces are
    /// (`Split`, `MergeMap`, the `.eh_frame` chunk of `Slots`).
    #[must_use]
    pub const fn is_placed(self) -> bool {
        matches!(self, Self::Regular | Self::Nobits | Self::Note | Self::Debug | Self::Metadata)
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
    /// The row references to the key bind to; itself otherwise. Every
    /// consumer of `sym_map` applies it once.
    pub redirect: Column<SymbolId, SymbolId>,
    /// The file whose group is kept, if a live file has the group.
    pub comdat: Column<ComdatId, OptId<FileId>>,
    /// Per file, its section ids: `section_base` and `e_shnum` (§2.2). Empty
    /// for files that are not live objects.
    pub section_base: Column<FileId, IdRange<SectionId>>,
    pub sections: Column<SectionId, SectionRow>,
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
            section_base: Column::filled(files, IdRange::empty(SectionId(0))),
            sections: Column::new(),
        }
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

    /// The invariants of §2.2 and §3.3 that the table alone shows:
    ///
    /// - the per-key columns have one row per key, the per-file columns one
    ///   per file;
    /// - section ranges are contiguous in `FileId` order (§2.2) and cover
    ///   `sections`; only live files have sections;
    /// - definitions, extracting references and kept groups name live files
    ///   in range; `redirect` stays in range; `common` is sorted;
    /// - a section of a kind without output is not `Kept`; a section
    ///   dropped for its group is in one.
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
        }
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
        for (section, row) in self.sections.iter() {
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
        Ok(())
    }
}

const _: () = assert!(size_of::<FileLive>() == 12);
const _: () = assert!(size_of::<Def>() == 12);
const _: () = assert!(size_of::<SymProps>() == 6);
const _: () = assert!(size_of::<SectionRow>() == 12);
const _: () = assert!(size_of::<(SymbolId, CommonShape)>() == 24);
