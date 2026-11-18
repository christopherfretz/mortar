//! The rows of the state's format version 5 (`spec/incr.md` IN-18c..IN-18e)
//! as the update reads and patches them: the rows of the new tables
//! (kinds 41-52) and of the tables format 5 changes (`META`, `FILES`,
//! `SYMBOLS`, `MENTIONS`, `RESOLVED`, `INDEX`, `FRAGS`, `HOLES`). WP-213
//! (U-1) fixed them in this one place: U-2 encodes and decodes them
//! (`crate::state5`), every other package reads them through
//! [`super::view::StateView`] and writes them into a
//! [`super::view::StatePatch`]. Tables format 5 does not change keep
//! [`crate::format`]'s row types (`SectionRow`, `ChunkRow`, `SegmentRow`,
//! `SlotRow`, `EhRow`, `SizeRow`, `SymPlaceRow`, `DefinedRow`, `PatchRow`,
//! `RedirectRow`, `LayoutRow`, `FragAtRow`, `StatRow`, `ProbeRow`,
//! `OutSecRow`).
//!
//! Widths: where IN-18e states a row's bytes, the fields here add up to
//! it (the test below); elsewhere the width is the fields' sum, U-2's to
//! pack. Every `u32` that names a row is that table's row index (the
//! update's id space, `mortar_core::incr`); [`NONE`] is "none". A `Str`
//! is an offset and length in `STRINGS`. The rows a key-keyed table holds
//! carry `tomb` (IN-18e: a deleted row keeps its key, the flag set); a
//! reader never returns a tombstone.

pub use crate::format::{Hash16, NONE, Str};

/// `FILES.flags` in format 5 (IN-18e, IN-71b): `POSITIONAL` (IN-28a) as in
/// format 4, plus `LAZY` (an archive member not extracted: a row, with its
/// defined and common mentions, so that an update can extract it, IN-71b)
/// and `GONE` (removed by an update; its ranges are dead until the
/// compaction).
pub mod file_flags {
    pub const POSITIONAL: u8 = 1;
    pub const LAZY: u8 = 2;
    pub const GONE: u8 = 4;
    pub const ALL: u8 = 7;
}

/// `FILES` (kind 3, append-only; IN-18e): one row per file of the link,
/// archive members included (`LAZY` when not extracted). Key: the row
/// (`FileId`). Written by the full link's capture and U-2 (appended,
/// flags and hash changed in place); read by every U-package.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct FileRow {
    /// Format 4's columns (`format::FileRow`): kind, member, path,
    /// mention, member name and ordinal, base and length in the
    /// container (bytes), content hash (L128).
    pub kind: u8,
    pub member: u8,
    /// [`file_flags`].
    pub flags: u8,
    pub path: Str,
    pub mention: u32,
    pub member_name: Str,
    pub member_ordinal: u32,
    pub base: u64,
    pub len: u64,
    pub hash: Hash16,
    /// The load-order position (IN-18e): the first link writes
    /// `FileId << 20`; an appeared or newly extracted file takes the
    /// integer midpoint of its neighbours' (none left: compaction).
    /// `FileId` order is `rank` order. Written by capture, U-2 (from U-3's
    /// `Extraction` and U-4's front); read by U-9 (the diagnostics' unit,
    /// IN-75a), U-4 (link order).
    pub rank: u64,
    /// The extraction round (IN-71b): an extracted member's number such
    /// that some live file or root with an edge to it has a smaller round;
    /// 0 for a file named on the command line; unused when `LAZY`.
    /// Written by capture and U-3 (`Extraction`); read by U-3.
    pub round: u32,
}

/// `FILE_INDEX` (kind 41, append-only, parallel to `FILES`; IN-18d): per
/// file the range of each file-keyed table. Key: the `FILES` row. Written
/// by capture and U-2; read by U-2's reader only (every file-keyed
/// accessor of `StateView` goes through it).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct FileIndexRow {
    /// One range per file-keyed table, indexed by [`FileTable`].
    pub ranges: [RowRange; FileTable::COUNT],
    /// The file was removed by an update: its ranges are dead.
    pub gone: bool,
}

