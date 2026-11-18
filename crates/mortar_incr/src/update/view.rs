//! The state as the update reads it ([`StateView`], [`StateRows`]), what
//! U2-U4 found changed ([`ChangedFiles`]), and what W5 writes
//! ([`StatePatch`]) (`spec/incr.md` IN-18c..IN-18e, IN-46b, IN-70;
//! WP-211 §C). WP-213 (U-1): the shapes are U-1's; **nothing in this file
//! is filled by another package** (U-2 implements [`StateRows`] in
//! `state5::read`; the in-memory backing is [`super::testing`]).
//!
//! **The read rule (IN-18c).** Every accessor reads the rows its key
//! names and nothing else: a file's range of a file-keyed table (through
//! `FILE_INDEX`), a key's rows of a key-keyed table (two binary searches,
//! body and append area), a key-parallel row by its `SYMBOLS` row, or a
//! whole table of the "whole" class (small: under 2 MB together on g10x).
//! Each accessor's doc names its table, class and cost. There is no
//! accessor that reads a large table whole (the compaction reads the
//! state through `state5::compact`, not through this trait). Every
//! accessor can fail with `state-corrupt` (a block that fails its hash, a
//! row out of range), never panic.

use mortar_core::incr::IncrReason;
use mortar_core::{ChunkId, FileId, OutSecId, SectionId, SymbolId};

use super::diags::DiagRow;
use super::rows::{
    CommonRow, EdgeRow, FileIndexRow, FileRow, FragRow, GroupRow, HoleRow, IndexRow, KeyRefRow,
    MentionRow, MetaRow, NameRow, OdrClassRow, OdrRefRow, RegionRow, ResolvedRow, SymbolRow,
    SymtabKeyRow, SymtabRow,
};
use crate::format::{
    ChunkRow, DefinedRow, EhRow, FragAtRow, LayoutRow, OutSecRow, PatchRow, ProbeRow, RedirectRow,
    SectionRow, SegmentRow, SizeRow, SlotRow, StatRow, Str, SymPlaceRow,
};

/// Read a row or a table, or `state-corrupt`.
pub type Rows<T> = Result<T, IncrReason>;

/// The read side of a format-5 state (IN-18e's access patterns). Two
/// implementations: U-2's lazy per-block reader over U1's descriptor
/// (`state5::open`) and [`super::testing::MemoryState`].
///
/// Cost classes in the docs: **header** (the directory, already read and
/// checked); **whole** (a whole "whole"-class table: small); **row** (one
/// row by index: one block); **range** (one file's range: its blocks);
/// **key** (one key's rows of a key-keyed table: two binary searches);
/// **scan** (a key-keyed table read whole: named where IN-75a needs it).
pub trait StateRows: Sync {
    /// Rows of table `kind` as the directory gives them (header). The
    /// update's id space (`mortar_core::incr`) starts new ids here.
    fn count(&self, kind: u32) -> u64;
    /// The plan hash in the header (IN-18b; header).
    fn plan_hash(&self) -> [u8; 16];

