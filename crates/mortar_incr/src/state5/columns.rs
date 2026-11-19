//! Every column of every format-5 table, in row order, with what it names
//! (WP-247 item 4, the F-1 class): ONE list, checked against IN-18e's
//! table list ([`super::TABLES`]: the widths of a table's columns sum to
//! its row width, so a column added to a row and not here fails
//! `crates/mortar_incr/tests/state5_columns.rs`), and the list that test
//! walks to show that [`super::compact::normalize`], [`super::write::encode`]
//! with its decode, [`super::compact::apply`] (the compaction's patch)
//! keep what each reference names. A column that names a row of another
//! table is a [`To`] other than [`To::Value`]; the test has one identity
//! reader per such column and fails on a listed reference without one.
//!
//! Runs of plain values are one entry (`"..."` names them); padding is a
//! value.

use crate::format::kind;

/// What a column names.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum To {
    /// No row of another table: a value, a hash, an offset in the output,
    /// padding.
    Value,
    /// A `Str`: 8 bytes, an offset and a length in `STRINGS`.
    Strings,
    /// A `FILES` row.
    Files,
    /// A `SYMBOLS` row (a key-parallel table's row number).
    Symbols,
    /// A `SECTIONS` row.
    Sections,
    /// A `CHUNKS` row.
    Chunks,
    /// An `OUTSECS` row.
    OutSecs,
    /// A `PATCHES` row.
    Patches,
    /// Rows of the file-keyed tables (`FILE_INDEX`'s ranges, rebuilt by
    /// the writer from the rows' files).
    FileKeyedRows,
    /// The table depends on the row's kind; the text says how.
    ByKind(&'static str),
    /// A `Str` holding a list whose entries name rows; the text says which.
    List(&'static str),
}

/// One column (or one run of value columns).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Column {
    pub name: &'static str,
    pub width: usize,
    pub to: To,
}

const fn c(name: &'static str, width: usize, to: To) -> Column {
    Column { name, width, to }
}

const V: To = To::Value;
const S: To = To::Strings;