/// One file's range of a file-keyed table (rows).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct RowRange {
    /// The first row.
    pub first: u32,
    /// Rows in use.
    pub count: u32,
    /// Rows held: `count + max(4, count/8)` when written (IN-18e).
    pub capacity: u32,
}

/// The file-keyed tables (IN-18e), in [`FileIndexRow::ranges`]' order.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum FileTable {
    Sections = 0,
    Mentions = 1,
    Index = 2,
    Edges = 3,
    FragAt = 4,
    EhRecords = 5,
    Symtab = 6,
    Regions = 7,
}

impl FileTable {
    pub const COUNT: usize = 8;
    /// Every file-keyed table.
    pub const ALL: [Self; Self::COUNT] = [
        Self::Sections,
        Self::Mentions,
        Self::Index,
        Self::Edges,
        Self::FragAt,
        Self::EhRecords,
        Self::Symtab,
        Self::Regions,
    ];
    /// Its table kind (`format::kind`).
    #[must_use]
    pub const fn kind(self) -> u32 {
        use crate::format::kind;
        match self {
            Self::Sections => kind::SECTIONS,
            Self::Mentions => kind::MENTIONS,
            Self::Index => kind::INDEX,
            Self::Edges => kind::EDGES,
            Self::FragAt => kind::FRAG_AT,
            Self::EhRecords => kind::EH_RECORDS,
            Self::Symtab => kind::SYMTAB,
            Self::Regions => kind::REGIONS,
        }
    }
}

/// `SYMBOLS.kind` in format 5: format 4's, plus `GONE` (a key no file
/// mentions any more: a tombstone of the key-parallel tables, IN-18e).
pub mod sym_kind {
    pub const GLOBAL: u8 = 0;
    pub const VERSIONED: u8 = 1;
    pub const LOCAL: u8 = 2;
    pub const GONE: u8 = 3;
}

/// `SYMBOLS` (kind 6, key-parallel; IN-18e): one row per key. Key: the row
/// (`SymbolId`), in IN-18e's one row order (globals by first mentioning
/// file's rank and `at_index`, unmentioned keys by name, locals by file
/// rank and index); an update appends. Same columns as format 4's
/// `format::SymbolRow` with [`sym_kind`]. Written by capture and U-2 (from
/// U-3's `ResolveDelta`); read by U-3, U-5, U-8, U-9.
pub type SymbolRow = crate::format::SymbolRow;

/// `MENTIONS` (kind 32, file-keyed; IN-18e "Shrinking"): one row per
/// symbol a file mentions, 12 bytes; the file is the range's. Written by
/// capture and U-2 (a changed file's range rewritten); read by U-3
/// (the old version's mentions, IN-71), U-8, U-9.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct MentionRow {
    /// The `SYMBOLS` row.
    pub symbol: u32,
    /// `format::def_kind`.
    pub kind: u8,
    pub bind: u8,
    pub typ: u8,
    /// `st_other`.
    pub other: u8,
    /// The `SECTIONS` row of its section, or [`NONE`].
    pub section: u32,
}

/// `RESOLVED` (kind 33, key-parallel; IN-18e "Shrinking"): the key's
/// resolution, 24 bytes; its key, file and name are gone (the row is the
/// key's; the section's row gives the file; a common's or an absolute's
/// file is `KEY_REFS`' definer row; signatures are `GROUPS`). Written by
/// capture and U-2 (from `ResolveDelta`); read by U-3, U-4, U-5.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct ResolvedRow {
    /// `format::def_kind` (kinds 6 and 7 are gone: `GROUPS`, `FILES`).
    pub kind: u8,
    pub typ: u8,
    pub bind: u8,
    pub other: u8,
    /// The `SECTIONS` row of the definition, or [`NONE`].
    pub section: u32,
    /// `st_value` (bytes in the section, or the absolute value).
    pub value: u64,
    /// `st_size` (bytes).
    pub size: u64,
}

