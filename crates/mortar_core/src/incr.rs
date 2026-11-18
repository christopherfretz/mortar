//! The incremental update's plain-data types that cross crates
//! (`spec/incr.md` §5.10; WP-213, package U-1 of the cut,
//! `spec/landed/incr/WP-211.md` §C). `mortar_layout`, `mortar_synth` and
//! `mortar_image` do not depend on `mortar_incr`, so what they take from or
//! give to the update lives here. No algorithm: every behaviour is the
//! package's that the type's text names.
//!
//! **The update's id space** (WP-213; every package codes against it). In an
//! update every [`FileId`], [`SymbolId`], [`SectionId`], [`ChunkId`] and
//! [`OutSecId`] at a seam is a **row of the state** (`spec/incr.md` IN-18e):
//! a `FILES`, `SYMBOLS`, `SECTIONS`, `CHUNKS` or `OUTSECS` row. An id at or
//! past the table's row count (as the state was opened, before the patch)
//! is **new to the update**, numbered in this order and nowhere else:
//! - a new `FileId` (an appeared file, or a member newly given a row) is
//!   `FILES`' count plus its position in `ChangedFiles`' list of new files
//!   (`mortar_incr::update::view`); U-2 appends their `FILES` rows in that
//!   order, so the id is the row the patched state gives it;
//! - a new `SymbolId` (a key no `SYMBOLS` row names) is `SYMBOLS`' count
//!   plus its position in `ResolveDelta`'s list of new keys
//!   (`mortar_incr::update::resolve`, U-3); U-2 appends in that order;
//! - a new `SectionId` (every section of a changed, appeared or newly
//!   extracted file as its new bytes give it: the old version's rows are
//!   that file's **old** sections, gone for the update) is `SECTIONS`'
//!   count plus its position in `ChangedFiles`' list of new sections (files
//!   in that list's order, sections in input order). U-2's patch writes
//!   them into the file's range (IN-18e: rewritten in place when the count
//!   fits its capacity, else a new range in the append area) and maps each
//!   new id to the row it gets; no other package renumbers;
//! - `ChunkId` and `OutSecId` have no new ids: an update that would need
//!   one ends `new-output-section` (IN-72 item 10).
//!
//! The section identity of `spec/model.md` §7.1 (anchor, name, ordinal)
//! matches a new section to its old row ([`SectionChange::old`]); a region
//! is keyed by its file and `(O, c)`: the [`OutSecId`] and the `.text`
//! category of `spec/layout.md` L-53 (`mortar_layout::order::text_category`,
//! 0 for a section that is not code).

use std::fmt;

use crate::layout::{Chunk, Defined, Hole, Patch, Place, Placement, Redirect, Segment};
use crate::layout::{SlotKind, SymPlace, Tls};
use crate::{ChunkId, FileId, OutSecId, SectionId, SymKey, SymbolId};

// ---- the reasons ------------------------------------------------------------

/// The word `update-not-implemented` carries (`spec/incr.md` §7): which part
/// of the update is missing. Each word is printed in parentheses after the
/// reason (`update-not-implemented (debug)`). Each entry point returns its
/// word until its package fills it; the join (U-10) leaves only `Debug`
/// and `Meta`, which the G packages delete.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Unimplemented {
    /// The state format 5 (U-2: `state5::open`, `patch`, `compact`).
    State5,
    /// Delta resolution, extraction, liveness (U-3).
    Resolve,
    /// The refill set (U-3: `update::refill::refill_set`).
    Refill,
    /// The delta layout (U-4).
    Layout,
    /// The per-file symbol tables (U-5).
    Symtab,
    /// Refill and image (U-6).
    Image,
    /// The writes and the crash protocol (U-7).
    Write,
    /// `.mason.meta` units under `--keep-mason-meta` in a changed file
    /// (IN-71d; until G-1's contributions cover it). U-8's entry point
    /// gives it while it is empty.
    Meta,
    /// A changed, appeared or newly extracted file with debug sections or
    /// a split-DWARF skeleton (IN-70; until G-3 joins G-1 and G-2).
    Debug,
}

impl Unimplemented {
    /// Every word, in `spec/incr.md` §7's order.
    pub const ALL: [Self; 9] = [
        Self::State5,
        Self::Resolve,
        Self::Refill,
        Self::Layout,
        Self::Symtab,
        Self::Image,
        Self::Write,
        Self::Meta,
        Self::Debug,
    ];

    /// The word (`spec/incr.md` §7).
    #[must_use]
    pub const fn word(self) -> &'static str {
        match self {
            Self::State5 => "state5",
            Self::Resolve => "resolve",
            Self::Refill => "refill",
            Self::Layout => "layout",
            Self::Symtab => "symtab",
            Self::Image => "image",
            Self::Write => "write",
            Self::Meta => "meta",
            Self::Debug => "debug",
        }
    }
}