    // ---- whole ----
    /// `META` (kind 1; whole: one row).
    ///
    /// # Errors
    /// `state-corrupt`.
    fn meta(&self) -> Rows<MetaRow>;
    /// `PROBES` (kind 4; whole).
    ///
    /// # Errors
    /// `state-corrupt`.
    fn probes(&self) -> Rows<Vec<ProbeRow>>;
    /// `OUTSECS` (kind 5; whole).
    ///
    /// # Errors
    /// `state-corrupt`.
    fn outsecs(&self) -> Rows<Vec<OutSecRow>>;
    /// `CHUNKS` (kind 8; whole).
    ///
    /// # Errors
    /// `state-corrupt`.
    fn chunks(&self) -> Rows<Vec<ChunkRow>>;
    /// `ORDER` (kind 9; whole).
    ///
    /// # Errors
    /// `state-corrupt`.
    fn order(&self) -> Rows<Vec<u32>>;
    /// `SEGMENTS` (kind 10; whole).
    ///
    /// # Errors
    /// `state-corrupt`.
    fn segments(&self) -> Rows<Vec<SegmentRow>>;
    /// `SLOTS` (kind 12; whole).
    ///
    /// # Errors
    /// `state-corrupt`.
    fn slots(&self) -> Rows<Vec<SlotRow>>;
    /// `SIZES` (kind 15; whole).
    ///
    /// # Errors
    /// `state-corrupt`.
    fn sizes(&self) -> Rows<Vec<SizeRow>>;
    /// `SYM_PLACE` (kind 16; whole).
    ///
    /// # Errors
    /// `state-corrupt`.
    fn sym_place(&self) -> Rows<Vec<SymPlaceRow>>;
    /// `DEFINED` (kind 17; whole).
    ///
    /// # Errors
    /// `state-corrupt`.
    fn defined(&self) -> Rows<Vec<DefinedRow>>;
    /// `PATCHES` (kind 18; whole).
    ///
    /// # Errors
    /// `state-corrupt`.
    fn patches(&self) -> Rows<Vec<PatchRow>>;
    /// `REDIRECTS` (kind 19; whole).
    ///
    /// # Errors
    /// `state-corrupt`.
    fn redirects(&self) -> Rows<Vec<RedirectRow>>;
    /// `SIZED` (kind 20; whole).
    ///
    /// # Errors
    /// `state-corrupt`.
    fn sized(&self) -> Rows<Vec<SizeRow>>;
    /// `LAYOUT` (kind 21; whole: one row).
    ///
    /// # Errors
    /// `state-corrupt`.
    fn layout(&self) -> Rows<LayoutRow>;
    /// `LEAVES` (kind 22; whole: one leaf hash per leaf of the output).
    ///
    /// # Errors
    /// `state-corrupt`.
    fn leaves(&self) -> Rows<Vec<[u8; 32]>>;

    // ---- append-only, by row ----
    /// A `FILES` row (kind 3; row).
    ///
    /// # Errors
    /// `state-corrupt`, a row out of range included.
    fn file(&self, file: FileId) -> Rows<FileRow>;
    /// A `FILE_INDEX` row (kind 41; row).
    ///
    /// # Errors
    /// `state-corrupt`.
    fn file_index(&self, file: FileId) -> Rows<FileIndexRow>;
    /// A `STAT` row (kind 2; row): `row` is the `STAT` row, which for an
    /// input is its `FILES.mention`'s (format 4's numbering, kept).
    ///
    /// # Errors
    /// `state-corrupt`.
    fn stat(&self, row: u32) -> Rows<StatRow>;
    /// A string of `STRINGS` (kind 23; the blocks it spans).
    ///
    /// # Errors
    /// `state-corrupt`.
    fn string(&self, at: Str) -> Rows<Vec<u8>>;

    // ---- file-keyed: one file's range ----
    /// A file's `SECTIONS` rows (kind 7; range), in section-index order;
    /// the k-th row is `SectionId(first + k)`.
    ///
    /// # Errors
    /// `state-corrupt`.
    fn sections_of(&self, file: FileId) -> Rows<Vec<SectionRow>>;
    /// One `SECTIONS` row by its id (kind 7; row).
    ///
    /// # Errors
    /// `state-corrupt`.
    fn section(&self, section: SectionId) -> Rows<SectionRow>;
    /// A file's `MENTIONS` rows (kind 32; range).
    ///
    /// # Errors
    /// `state-corrupt`.
    fn mentions_of(&self, file: FileId) -> Rows<Vec<MentionRow>>;
    /// A file's `INDEX` rows (kind 34; range).
    ///
    /// # Errors
    /// `state-corrupt`.
    fn index_of(&self, file: FileId) -> Rows<Vec<IndexRow>>;
    /// A file's `EDGES` rows (kind 42; range).
    ///
    /// # Errors
    /// `state-corrupt`.
    fn edges_of(&self, file: FileId) -> Rows<Vec<EdgeRow>>;
    /// A file's `FRAG_AT` rows (kind 40; range).
    ///
    /// # Errors
    /// `state-corrupt`.
    fn frag_at_of(&self, file: FileId) -> Rows<Vec<FragAtRow>>;
    /// A file's `EH_RECORDS` rows (kind 13; range).
    ///
    /// # Errors
    /// `state-corrupt`.
    fn eh_records_of(&self, file: FileId) -> Rows<Vec<EhRow>>;
    /// A file's `SYMTAB` row (kind 43; range of 0 or 1 row); file 0's is
    /// the global area.
    ///
    /// # Errors
    /// `state-corrupt`.
    fn symtab_of(&self, file: FileId) -> Rows<Option<SymtabRow>>;
    /// A file's `REGIONS` rows (kind 51; range), in `(out, category,
    /// ordinal)` order.
    ///
    /// # Errors
    /// `state-corrupt`.
    fn regions_of(&self, file: FileId) -> Rows<Vec<RegionRow>>;