/// `INDEX` (kind 34, file-keyed, grouped by referring file; IN-18e): the
/// refill index of IN-27, packed to 9 bytes. Same columns as format 4's
/// `format::IndexRow` (`unit`: the referring `SECTIONS` row; `kind`:
/// `format::index_kind`; `target`: a `SYMBOLS` or `SECTIONS` row by
/// kind). Written by capture and U-2; read by U-3 (`refill_set`).
pub type IndexRow = crate::format::IndexRow;

/// `EDGES` (kind 42, file-keyed, `--gc-sections` only; IN-29, IN-71a): one
/// row per stored edge of the liveness enumerator, by IN-29's row kind.
/// Written by capture and U-2 (a changed file's fresh rows); read by U-3
/// (`live`).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct EdgeRow {
    /// [`edge_kind`].
    pub kind: u8,
    /// The source section: its `SECTIONS` row.
    pub source: u32,
    /// By kind: a `SYMBOLS` row (`RELOC`, `START_STOP`, `NONALLOC_KEY`,
    /// `FDE_KEY`) or a `SECTIONS` row (`LOCAL`, `FDE_SECTION`,
    /// `NONALLOC_SECTION`).
    pub target: u32,
    /// `LOCAL` into a split section: the offset (bytes) whose piece is
    /// live; `u64::MAX` for "every piece" (`SHT_REL`); else 0.
    pub offset: u64,
}

/// IN-29's row kinds (image.md I-7's edge kinds that are stored as rows;
/// `linkorder` and `group` are columns of `SECTIONS`, roots are flags).
pub mod edge_kind {
    pub const RELOC: u8 = 0;
    pub const START_STOP: u8 = 1;
    pub const LOCAL: u8 = 2;
    pub const FDE_SECTION: u8 = 3;
    pub const FDE_KEY: u8 = 4;
    pub const NONALLOC_KEY: u8 = 5;
    pub const NONALLOC_SECTION: u8 = 6;
}

/// `SYMTAB` (kind 43, file-keyed; IN-49a): a file's `.symtab` block and
/// `.strtab` region, one row; file 0's range holds the global area's row.
/// The row is `mortar_core::incr::SymtabBlock` (entries and bytes).
/// Written by capture and U-2 (from U-5's `SymtabPlan`); read by U-5.
pub type SymtabRow = mortar_core::incr::SymtabBlock;

/// `REGIONS` (kind 51, file-keyed; IN-18e, IN-72 item 2; WP-213 F-1): a
/// file's regions, one row per `(output section, category, ordinal)`: the
/// first link's region per `(O, c)`, then one per overflow, kept until the
/// compaction. The file is the range's. A decision row of IN-78 (`P_U`'s
/// `region` record, plumb.md §3.2). As `mortar_core::incr::StoredRegion`
/// without `file`. Written by capture and U-2 (from U-4's
/// `LayoutDelta::regions_opened` and its growth steps); read by U-4
/// through `LayoutRows::regions_of`, U-6 (the map's `region` line).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct RegionRow {
    /// The `OUTSECS` row.
    pub out: u32,
    /// The `.text` category (layout.md L-53); 0 when not code.
    pub category: u8,
    /// 0 for the first link's region, then the overflow regions in the
    /// order they were opened.
    pub ordinal: u32,
    /// The `CHUNKS` row of the body chunk it is in.
    pub chunk: u32,
    /// Bytes from the chunk's start.
    pub offset: u64,
    /// Bytes: its sections, their padding and its slack (IN-33).
    pub capacity: u64,
}

/// `GROUPS` (kind 44, key-keyed by signature; IN-71): a COMDAT group's
/// signature and its keeper. Written by capture and U-2 (from
/// `ResolveDelta`); read by U-3 (re-election, IN-72 item 2a), U-8 (a
/// changed keeper is ODR-affected, IN-71d).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct GroupRow {
    /// The signature (bytes in `STRINGS`): the key.
    pub signature: Str,
    /// The keeper's `FILES` row.
    pub keeper: u32,
    pub tomb: bool,
}