/// Why an incremental-mode link is a full link (`spec/incr.md` §7): one
/// variant per row of §7's table, in its order, and nothing else (a write's
/// own failure is not a reason, IN-77e). `Display` prints the reason's name
/// alone ([`IncrReason::name`]); the detail is data ([`IncrReason::detail`])
/// and [`IncrReason::report`] gives the words the report line and
/// `incremental-update-failed` print (`name`, or `name: detail`, or
/// `update-not-implemented (word)`), byte for byte what the floor printed
/// before this type (WP-213).
///
/// Every variant but `UpdateNotImplemented` carries a detail string, empty
/// when there is nothing to add; a package that names a reason fills it
/// with what a user needs (the key, the section, the capacity), never with
/// text another package parses.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum IncrReason {
    /// `no-state`: no state at the state path (the first link).
    NoState(String),
    /// `state-corrupt`: the state fails validation (IN-18, IN-18c).
    StateCorrupt(String),
    /// `state-version`: the state's format or mortar version differs.
    StateVersion(String),
    /// `options-changed`: the normalized plan differs (IN-18b), or a
    /// changed path that is no file of the link (a linker script).
    OptionsChanged(String),
    /// `output-changed`: the output is missing or not the one the state
    /// describes (IN-21, IN-77a), or W4 found another file at `P`.
    OutputChanged(String),
    /// `output-torn`: the state's "updating" flag is set (IN-46, IN-77).
    OutputTorn(String),
    /// `state-busy`: the state's path named another inode after the lock,
    /// 8 times in a row (IN-77b).
    StateBusy(String),
    /// `lock-unavailable`: `flock` is not enforced on the state's file
    /// system (IN-77b, image.md I-59).
    LockUnavailable(String),
    /// `small-output`: the output is below `mortar_incr::SMALL_OUTPUT`.
    SmallOutput(String),
    /// `update-not-implemented`, with the missing part's word, or none:
    /// the stage-A floor (`mortar::incremental`, every check passed and no
    /// update exists), which prints the bare name.
    UpdateNotImplemented(Option<Unimplemented>),
    /// `extraction-changed`: IN-71b's bound.
    ExtractionChanged(String),
    /// `inputs-reordered`: two unchanged inputs' relative order changed.
    InputsReordered(String),
    /// `too-much-changed`: IN-16's threshold.
    TooMuchChanged(String),
    /// `reserve-exhausted`: a capacity of IN-32, IN-34 is exhausted.
    ReserveExhausted(String),
    /// `island-exhausted`: a thunk or patch does not fit its island
    /// (IN-72 item 9).
    IslandExhausted(String),
    /// `tls-layout-changed` (IN-34).
    TlsLayoutChanged(String),
    /// `copy-size-changed` (IN-34, IN-40).
    CopySizeChanged(String),
    /// `link-order-moved` (IN-72 item 4).
    LinkOrderMoved(String),
    /// `symbol-kind-changed` (IN-26, IN-71).
    SymbolKindChanged(String),
    /// `symtab-exhausted` (IN-49a).
    SymtabExhausted(String),
    /// `new-output-section` (IN-72 item 10).
    NewOutputSection(String),
    /// `property-changed` (IN-72 item 10).
    PropertyChanged(String),
    /// `dynamic-changed` (IN-42).
    DynamicChanged(String),
    /// `debug-compaction` (IN-57).
    DebugCompaction(String),
    /// `debug-name-index` (IN-55).
    DebugNameIndex(String),
    /// `debug-unit-kind` (IN-55a).
    DebugUnitKind(String),
    /// `input-changed`: a file the update read is not the file it judged
    /// (IN-28a condition 2, IN-75).
    InputChanged(String),
    /// `layout-changed`: the test path's only (WP-064a); never an update's.
    LayoutChanged(String),
    /// `resolution-changed`: the test path's only (WP-064a).
    ResolutionChanged(String),
}

impl IncrReason {
    /// Every reason's name, in `spec/incr.md` §7's order (the spec test
    /// compares it with the table).
    pub const NAMES: [&'static str; 29] = [
        "no-state",
        "state-corrupt",
        "state-version",
        "options-changed",
        "output-changed",
        "output-torn",
        "state-busy",
        "lock-unavailable",
        "small-output",
        "update-not-implemented",
        "extraction-changed",
        "inputs-reordered",
        "too-much-changed",
        "reserve-exhausted",
        "island-exhausted",
        "tls-layout-changed",
        "copy-size-changed",
        "link-order-moved",
        "symbol-kind-changed",
        "symtab-exhausted",
        "new-output-section",
        "property-changed",
        "dynamic-changed",
        "debug-compaction",
        "debug-name-index",
        "debug-unit-kind",
        "input-changed",
        "layout-changed",
        "resolution-changed",
    ];