/// The columns of every table of [`super::TABLES`], in its order.
pub const COLUMNS: [(u32, &[Column]); 38] = [
    (kind::META, &[c("version", 8, S), c("...", 188, V)]),
    (kind::STAT, &[c("tag..pad", 8, V), c("path", 8, S), c("dev..hash", 64, V), c("fstype", 8, S)]),
    (
        kind::FILES,
        &[
            c("kind..pad", 4, V),
            c("path", 8, S),
            c("mention", 4, V),
            c("member_name", 8, S),
            c("member_ordinal..stack", 65, V),
            c("properties", 8, S),
        ],
    ),
    (kind::PROBES, &[c("path", 8, S), c("present..pad", 4, V)]),
    (kind::OUTSECS, &[c("name", 8, S), c("ordinal", 4, V)]),
    (
        kind::SYMBOLS,
        &[
            c("kind..pad", 4, V),
            c("file", 4, To::ByKind("a LOCAL's file: FILES; else NONE")),
            c("ordinal", 4, V),
            c("name", 8, S),
            c("version", 8, S),
            c("at_file", 4, To::Files),
            c("at_index", 4, V),
        ],
    ),
    (
        kind::SECTIONS,
        &[
            c("file", 4, To::Files),
            c("anchor_kind, kind, bits, pad", 4, V),
            c("anchor", 8, S),
            c("name", 8, S),
            c("ordinal..offset", 40, V),
            c("place_chunk", 4, To::Chunks),
            c("place_offset", 8, V),
        ],
    ),
    (
        kind::CHUNKS,
        &[
            c("tag..pad", 4, V),
            c("name", 8, S),
            c("ordinal", 4, V),
            c("out", 4, To::OutSecs),
            c("part..shndx", 52, V),
        ],
    ),
    (kind::ORDER, &[c("chunk", 4, To::Chunks)]),
    (kind::SEGMENTS, &[c("...", 48, V)]),
    (kind::FRAGS, &[c("out", 4, To::OutSecs), c("class", 4, V), c("hash..tomb", 33, V)]),
    (kind::SLOTS, &[c("kind..pad", 4, V), c("symbol", 4, To::Symbols), c("refs", 4, V)]),
    (kind::SIZES, &[c("chunk", 4, To::Chunks), c("size", 8, V)]),
    (
        kind::SYM_PLACE,
        &[
            c("kind..pad", 4, V),
            c("symbol", 4, To::Symbols),
            c("chunk", 4, To::Chunks),
            c("offset..pad", 20, V),
        ],
    ),
    (
        kind::DEFINED,
        &[
            c("symbol", 4, To::Symbols),
            c("kind..pad", 4, V),
            c("chunk", 4, To::Chunks),
            c("value", 8, V),
        ],
    ),
    (
        kind::PATCHES,
        &[
            c("chunk", 4, To::Chunks),
            c("kind..pad", 4, V),
            c("offset", 8, V),
            c("symbol", 4, To::ByKind("THUNK, THUNK_PLT: SYMBOLS; E843419: NONE")),
            c("section", 4, To::ByKind("E843419: SECTIONS; else NONE")),
            c("value", 8, V),
        ],
    ),
    (
        kind::REDIRECTS,
        &[c("section", 4, To::Sections), c("patch", 4, To::Patches), c("offset", 8, V)],
    ),
    (kind::SIZED, &[c("chunk", 4, To::Chunks), c("size", 8, V)]),
    (kind::LAYOUT, &[c("...", 32, V)]),
    (kind::LEAVES, &[c("...", 32, V)]),
    (kind::STRINGS, &[c("byte", 1, V)]),
    (
        kind::MENTIONS,
        &[c("symbol", 4, To::Symbols), c("kind..other", 4, V), c("section", 4, To::Sections)],
    ),
    (
        kind::RESOLVED,
        &[c("kind..other", 4, V), c("section", 4, To::Sections), c("value..facts", 20, V)],
    ),
    (
        kind::INDEX,
        &[
            c("unit", 4, To::Sections),
            c("kind", 1, V),
            c("target", 4, To::ByKind("GLOBAL, SLOT: SYMBOLS; else SECTIONS")),
        ],
    ),
    (kind::HOLES, &[c("chunk", 4, To::Chunks), c("kind..size", 20, V)]),
    (
        kind::FILE_INDEX,
        &[
            c("ranges", 96, To::FileKeyedRows),
            c("first_global", 4, V),
            c("gone..pad", 4, V),
            c("locals.first", 4, To::Symbols),
            c("locals.count", 4, V),
        ],
    ),
    (
        kind::EDGES,
        &[
            c("kind", 1, V),
            c("source", 4, To::Sections),
            c("target", 4, To::ByKind("edge_kind::target: Section: SECTIONS; Key: SYMBOLS")),
            c("offset", 4, V),
        ],
    ),
    (kind::SYMTAB, &[c("...", 36, V)]),
    (kind::GROUPS, &[c("signature", 8, S), c("file", 4, To::Files), c("tomb", 1, V)]),
    (kind::KEY_REFS, &[c("key", 4, To::Symbols), c("file", 4, To::Files), c("def..demand", 6, V)]),
    (kind::COMMONS, &[c("key", 4, To::Symbols), c("file", 4, To::Files), c("size..tomb", 17, V)]),
    (
        kind::DIAGS,
        &[
            c("phase", 2, V),
            c("unit", 4, To::ByKind("a file phase (diags::file_unit): FILES; else a value")),
            c("seq..tomb", 9, V),
            c("subjects", 8, To::List("File: FILES; Key: SYMBOLS; OdrKey's name: STRINGS")),
            c("code", 8, S),
            c("text", 8, S),
            c("refs", 8, To::List("each reference's file: FILES")),
            c("ref_count", 8, V),
        ],
    ),
    (kind::SYMTAB_KEYS, &[c("index", 4, V), c("strtab", 4, V)]),
    (
        kind::REGIONS,
        &[
            c("out", 4, To::OutSecs),
            c("category..ordinal", 5, V),
            c("chunk", 4, To::Chunks),
            c("offset..capacity", 16, V),
        ],
    ),
    (kind::NAMES, &[c("hash", 8, V), c("row", 4, To::Symbols)]),
    (kind::COPIES, &[c("key", 4, To::Symbols), c("file", 4, To::Files), c("index..pad", 16, V)]),
    (kind::FRAG_USE, &[c("file", 4, To::Files), c("out", 4, To::OutSecs), c("class..hash", 20, V)]),
    // `out` is the class (`.debug_str`, `.debug_line_str`), not an
    // `OUTSECS` row; `offset` is in the output section.
    (kind::DEBUG_STRS, &[c("out", 1, V), c("hash", 8, V), c("offset", 4, V)]),
];

/// The listed references: `(table, column)` of every column that names a
/// row of another table.
pub fn references() -> impl Iterator<Item = (u32, &'static Column)> {
    COLUMNS.iter().flat_map(|(k, cols)| cols.iter().filter(|c| c.to != To::Value).map(|c| (*k, c)))
}