/// `KEY_REFS.flags` bits (IN-18e: the attributes resolution reads besides
/// definedness, binding and type).
pub mod key_ref_flags {
    /// `STV_*` in bits 0-1.
    pub const VISIBILITY: u8 = 3;
    /// The mention is a `LAZY` member's (IN-71b).
    pub const LAZY: u8 = 4;
    /// Defined in a section (else absolute, when defined).
    pub const IN_SECTION: u8 = 8;
    /// How U-2 encodes [`super::KeyRefRow::tomb`] in the 12 bytes.
    pub const TOMB: u8 = 16;
}

/// `KEY_REFS` (kind 45, key-keyed by `(key, file)`; IN-18e): one row per
/// (key, mentioning file), 12 bytes: `MENTIONS` transposed, with the
/// lazy members' defined and common mentions and file 0's linker-defined
/// keys. Written by capture and U-2 (from `ResolveDelta`); read by U-3
/// (IN-71's re-resolution of an affected key: its rows only).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct KeyRefRow {
    /// The `SYMBOLS` row.
    pub key: u32,
    /// The `FILES` row of the mentioning file (0: linker-defined).
    pub file: u32,
    /// `format::def_kind` of the mention (undefined, section, absolute,
    /// common, ...).
    pub def: u8,
    pub bind: u8,
    pub typ: u8,
    /// [`key_ref_flags`].
    pub flags: u8,
    pub tomb: bool,
}

/// `FRAGS` (kind 11, key-keyed by identity; IN-72 item 6): a merged
/// fragment's identity, offset and reference count. Written by capture
/// and U-2 (from `LayoutDelta::frags`); read by U-4, U-6. As
/// `mortar_core::incr::StoredFrag` (whose `class` is this `ordinal`) plus
/// `tomb`.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct FragRow {
    pub out: u32,
    pub ordinal: u32,
    pub hash: Hash16,
    /// Bytes from the merged chunk's start.
    pub offset: u64,
    /// Live pieces naming it; 0: a dead range (IN-72 item 6).
    pub refs: u32,
    pub tomb: bool,
}

/// `HOLES` (kind 39, key-keyed by `(chunk, offset)`; IN-3, IN-3a, IN-72):
/// format 4's `format::HoleRow` plus `tomb`. Written by capture and U-2
/// (from `LayoutDelta::holes`); read by U-4, U-6.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct HoleRow {
    pub chunk: u32,
    /// `format::hole_kind`.
    pub kind: u8,
    /// Bytes from the chunk's start.
    pub offset: u64,
    /// Bytes.
    pub size: u64,
    pub tomb: bool,
}

/// `COMMONS` (kind 46, key-keyed by key; IN-71b, IN-72 item 11): each
/// common's winning file, size and alignment. Written by capture and U-2
/// (from `ResolveDelta`); read by U-3 (the largest common, IN-71), U-4
/// (the common's place in its file's `.bss` region).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct CommonRow {
    /// The `SYMBOLS` row.
    pub key: u32,
    /// The `FILES` row of the file whose common wins.
    pub file: u32,
    /// Bytes.
    pub size: u64,
    /// Bytes (a power of two).
    pub align: u64,
    pub tomb: bool,
}

/// `ODR_CLASSES` (kind 47, key-keyed by ODR key; IN-71d): one row per
/// (ODR key, class), 28 bytes. Written by capture and U-2 (from U-8);
/// read by U-8.
///
/// **PROVISIONAL (U-8's design step, WP-213 follow-up F-5)**: how an ODR
/// key is numbered, and whether a file-keyed table must find a changed
/// file's OLD keys (IN-71d) without a scan, are U-8's opening question;
/// this row and [`OdrRefRow`] may be reshaped or joined by another table
/// (a row struct and a kind) after the ruling.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct OdrClassRow {
    /// The ODR key (odr.md O-20's key), numbered by U-8 (see WP-213's
    /// record: the numbering is U-8's and U-2's to settle).
    pub key: u32,
    /// The class's hash (L128 of the rows that fall in it).
    pub hash: Hash16,
    /// The class's layout word (odr.md O-21).
    pub layout: u64,
    pub tomb: bool,
}