    /// The reason's name (`spec/incr.md` §7).
    #[must_use]
    pub fn name(&self) -> &'static str {
        match self {
            Self::NoState(_) => "no-state",
            Self::StateCorrupt(_) => "state-corrupt",
            Self::StateVersion(_) => "state-version",
            Self::OptionsChanged(_) => "options-changed",
            Self::OutputChanged(_) => "output-changed",
            Self::OutputTorn(_) => "output-torn",
            Self::StateBusy(_) => "state-busy",
            Self::LockUnavailable(_) => "lock-unavailable",
            Self::SmallOutput(_) => "small-output",
            Self::UpdateNotImplemented(_) => "update-not-implemented",
            Self::ExtractionChanged(_) => "extraction-changed",
            Self::InputsReordered(_) => "inputs-reordered",
            Self::TooMuchChanged(_) => "too-much-changed",
            Self::ReserveExhausted(_) => "reserve-exhausted",
            Self::IslandExhausted(_) => "island-exhausted",
            Self::TlsLayoutChanged(_) => "tls-layout-changed",
            Self::CopySizeChanged(_) => "copy-size-changed",
            Self::LinkOrderMoved(_) => "link-order-moved",
            Self::SymbolKindChanged(_) => "symbol-kind-changed",
            Self::SymtabExhausted(_) => "symtab-exhausted",
            Self::NewOutputSection(_) => "new-output-section",
            Self::PropertyChanged(_) => "property-changed",
            Self::DynamicChanged(_) => "dynamic-changed",
            Self::DebugCompaction(_) => "debug-compaction",
            Self::DebugNameIndex(_) => "debug-name-index",
            Self::DebugUnitKind(_) => "debug-unit-kind",
            Self::InputChanged(_) => "input-changed",
            Self::LayoutChanged(_) => "layout-changed",
            Self::ResolutionChanged(_) => "resolution-changed",
        }
    }

    /// The detail: the string a variant carries, or the word of
    /// `update-not-implemented` (empty for the floor's bare name).
    #[must_use]
    pub fn detail(&self) -> &str {
        match self {
            Self::UpdateNotImplemented(w) => w.map_or("", Unimplemented::word),
            Self::NoState(d)
            | Self::StateCorrupt(d)
            | Self::StateVersion(d)
            | Self::OptionsChanged(d)
            | Self::OutputChanged(d)
            | Self::OutputTorn(d)
            | Self::StateBusy(d)
            | Self::LockUnavailable(d)
            | Self::SmallOutput(d)
            | Self::ExtractionChanged(d)
            | Self::InputsReordered(d)
            | Self::TooMuchChanged(d)
            | Self::ReserveExhausted(d)
            | Self::IslandExhausted(d)
            | Self::TlsLayoutChanged(d)
            | Self::CopySizeChanged(d)
            | Self::LinkOrderMoved(d)
            | Self::SymbolKindChanged(d)
            | Self::SymtabExhausted(d)
            | Self::NewOutputSection(d)
            | Self::PropertyChanged(d)
            | Self::DynamicChanged(d)
            | Self::DebugCompaction(d)
            | Self::DebugNameIndex(d)
            | Self::DebugUnitKind(d)
            | Self::InputChanged(d)
            | Self::LayoutChanged(d)
            | Self::ResolutionChanged(d) => d,
        }
    }

    /// The reason named `name` with `detail` (`update-not-implemented`:
    /// `detail` is its word, or empty), or `None` for a name §7 does not
    /// have. For the places that name a reason by its `&'static str`
    /// today (`mortar_incr::delta`'s fall-backs).
    #[must_use]
    pub fn from_name(name: &str, detail: impl Into<String>) -> Option<Self> {
        let d = detail.into();
        Some(match name {
            "no-state" => Self::NoState(d),
            "state-corrupt" => Self::StateCorrupt(d),
            "state-version" => Self::StateVersion(d),
            "options-changed" => Self::OptionsChanged(d),
            "output-changed" => Self::OutputChanged(d),
            "output-torn" => Self::OutputTorn(d),
            "state-busy" => Self::StateBusy(d),
            "lock-unavailable" => Self::LockUnavailable(d),
            "small-output" => Self::SmallOutput(d),
            "update-not-implemented" => {
                if d.is_empty() {
                    Self::UpdateNotImplemented(None)
                } else {
                    let w = Unimplemented::ALL.into_iter().find(|w| w.word() == d)?;
                    Self::UpdateNotImplemented(Some(w))
                }
            }
            "extraction-changed" => Self::ExtractionChanged(d),
            "inputs-reordered" => Self::InputsReordered(d),
            "too-much-changed" => Self::TooMuchChanged(d),
            "reserve-exhausted" => Self::ReserveExhausted(d),
            "island-exhausted" => Self::IslandExhausted(d),
            "tls-layout-changed" => Self::TlsLayoutChanged(d),
            "copy-size-changed" => Self::CopySizeChanged(d),
            "link-order-moved" => Self::LinkOrderMoved(d),
            "symbol-kind-changed" => Self::SymbolKindChanged(d),
            "symtab-exhausted" => Self::SymtabExhausted(d),
            "new-output-section" => Self::NewOutputSection(d),
            "property-changed" => Self::PropertyChanged(d),
            "dynamic-changed" => Self::DynamicChanged(d),
            "debug-compaction" => Self::DebugCompaction(d),
            "debug-name-index" => Self::DebugNameIndex(d),
            "debug-unit-kind" => Self::DebugUnitKind(d),
            "input-changed" => Self::InputChanged(d),
            "layout-changed" => Self::LayoutChanged(d),
            "resolution-changed" => Self::ResolutionChanged(d),
            _ => return None,
        })
    }

    /// The words of the report line `incremental: full (<report>)` and of
    /// `incremental-update-failed`: the name alone when the detail is
    /// empty; `update-not-implemented (word)`; else `name: detail`.
    #[must_use]
    pub fn report(&self) -> String {
        match self {
            Self::UpdateNotImplemented(Some(w)) => format!("{} ({})", self.name(), w.word()),
            _ if self.detail().is_empty() => self.name().to_owned(),
            _ => format!("{}: {}", self.name(), self.detail()),
        }
    }
}