    // ---- key-keyed: one key's rows ----
    /// A key's `KEY_REFS` rows, by file (kind 45; key).
    ///
    /// # Errors
    /// `state-corrupt`.
    fn key_refs(&self, key: SymbolId) -> Rows<Vec<KeyRefRow>>;
    /// The `GROUPS` rows of a signature (kind 44; key: every row of the
    /// signature, one per live file with a copy), in **no promised
    /// order**, no tombstone; the keeper is [`super::rows::keeper`] of
    /// them, by the files' current `FILES.rank` (WP-215 D2).
    ///
    /// # Errors
    /// `state-corrupt`.
    fn group(&self, signature: &[u8]) -> Rows<Vec<GroupRow>>;
    /// An output section's `FRAGS` rows, by offset (kind 11; key).
    ///
    /// # Errors
    /// `state-corrupt`.
    fn frags_in(&self, out: OutSecId) -> Rows<Vec<FragRow>>;
    /// A chunk's `HOLES` rows, by offset (kind 39; key).
    ///
    /// # Errors
    /// `state-corrupt`.
    fn holes_in(&self, chunk: ChunkId) -> Rows<Vec<HoleRow>>;
    /// A key's `COMMONS` rows (kind 46; key: one per live file with a
    /// common mention of it), in **no promised order**, no tombstone
    /// (WP-215 D6).
    ///
    /// # Errors
    /// `state-corrupt`.
    fn commons(&self, key: SymbolId) -> Rows<Vec<CommonRow>>;
    /// A key's **stored** common as resolve.md §3.3 gives it (kind 46;
    /// key, plus one `FILES` row per common mention):
    /// [`super::rows::winning_common`] of [`Self::commons`] ranked by this
    /// view's own `FILES.rank` (the winning file, the maximum size and
    /// alignment), `None` when no live file has a common mention of it.
    /// An update whose changed, appeared or drawn-in files have new ranks
    /// calls `winning_common` itself with the current ranks. Provided: an
    /// implementation does not override it (no winner is stored).
    ///
    /// # Errors
    /// `state-corrupt`.
    fn common(&self, key: SymbolId) -> Rows<Option<CommonRow>> {
        super::rows::winning_common(&self.commons(key)?, |f| Ok(self.file(FileId(f))?.rank))
    }
    /// An ODR key's `ODR_CLASSES` rows (kind 47; key). **PROVISIONAL
    /// (U-8's design step)**: the key's numbering and this accessor's
    /// shape follow U-8's ruling (WP-213 follow-up F-5).
    ///
    /// # Errors
    /// `state-corrupt`.
    fn odr_classes(&self, odr_key: u32) -> Rows<Vec<OdrClassRow>>;
    /// An ODR key's `ODR_REFS` rows (kind 48; key). **PROVISIONAL
    /// (U-8's design step)**, as [`Self::odr_classes`].
    ///
    /// # Errors
    /// `state-corrupt`.
    fn odr_refs(&self, odr_key: u32) -> Rows<Vec<OdrRefRow>>;
    /// Every `DIAGS` row (kind 49; **scan**: IN-75a drops every row with
    /// any changed subject, which no single key finds; the table holds
    /// the link's warnings, under 1 MB, IN-18e).
    ///
    /// # Errors
    /// `state-corrupt`.
    fn diags(&self) -> Rows<Vec<DiagRow>>;