/// `ODR_REFS` (kind 48, key-keyed by `(ODR key, file, row kind)`; IN-71d):
/// one row per (ODR key, file, row kind), 12 bytes: the class the file's
/// row falls in. Written by capture and U-2 (from U-8); read by U-8.
/// **PROVISIONAL (U-8's design step)**, as [`OdrClassRow`].
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct OdrRefRow {
    pub key: u32,
    /// The `FILES` row.
    pub file: u32,
    /// odr.md's row kind.
    pub kind: u8,
    /// Its class: the position among the key's `ODR_CLASSES` rows.
    pub class: u16,
    pub tomb: bool,
}

/// `DIAGS` (kind 49, key-keyed; IN-75a): one stored warning. The row type
/// is [`super::diags::DiagRow`] (WP-211 §C puts it in U-9's file); U-2
/// stores it, U-9 reads and recomputes it.
pub type DiagsRow = super::diags::DiagRow;

/// `SYMTAB_KEYS` (kind 50, key-parallel; IN-49a): a global's `.symtab`
/// index and `.strtab` offset, 8 bytes. Written by capture and U-2 (from
/// `SymtabPlan::globals`); read by U-5.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct SymtabKeyRow {
    /// The `.symtab` index; [`NONE`] for a key with no global entry.
    pub index: u32,
    /// Its name's offset in `.strtab`.
    pub strtab: u32,
}

/// `NAMES` (kind 52, key-keyed by `(hash, row)`; IN-18e; WP-213 F-2): the
/// by-name index of `SYMBOLS`' globals (kinds 0 and 1, and `GONE` keys,
/// which keep their row), 12 bytes. Key: `(hash, row)`. A lookup finds
/// every row of the hash and compares the name and version bytes through
/// `SYMBOLS` and `STRINGS`: a hash alone never identifies a key
/// (`StateRows::find_key`). Written by capture and U-2 (a row per new key
/// of `ResolveDelta::new_keys`); read by U-3 through `find_key`.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct NameRow {
    /// [`name_hash`] of the key's name and version.
    pub hash: u64,
    /// The `SYMBOLS` row.
    pub row: u32,
}

/// `NAMES`' hash (IN-18e): `mortar_core::hash::hash_bytes` (model.md §6
/// rule 5) of the bytes of `SYMBOLS.name`, one zero byte, and the bytes of
/// `SYMBOLS.version` (empty for an unversioned key), exactly as `STRINGS`
/// holds them. The one function U-2 (writing) and U-3 (looking up) call.
#[must_use]
pub fn name_hash(name: &[u8], version: &[u8]) -> u64 {
    let mut bytes = Vec::with_capacity(name.len() + 1 + version.len());
    bytes.extend_from_slice(name);
    bytes.push(0);
    bytes.extend_from_slice(version);
    mortar_core::hash::hash_bytes(&bytes)
}

/// W-1's clone probe record in `META` (IN-76a): the output's device, its
/// file system type and the measured copy time.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct CloneProbe {
    /// `st_dev` of the output.
    pub dev: u64,
    /// The mount's file system type (`/proc/self/mountinfo`), as bytes,
    /// zero-padded.
    pub fs_type: [u8; 16],
    /// Nanoseconds per MiB of the timed `std::io::copy` trial.
    pub copy_ns_per_mib: u64,
}