impl fmt::Display for IncrReason {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.name())
    }
}

// ---- the stored layout (what U-4 and U-5 read) ------------------------------

/// A section row as the delta layout reads it (`SECTIONS`, IN-18e;
/// IN-72): its identity (`spec/model.md` §7.1), header, and place. Read by
/// U-4 (keep/place phases), U-5 (its file's symbols' sections); written by
/// U-2's reader from the row.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StoredSection {
    /// The `SECTIONS` row.
    pub id: SectionId,
    /// The `FILES` row of its file.
    pub file: FileId,
    /// Its index in its file's section header table.
    pub index: u32,
    /// The anchor's kind (`format::SectionRow::anchor_kind`: 0 none, 1
    /// COMDAT signature, 2 global, 3 local, as `capture.rs` writes it).
    pub anchor_kind: u8,
    /// The anchor's bytes.
    pub anchor: Vec<u8>,
    /// The section's name.
    pub name: Vec<u8>,
    /// Among the file's sections with the same anchor and name.
    pub ordinal: u32,
    pub sh_type: u32,
    pub sh_flags: u64,
    /// Bytes.
    pub size: u64,
    /// Bytes (a power of two; 1 for none).
    pub align: u64,
    /// Where it is: `(chunk, offset from the chunk's start)`, or
    /// [`Place::NONE`] when the link placed it nowhere (dead, discarded,
    /// merged: a merged section's pieces are `FRAGS`).
    pub place: Place,
}

/// A region (IN-30, IN-72 item 2): a run of one file's sections in one
/// `(O, c)` of a hole-tolerant output section, with its slack. A file has
/// one per `(O, c)` from its first link and one more per overflow, kept
/// until the compaction. Key: `(file, out, category, ordinal)`. Written by
/// the first link's capture and by U-4 (`LayoutDelta::regions_opened`);
/// read by U-4 (phase 2 (ii)), U-6 (the map's `region` line, WP-211
/// §R.12), H-1 (`P_U`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct StoredRegion {
    pub file: FileId,
    pub out: OutSecId,
    /// The `.text` category (layout.md L-53); 0 when not code.
    pub category: u8,
    /// 0 for the first link's region, then the overflow regions in the
    /// order they were opened (IN-72 item 2 fills them in this order).
    pub ordinal: u32,
    /// The body chunk it is in.
    pub chunk: ChunkId,
    /// Bytes from the chunk's start.
    pub offset: u64,
    /// Bytes: its sections, their padding and its slack (IN-33).
    pub capacity: u64,
}

/// A merged-section fragment (`FRAGS`, IN-72 item 6): identity (output
/// section, merge class, content hash), offset, reference count. Key: the
/// identity. Read and written by U-4; read by U-6 (a fragment whose count
/// rises from 0 is rewritten).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct StoredFrag {
    pub out: OutSecId,
    /// The merge class (`sh_flags`, `sh_entsize`, alignment of its pieces)
    /// as the `FRAGS.ordinal` column numbers it.
    pub class: u32,
    /// L128 of the content.
    pub hash: [u8; 16],
    /// Bytes from the merged chunk's start.
    pub offset: u64,
    /// Live pieces naming it (IN-18e `FRAGS.refs`); 0: a dead range.
    pub refs: u32,
}

/// A GOT, PLT, IPLT or TLS slot (`SLOTS`, IN-28b, IN-72 item 7): the
/// `index`-th slot of its kind's list. Read and written by U-4.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct StoredSlot {
    pub kind: SlotKind,
    /// The key (a global's `SYMBOLS` row, or a local by `(FILES row,
    /// symbol-table index)`: IN-28b's section identity is the caller's).
    pub key: SymKey,
    /// Its position in its kind's list.
    pub index: u32,
}

/// One file's `.symtab` block and `.strtab` region (`SYMTAB` table 43,
/// IN-49a), or (for file 0's row) the global area. Entries and bytes;
/// read and written by U-5; U-2 stores it; the map's `symblock` line
/// (WP-211 §R.12, U-6).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash)]
pub struct SymtabBlock {
    /// The block's first `.symtab` index.
    pub first: u32,
    /// Its entries (the file's locals, S-74), fillers not counted.
    pub count: u32,
    /// Entries it can hold: `count + max(2, count/8)` when written.
    pub capacity: u32,
    /// Its `.strtab` region: offset in `.strtab`, bytes used, bytes held.
    pub strtab_offset: u64,
    pub strtab_len: u64,
    pub strtab_capacity: u64,
}