    // ---- key-parallel: by SYMBOLS row ----
    /// A `SYMBOLS` row (kind 6; row).
    ///
    /// # Errors
    /// `state-corrupt`.
    fn symbol(&self, key: SymbolId) -> Rows<SymbolRow>;
    /// A `RESOLVED` row (kind 33; row).
    ///
    /// # Errors
    /// `state-corrupt`.
    fn resolved(&self, key: SymbolId) -> Rows<ResolvedRow>;
    /// A `SYMTAB_KEYS` row (kind 50; row).
    ///
    /// # Errors
    /// `state-corrupt`.
    fn symtab_key(&self, key: SymbolId) -> Rows<SymtabKeyRow>;
    /// The global key named `(name, version)` (`version` empty for an
    /// unversioned key; the bytes as `SYMBOLS.name` and `SYMBOLS.version`
    /// name them in `STRINGS`), or `None`: U-3 maps a changed file's
    /// mentions to `SYMBOLS` rows with it. Through `NAMES` (kind 52;
    /// key): every row of [`super::rows::name_hash`]`(name, version)`,
    /// each compared by its `SYMBOLS` row's name and version **bytes**: a
    /// hash alone never identifies a key, and two keys of one hash are
    /// told apart here. A `GONE` key (`SYMBOLS.kind` `sym_kind::GONE`)
    /// keeps its `NAMES` row and is returned: a key that comes back takes
    /// its old `SYMBOLS` row (IN-18e's key-parallel rule: a key's rows are
    /// changed in place by its row; the tombstone is its kind), is not a
    /// new key, and adds no `NAMES` row. Locals have no `NAMES` row.
    ///
    /// # Errors
    /// `state-corrupt`.
    fn find_key(&self, name: &[u8], version: &[u8]) -> Rows<Option<SymbolId>>;
}

/// The state an update reads (WP-211 §C `StateView<'s>`): a [`StateRows`]
/// backing, read through `Deref`. Built by `state5::open` (U-2) over U1's
/// descriptor, which it borrows for `'s`, or by
/// [`super::testing::MemoryState::view`].
pub struct StateView<'s> {
    rows: Box<dyn StateRows + 's>,
}

impl<'s> StateView<'s> {
    /// The view over `rows`.
    #[must_use]
    pub fn new(rows: Box<dyn StateRows + 's>) -> Self {
        Self { rows }
    }
}

impl<'s> std::ops::Deref for StateView<'s> {
    type Target = dyn StateRows + 's;
    fn deref(&self) -> &Self::Target {
        &*self.rows
    }
}

impl std::fmt::Debug for StateView<'_> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("StateView").finish_non_exhaustive()
    }
}

// ---- what U2-U4 found ---------------------------------------------------------

/// What happened to a file (IN-70 U2-U5; IN-71b).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Change {
    /// Its bytes changed (IN-22's hash).
    Changed,
    /// New to the link (a new `FILES` row).
    Appeared,
    /// No longer in the link (`FILES` flag `GONE`).
    Gone,
    /// Unchanged, but read again because the update reaches it (IN-72
    /// item 2a: a group re-elected to it; IN-71's drawn-in files).
    DrawnIn,
    /// A `LAZY` member now extracted (IN-71b; set by U-3's `Extraction`,
    /// the front reads it then).
    Extracted,
    /// An extracted member no longer extracted (IN-71b).
    NoLongerExtracted,
}

/// A key a changed file mentions, by name: the file's mentions name keys
/// by their position in [`ChangedFile::keys`] (the `symbol` column of its
/// fresh [`MentionRow`]s and `KEY_REFS` rows), and U-3's `ResolveDelta`
/// maps each to a `SymbolId` (an existing row by `find_key`, or a new one).
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct KeyName {
    pub name: Vec<u8>,
    /// Empty when unversioned.
    pub version: Vec<u8>,
    /// `format::sym_kind`; a local's key is `(file, index)`.
    pub kind: u8,
    /// Its symbol-table index in the file (a local's identity).
    pub index: u32,
}