/// `META` (kind 1, whole; IN-18c, IN-77c, IN-76a): format 4's columns with
/// the build id inline, plus the boot id, `output-synced`, the compaction
/// count, the clone probe and the symbol tables' sizes. Written by capture
/// and U-2 (W5's binding); read by U1 (the join), U-5, W-1.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MetaRow {
    pub version: Str,
    pub machine: u16,
    pub class: u8,
    pub output_kind: u8,
    pub leaf_fn: u8,
    /// Bytes of the output.
    pub file_size: u64,
    pub sealed_start: u64,
    pub sealed_end: u64,
    /// The build id's length (bytes, at most `format::MAX_BUILD_ID`) and
    /// bytes (IN-18e: not in `STRINGS`).
    pub build_id_len: u8,
    pub build_id: [u8; 64],
    /// `/proc/sys/kernel/random/boot_id`'s 16 bytes (IN-77c).
    pub boot_id: [u8; 16],
    /// 0 from a full link, 1 from W5/C4 (IN-77c).
    pub output_synced: bool,
    /// Compactions since the last full link (a history field, IN-78).
    pub compactions: u32,
    /// W-1's record, when probed.
    pub clone_probe: Option<CloneProbe>,
    /// `.symtab`/`.strtab` sizes and slack (IN-49a).
    pub symtab: mortar_core::incr::SymtabTables,
}

impl Default for MetaRow {
    fn default() -> Self {
        Self {
            version: Str::default(),
            machine: 0,
            class: 0,
            output_kind: 0,
            leaf_fn: 0,
            file_size: 0,
            sealed_start: 0,
            sealed_end: 0,
            build_id_len: 0,
            build_id: [0; 64],
            boot_id: [0; 16],
            output_synced: false,
            compactions: 0,
            clone_probe: None,
            symtab: mortar_core::incr::SymtabTables::default(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // spec: incr.md IN-18e (the widths it states, as the fields' sum)
    #[test]
    fn stated_row_widths() {
        let mention = 4 + 1 + 1 + 1 + 1 + 4;
        assert_eq!(mention, 12);
        let resolved = 1 + 1 + 1 + 1 + 4 + 8 + 8;
        assert_eq!(resolved, 24);
        let index = 4 + 1 + 4;
        assert_eq!(index, 9);
        let key_ref = 4 + 4 + 1 + 1 + 1 + 1; // tomb is a bit of flags
        assert_eq!(key_ref, 12);
        let symtab_key = 4 + 4;
        assert_eq!(symtab_key, 8);
        let odr_class = 4 + 16 + 8;
        assert_eq!(odr_class, 28);
        let odr_ref = 4 + 4 + 1 + 1 + 2; // key, file, kind, tomb, class
        assert_eq!(odr_ref, 12);
        let edge = 1 + 4 + 4 + 8;
        assert_eq!(edge, 17);
        let name = 8 + 4;
        assert_eq!(name, 12);
        // The structs' fields are the ones counted (a field added fails
        // here first).
        let MentionRow { symbol: _, kind: _, bind: _, typ: _, other: _, section: _ } =
            MentionRow::default();
        let ResolvedRow { kind: _, typ: _, bind: _, other: _, section: _, value: _, size: _ } =
            ResolvedRow::default();
        let KeyRefRow { key: _, file: _, def: _, bind: _, typ: _, flags: _, tomb: _ } =
            KeyRefRow::default();
        let SymtabKeyRow { index: _, strtab: _ } = SymtabKeyRow::default();
        let OdrClassRow { key: _, hash: _, layout: _, tomb: _ } = OdrClassRow::default();
        let OdrRefRow { key: _, file: _, kind: _, class: _, tomb: _ } = OdrRefRow::default();
        let EdgeRow { kind: _, source: _, target: _, offset: _ } = EdgeRow::default();
        let NameRow { hash: _, row: _ } = NameRow::default();
        assert_eq!(FileTable::ALL.len(), FileTable::COUNT);
    }

    // spec: incr.md IN-18e (`NAMES`' hash: name, a zero byte, version)
    #[test]
    fn name_hash_separates_name_and_version() {
        assert_ne!(name_hash(b"foo", b"V1"), name_hash(b"foo", b"V2"));
        assert_ne!(name_hash(b"foo", b""), name_hash(b"fo", b"o"));
        assert_eq!(name_hash(b"foo", b""), mortar_core::hash::hash_bytes(b"foo\0"));
    }
}