/// The sizes of `.symtab` and `.strtab` and their file slack (IN-49a):
/// read by U-5.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash)]
pub struct SymtabTables {
    /// `sh_info`: entries before the global area, fillers included.
    pub sh_info: u32,
    /// The first index of the local tail reserve and its entries.
    pub tail_first: u32,
    pub tail_capacity: u32,
    /// `.symtab`'s `sh_size` and the bytes after it in no section.
    pub symtab_size: u64,
    pub symtab_slack: u64,
    /// `.strtab`'s `sh_size` and its slack.
    pub strtab_size: u64,
    pub strtab_slack: u64,
}

/// The read side of a stored layout (WP-211 §C `StoredLayout`): the
/// `Placement` rows an update reads, each by the key the update has, never
/// a whole read of a table that is not "whole" in IN-18e (IN-18c: an update
/// reads only the blocks it touches). Every call can fail with
/// `state-corrupt` (a block that fails its hash, IN-18c). Implemented by U-2
/// over a `StateView` (`mortar_incr::state5::read::LayoutOf`), by
/// [`MemoryLayout`] for tests and for H-1's `P_U` (read from a map).
///
/// Cost classes: *whole* reads a whole small table (IN-18e class "whole",
/// under 2 MB together on g10x); *range* reads one file's range or one key's
/// rows; nothing here is O(state).
pub trait LayoutRows: Sync {
    /// `CHUNKS` (whole): every chunk, by id.
    ///
    /// # Errors
    /// `state-corrupt`.
    fn chunks(&self) -> Result<Vec<(ChunkId, Chunk)>, IncrReason>;
    /// `ORDER` (whole): every chunk once, in file order.
    ///
    /// # Errors
    /// `state-corrupt`.
    fn order(&self) -> Result<Vec<ChunkId>, IncrReason>;
    /// `SEGMENTS` (whole).
    ///
    /// # Errors
    /// `state-corrupt`.
    fn segments(&self) -> Result<Vec<Segment>, IncrReason>;
    /// `OUTSECS` (whole): each output section's name.
    ///
    /// # Errors
    /// `state-corrupt`.
    fn outsecs(&self) -> Result<Vec<(OutSecId, Vec<u8>)>, IncrReason>;
    /// A file's `SECTIONS` range (range, by `FILE_INDEX`).
    ///
    /// # Errors
    /// `state-corrupt`.
    fn sections_of(&self, file: FileId) -> Result<Vec<StoredSection>, IncrReason>;
    /// A file's regions, every `(O, c)`, in `(out, category, ordinal)`
    /// order (range). See WP-213's finding on where they are stored.
    ///
    /// # Errors
    /// `state-corrupt`.
    fn regions_of(&self, file: FileId) -> Result<Vec<StoredRegion>, IncrReason>;
    /// A chunk's holes, by offset (key-keyed `HOLES`, two binary searches).
    ///
    /// # Errors
    /// `state-corrupt`.
    fn holes_in(&self, chunk: ChunkId) -> Result<Vec<Hole>, IncrReason>;
    /// An output section's fragments, by offset (key-keyed `FRAGS`).
    ///
    /// # Errors
    /// `state-corrupt`.
    fn frags_in(&self, out: OutSecId) -> Result<Vec<StoredFrag>, IncrReason>;
    /// `SLOTS` (whole).
    ///
    /// # Errors
    /// `state-corrupt`.
    fn slots(&self) -> Result<Vec<StoredSlot>, IncrReason>;
    /// `PATCHES` (whole): thunks and erratum patches; an island is a
    /// `ChunkKind::Patches` chunk with its capacity.
    ///
    /// # Errors
    /// `state-corrupt`.
    fn patches(&self) -> Result<Vec<Patch>, IncrReason>;
    /// `REDIRECTS` (whole).
    ///
    /// # Errors
    /// `state-corrupt`.
    fn redirects(&self) -> Result<Vec<Redirect>, IncrReason>;
    /// `SYM_PLACE` (whole): commons, copies (a common's place is a section
    /// of its winning file in incremental mode, IN-72 item 11).
    ///
    /// # Errors
    /// `state-corrupt`.
    fn sym_place(&self) -> Result<Vec<SymPlace>, IncrReason>;
    /// `DEFINED` (whole).
    ///
    /// # Errors
    /// `state-corrupt`.
    fn defined(&self) -> Result<Vec<Defined>, IncrReason>;
    /// `LAYOUT` (whole): the TLS template and the entry.
    ///
    /// # Errors
    /// `state-corrupt`.
    fn tls_and_entry(&self) -> Result<(Option<Tls>, Option<u64>), IncrReason>;
    /// A file's `.symtab` block (`SYMTAB`, range); `None` when it has none.
    ///
    /// # Errors
    /// `state-corrupt`.
    fn symtab_block(&self, file: FileId) -> Result<Option<SymtabBlock>, IncrReason>;
    /// The global area (file 0's `SYMTAB` row) and the tables' sizes.
    ///
    /// # Errors
    /// `state-corrupt`.
    fn symtab_tables(&self) -> Result<(SymtabBlock, SymtabTables), IncrReason>;
}