/// One changed, appeared, gone, drawn-in, extracted or no longer
/// extracted file and its new rows (IN-70 U2-U4). Written by the join
/// (U-10, U2-U4's front over the new bytes, the same capture functions a
/// full link uses); read by U-3, U-8, U-9, U-2 (its ranges in the patch).
/// A gone or no-longer-extracted file has no new rows.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ChangedFile {
    /// Its `FILES` row; a new id (the state's `FILES` count plus its
    /// position in [`ChangedFiles::appeared`]) for an appeared file.
    pub file: FileId,
    pub change: Change,
    /// Its `FILES` row as the new bytes give it (hash, flags; `rank` and
    /// `round` are U-2's and U-3's).
    pub row: FileRow,
    /// The id its first new section takes: its sections are
    /// `first_section .. first_section + sections.len()`, past the
    /// state's `SECTIONS` count, in [`ChangedFiles::files`] order.
    pub first_section: SectionId,
    /// Its `SECTIONS` rows from the new bytes (place [`crate::format::NONE`]:
    /// U-4 places them).
    pub sections: Vec<SectionRow>,
    /// The keys it mentions, by name.
    pub keys: Vec<KeyName>,
    /// Its `MENTIONS` rows, `symbol` = position in `keys`.
    pub mentions: Vec<MentionRow>,
    /// Its `INDEX` rows (`unit` and section targets in the new id space,
    /// key targets = position in `keys`).
    pub index: Vec<IndexRow>,
    /// Its `EDGES` rows (under `--gc-sections`), ids as `index`.
    pub edges: Vec<EdgeRow>,
    /// Its `EH_RECORDS` rows.
    pub eh_records: Vec<EhRow>,
    /// `.mason.meta` units it carries (IN-71d; 0 for GCC and Clang).
    pub meta_units: u32,
    /// It has debug sections or a split-DWARF skeleton (IN-70: the G
    /// packages' `update-not-implemented (debug)` until G-3).
    pub debug: bool,
    /// Its non-local symbol-table entries named `foo@V` or `foo@@V` (an
    /// `@` in the name), counted from its new bytes as the capture counts
    /// `META.versioned` (WP-215 D8: U-3 gives `update-not-implemented
    /// (versioned)` when it is not 0). 0 for a gone or no longer
    /// extracted file.
    pub versioned: u32,
}

/// U2-U4's result (WP-211 §C `ChangedFiles`). Written by the join; read by
/// every U-package from U-3 on.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ChangedFiles {
    /// Every file that changed in any way, sorted by `file`.
    pub files: Vec<ChangedFile>,
    /// The appeared files' ids in the order their `FILES` rows are
    /// appended.
    pub appeared: Vec<FileId>,
    /// The positional set (IN-28a, U3): files whose `POSITIONAL` flag is
    /// set, read from `FILES` (none of them changed).
    pub positional: Vec<FileId>,
}

// ---- what W5 writes -----------------------------------------------------------

/// A key-keyed table's edits (IN-18e): rows to put (inserted, or changed
/// in place when the key is unchanged) and keys to tombstone (the row's
/// key fields; other fields ignored).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct KeyEdits<R> {
    pub put: Vec<R>,
    pub delete: Vec<R>,
}

impl<R> Default for KeyEdits<R> {
    fn default() -> Self {
        Self { put: Vec::new(), delete: Vec::new() }
    }
}

/// A key-parallel table's edits (IN-18e): rows changed in place by
/// `SYMBOLS` row, rows appended (one per new key, in `ResolveDelta`'s
/// order: the new key's id is the table's count plus its position).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParallelEdits<R> {
    pub set: Vec<(SymbolId, R)>,
    pub append: Vec<R>,
}