/// A stored layout (WP-211 §C): a borrowed [`LayoutRows`]. U-4's and U-5's
/// entry points take it; the rows are read through `Deref`.
#[derive(Clone, Copy)]
pub struct StoredLayout<'a> {
    rows: &'a dyn LayoutRows,
}

impl<'a> StoredLayout<'a> {
    /// The layout `rows` give.
    #[must_use]
    pub fn new(rows: &'a dyn LayoutRows) -> Self {
        Self { rows }
    }
}

impl<'a> std::ops::Deref for StoredLayout<'a> {
    type Target = dyn LayoutRows + 'a;
    fn deref(&self) -> &Self::Target {
        self.rows
    }
}

impl fmt::Debug for StoredLayout<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("StoredLayout").finish_non_exhaustive()
    }
}

/// [`LayoutRows`] in memory: hand-made rows for the packages' tests, and
/// H-1's `P_U` read from an update's map (IN-78). Plain public fields;
/// every accessor filters or copies them (no hashing, never fails). No
/// link builds one.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct MemoryLayout {
    pub chunks: Vec<Chunk>,
    pub order: Vec<ChunkId>,
    pub segments: Vec<Segment>,
    pub outsecs: Vec<Vec<u8>>,
    pub sections: Vec<StoredSection>,
    pub regions: Vec<StoredRegion>,
    pub holes: Vec<Hole>,
    pub frags: Vec<StoredFrag>,
    pub slots: Vec<StoredSlot>,
    pub patches: Vec<Patch>,
    pub redirects: Vec<Redirect>,
    pub sym_place: Vec<SymPlace>,
    pub defined: Vec<Defined>,
    pub tls: Option<Tls>,
    pub entry: Option<u64>,
    /// `(file, block)`.
    pub symtab_blocks: Vec<(FileId, SymtabBlock)>,
    pub global_area: SymtabBlock,
    pub symtab_tables: SymtabTables,
}

impl LayoutRows for MemoryLayout {
    fn chunks(&self) -> Result<Vec<(ChunkId, Chunk)>, IncrReason> {
        Ok(self.chunks.iter().enumerate().map(|(i, c)| (ChunkId(i as u32), *c)).collect())
    }
    fn order(&self) -> Result<Vec<ChunkId>, IncrReason> {
        Ok(self.order.clone())
    }
    fn segments(&self) -> Result<Vec<Segment>, IncrReason> {
        Ok(self.segments.clone())
    }
    fn outsecs(&self) -> Result<Vec<(OutSecId, Vec<u8>)>, IncrReason> {
        Ok(self.outsecs.iter().enumerate().map(|(i, n)| (OutSecId(i as u32), n.clone())).collect())
    }
    fn sections_of(&self, file: FileId) -> Result<Vec<StoredSection>, IncrReason> {
        Ok(self.sections.iter().filter(|s| s.file == file).cloned().collect())
    }
    fn regions_of(&self, file: FileId) -> Result<Vec<StoredRegion>, IncrReason> {
        let mut v: Vec<StoredRegion> =
            self.regions.iter().filter(|r| r.file == file).copied().collect();
        v.sort_by_key(|r| (r.out, r.category, r.ordinal));
        Ok(v)
    }
    fn holes_in(&self, chunk: ChunkId) -> Result<Vec<Hole>, IncrReason> {
        let mut v: Vec<Hole> = self.holes.iter().filter(|h| h.chunk == chunk).copied().collect();
        v.sort_by_key(|h| h.offset);
        Ok(v)
    }
    fn frags_in(&self, out: OutSecId) -> Result<Vec<StoredFrag>, IncrReason> {
        let mut v: Vec<StoredFrag> = self.frags.iter().filter(|f| f.out == out).copied().collect();
        v.sort_by_key(|f| f.offset);
        Ok(v)
    }
    fn slots(&self) -> Result<Vec<StoredSlot>, IncrReason> {
        Ok(self.slots.clone())
    }
    fn patches(&self) -> Result<Vec<Patch>, IncrReason> {
        Ok(self.patches.clone())
    }
    fn redirects(&self) -> Result<Vec<Redirect>, IncrReason> {
        Ok(self.redirects.clone())
    }
    fn sym_place(&self) -> Result<Vec<SymPlace>, IncrReason> {
        Ok(self.sym_place.clone())
    }
    fn defined(&self) -> Result<Vec<Defined>, IncrReason> {
        Ok(self.defined.clone())
    }
    fn tls_and_entry(&self) -> Result<(Option<Tls>, Option<u64>), IncrReason> {
        Ok((self.tls, self.entry))
    }
    fn symtab_block(&self, file: FileId) -> Result<Option<SymtabBlock>, IncrReason> {
        Ok(self.symtab_blocks.iter().find(|(f, _)| *f == file).map(|(_, b)| *b))
    }
    fn symtab_tables(&self) -> Result<(SymtabBlock, SymtabTables), IncrReason> {
        Ok((self.global_area, self.symtab_tables))
    }
}

// ---- what U-4 takes and gives -----------------------------------------------

/// One section of a changed or drawn-in file as the update's front gives
/// it (IN-72 item 2, 2a), the input of the delta layout (U-4). Written by
/// the join (U-10) from the parsed file and U-3's liveness and election;
/// read by U-4. Key: `id`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SectionChange<'a> {
    /// Its new id (the module text's id space).
    pub id: SectionId,
    /// Its file's `FILES` row (a new id for an appeared file).
    pub file: FileId,
    /// Its index in the file's section header table.
    pub index: u32,
    /// Its identity (`spec/model.md` §7.1): anchor and name, then
    /// `ordinal` among the file's sections with the same key.
    pub key: crate::identity::SectionKey<'a>,
    pub ordinal: u32,
    /// The `SECTIONS` row of its previous version matched by that
    /// identity, or `None` (new to the file, or its file appeared).
    pub old: Option<SectionId>,
    pub sh_type: u32,
    pub sh_flags: u64,
    /// Bytes; for a common's section (IN-72 item 11) the common's size.
    pub size: u64,
    /// Bytes (a power of two).
    pub align: u64,
    /// Live after IN-71a (always `true` without `--gc-sections`).
    pub live: bool,
    /// Kept by IN-71's COMDAT election (`true` outside a group).
    pub kept: bool,
}

/// A growth step (IN-72 item 3): an output section grown into its
/// segment's reserve, and what moved. Written by U-4; read by U-6 (both
/// header tables rewritten, every moved byte dirty), U-3 (the moved
/// sections' referrers), U-5 (remapped `.symtab` values).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct GrowthStep {
    /// The output section that grew.
    pub out: OutSecId,
    /// Bytes added to its tail; every later output section of its
    /// `PT_LOAD` moves by this much (in address, and in file offset unless
    /// `NOBITS`).
    pub step: u64,
}

/// The delta layout's result (WP-211 §C `LayoutDelta`; IN-72): the update's
/// own `Placement` (`P_U`, IN-78) and what changed against the stored one.
/// Written by U-4; read by U-3 (`recompute`, `refill_set`), U-5, U-6, U-2
/// (the patch's decision rows), the join.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct LayoutDelta {
    /// The update's placement, of the full link's shape (`Layout::check`
    /// accepts it), indexed by the update's id space: a section of a
    /// changed file's old version, killed or gone, has [`Place::NONE`].
    pub placement: Placement,
    /// Sections whose address changed, in any file (IN-27's moved
    /// sections; their files' `SECTIONS` rows are rewritten, IN-78),
    /// sorted.
    pub moved: Vec<SectionId>,
    /// Sections placed before and not now (dead, no longer kept, gone),
    /// sorted.
    pub killed: Vec<SectionId>,
    /// Sections not placed before and placed now (revived, newly kept),
    /// sorted.
    pub revived: Vec<SectionId>,
    /// Holes and dead ranges this update made (IN-3, IN-3a), sorted by
    /// `(chunk, offset)`; `placement.holes` has every hole of the output.
    pub holes: Vec<Hole>,
    /// Regions opened by this update's overflow (IN-72 item 2 phase 3).
    pub regions_opened: Vec<StoredRegion>,
    /// Growth steps, in address order (IN-72 item 3).
    pub growth_steps: Vec<GrowthStep>,
    /// Each winning common's key and the file whose `.bss` region holds
    /// it (IN-72 item 11; the map's `common` line names the file, U-6).
    pub commons: Vec<(SymbolId, FileId)>,
    /// Fragments whose `refs` or offset changed, appended or revived
    /// (IN-72 item 6), by identity.
    pub frags: Vec<StoredFrag>,
    /// Slots appended (IN-72 item 7).
    pub slots_appended: Vec<StoredSlot>,
    /// Slots now dead (written 0, IN-72 item 7).
    pub slots_dead: Vec<StoredSlot>,
}

// ---- what U-5 gives ---------------------------------------------------------

/// One `.symtab` entry an update writes (IN-49a), by index. A filler is
/// an entry of the filler's form (`name` 0, `info` 0x03, `shndx` 1, `value`
/// section 1's address); the plan lists fillers as ranges instead.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct SymtabEntry {
    /// `.symtab` index.
    pub index: u32,
    /// `st_name`: offset in `.strtab`.
    pub name: u32,
    pub info: u8,
    pub other: u8,
    /// The output section index (`SHN_XINDEX` and `.symtab_shndx` when it
    /// does not fit 16 bits, the writer's).
    pub shndx: u32,
    pub value: u64,
    pub size: u64,
}