impl<R> Default for ParallelEdits<R> {
    fn default() -> Self {
        Self { set: Vec::new(), append: Vec::new() }
    }
}

/// One file's new file-keyed ranges (IN-18e): `None` keeps the range,
/// `Some` rewrites it (in place when it fits its capacity, else in the
/// append area: U-2's).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FileRanges {
    pub file: FileId,
    pub sections: Option<Vec<SectionRow>>,
    pub mentions: Option<Vec<MentionRow>>,
    pub index: Option<Vec<IndexRow>>,
    pub edges: Option<Vec<EdgeRow>>,
    pub frag_at: Option<Vec<FragAtRow>>,
    pub eh_records: Option<Vec<EhRow>>,
    pub symtab: Option<Option<SymtabRow>>,
    /// `REGIONS` (kind 51): rewritten when a region of the file was opened,
    /// grown or moved (IN-78's derived table).
    pub regions: Option<Vec<RegionRow>>,
    /// The file is gone: its ranges are dead (`FILE_INDEX.gone`).
    pub gone: bool,
}

/// The rows W5 writes (WP-211 §C `StatePatch`; IN-46b, IN-78's table of
/// what an update rewrites), by table and class, plus `META`'s new
/// binding. Every id is the update's id space; a `Str` at or past the
/// state's `STRINGS` length is an offset into [`StatePatch::strings`]
/// (appended in order). Assembled by the join (U-10) from every
/// package's result; written by U-2 (`state5::patch`, or `compact`).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct StatePatch {
    /// `META` with the new binding (W5: tuple, build id, leaves,
    /// `output-synced` 1).
    pub meta: Option<MetaRow>,
    // whole tables, rewritten whole when any row differs
    pub probes: Option<Vec<ProbeRow>>,
    pub outsecs: Option<Vec<OutSecRow>>,
    pub chunks: Option<Vec<ChunkRow>>,
    pub order: Option<Vec<u32>>,
    pub segments: Option<Vec<SegmentRow>>,
    pub slots: Option<Vec<SlotRow>>,
    pub sizes: Option<Vec<SizeRow>>,
    pub sym_place: Option<Vec<SymPlaceRow>>,
    pub defined: Option<Vec<DefinedRow>>,
    pub patches: Option<Vec<PatchRow>>,
    pub redirects: Option<Vec<RedirectRow>>,
    pub sized: Option<Vec<SizeRow>>,
    pub layout: Option<LayoutRow>,
    pub leaves: Option<Vec<[u8; 32]>>,
    // append-only
    /// `FILES` rows changed in place (hash, rank, flags, round).
    pub files_set: Vec<(FileId, FileRow)>,
    /// `FILES` rows appended, in `ChangedFiles::appeared` order.
    pub files_append: Vec<FileRow>,
    /// `STAT` rows changed in place (by `STAT` row) and appended.
    pub stat_set: Vec<(u32, StatRow)>,
    pub stat_append: Vec<StatRow>,
    /// Bytes appended to `STRINGS`.
    pub strings: Vec<u8>,
    // file-keyed
    pub ranges: Vec<FileRanges>,
    // key-keyed
    pub key_refs: KeyEdits<KeyRefRow>,
    pub groups: KeyEdits<GroupRow>,
    pub frags: KeyEdits<FragRow>,
    pub holes: KeyEdits<HoleRow>,
    pub commons: KeyEdits<CommonRow>,
    pub odr_classes: KeyEdits<OdrClassRow>,
    pub odr_refs: KeyEdits<OdrRefRow>,
    pub diags: KeyEdits<DiagRow>,
    /// `NAMES` (kind 52): one row put per new key (`ResolveDelta::new_keys`);
    /// never deleted (a `GONE` key keeps its row).
    pub names: KeyEdits<NameRow>,
    // key-parallel
    pub symbols: ParallelEdits<SymbolRow>,
    pub resolved: ParallelEdits<ResolvedRow>,
    pub symtab_keys: ParallelEdits<SymtabKeyRow>,
}