/// The update's `.symtab`/`.strtab` plan (WP-211 §C `SymtabPlan`; IN-49a):
/// what U-5 decides and U-6 writes. Not `mortar_synth::SymtabPlan` (the
/// full link's plan). Written by U-5; read by U-6 (the bytes and the map's
/// `hole .symtab`/`.strtab` lines), U-2 (the `SYMTAB` and `SYMTAB_KEYS`
/// rows of the patch).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SymtabPlan {
    /// Every rewritten or moved block, by file: its new place.
    pub blocks: Vec<(FileId, SymtabBlock)>,
    /// The global area after the update.
    pub global_area: SymtabBlock,
    /// The tables' sizes after the update (`sh_info`, `sh_size`, slack).
    pub tables: SymtabTables,
    /// Entries to write, by index.
    pub entries: Vec<SymtabEntry>,
    /// Index ranges `[start, end)` to write as fillers (a block's unused
    /// capacity, a moved block's old place).
    pub fillers: Vec<(u32, u32)>,
    /// `.strtab` bytes to write: `(offset, bytes)` (names, each with its
    /// terminating zero).
    pub strtab: Vec<(u64, Vec<u8>)>,
    /// `.strtab` dead ranges `(offset, size)` (old names, IN-49a: the map's
    /// `dead .strtab <off> <size> strings`).
    pub strtab_dead: Vec<(u64, u64)>,
    /// Each global whose `.symtab` index or `.strtab` offset changed:
    /// `(key, index, strtab offset)` (`SYMTAB_KEYS` rows).
    pub globals: Vec<(SymbolId, u32, u32)>,
    /// Entries U-6 copies from the previous output, `(from, to)` indices
    /// (the global area's last entry taking a removed one's index; IN-49a:
    /// read from the output, not the state).
    pub moves: Vec<(u32, u32)>,
    /// Index ranges `[start, end)` of unchanged files' blocks whose
    /// `st_value`s U-6 remaps through `LayoutDelta::growth_steps`, the rest
    /// of each entry as read (IN-49a).
    pub remap: Vec<(u32, u32)>,
}

// ---- what U-3 gives U-6 -----------------------------------------------------

/// A unit to refill (IN-27, IN-74): a section of the update's id space.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct RefillUnit {
    pub section: SectionId,
    /// `mortar_incr::delta::reason` bits: why (the report's words).
    pub reasons: u32,
}

/// A synthetic or packed chunk rewritten whole (IN-72 items 4, 5, 7).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct RefillChunk {
    pub chunk: ChunkId,
    /// `mortar_incr::delta::reason` bits.
    pub reasons: u32,
}

/// The refill set of an update (WP-211 §C `RefillSet`: `delta::Delta`'s
/// shape, IN-74). Moved here from `mortar_incr::update::refill` (WP-213):
/// U-6's `mortar_image::update::refill` takes it and `mortar_image` does not
/// depend on `mortar_incr`. Written by U-3; read by U-6, the bench rows
/// (B-1).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct RefillSet {
    /// Units refilled, sorted by section.
    pub units: Vec<RefillUnit>,
    /// Chunks rewritten whole, sorted.
    pub chunks: Vec<RefillChunk>,
    /// Bytes of the units, and of the output's allocated sections (IN-16's
    /// bound is their ratio).
    pub unit_bytes: u64,
    pub alloc_bytes: u64,
    /// Definitions of changed and drawn-in files whose address moved (the
    /// bench row of IN-74).
    pub moved_definitions: u64,
    /// Files with a refilled unit (the bench row of IN-74).
    pub referrer_files: u64,
}

// ---- what U-6 gives U-7 -----------------------------------------------------

/// A range of the output the update writes (WP-211 §C; IN-75: the dirty
/// ranges cover every byte that differs from the previous output). Bytes
/// from the file's start. Written by U-6; read by U-7 (W3/C1), H-1.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct DirtyRange {
    pub offset: u64,
    pub len: u64,
}

#[cfg(test)]
mod tests {
    use super::*;

    // spec: incr.md §7 (one name per reason; the report words of today)
    #[test]
    fn names_round_trip_and_report_words() {
        for name in IncrReason::NAMES {
            let r = IncrReason::from_name(name, "").expect(name);
            assert_eq!(r.name(), name);
            assert_eq!(r.to_string(), name);
            assert_eq!(r.report(), name);
        }
        let r = IncrReason::StateCorrupt("table 7".into());
        assert_eq!(r.report(), "state-corrupt: table 7");
        assert_eq!(r.to_string(), "state-corrupt");
        let r = IncrReason::UpdateNotImplemented(Some(Unimplemented::Debug));
        assert_eq!(r.report(), "update-not-implemented (debug)");
        assert_eq!(r.detail(), "debug");
        for w in Unimplemented::ALL {
            let r = IncrReason::from_name("update-not-implemented", w.word()).unwrap();
            assert_eq!(r, IncrReason::UpdateNotImplemented(Some(w)));
        }
        assert!(IncrReason::from_name("update-not-implemented", "nonsense").is_none());
        assert!(IncrReason::from_name("full requested", "").is_none());
    }
}
