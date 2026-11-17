//! The reader (`meta.md` §3, §4, §7, §8).
//!
//! A [`FileReader`] reads the containers of one file in section-index
//! order (MT-4) into one sequence of [`Unit`]s. Each unit is validated
//! whole when it is opened (MT-14): framing, every string reference of the
//! kinds this reader knows, and their row order. A malformed unit is
//! skipped with exactly one [`Finding::Malformed`] and the units after it
//! are read, as long as its `unit_size` still says where the next one
//! starts; a fault that hides the next unit's position (bad magic, a
//! `unit_size` that is not usable) ends the section with one finding.
//! A unit of another major is skipped with [`Finding::Version`], once per
//! file (MT-35). Unknown kinds are skipped silently, a later `version` of
//! a kind is read through the v1 prefix of each row, a later minor through
//! `header_size` (MT-25, MT-36).
//!
//! Nothing is copied: a unit borrows the section's bytes and rows are
//! decoded when asked for. Nothing panics on any input (the property test
//! `tests/mutate.rs`), and the result is a function of the bytes alone.

use core::fmt;
use core::ops::Range;

use crate::{BLOCK_HEADER_SIZE, HEADER_SIZE, MAGIC, MAJOR, SECTION_ALIGN, kind};

/// The section header fields MT-1 constrains beyond MT-6's predicate.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SectionAttrs {
    /// The section's index in its object (for the findings).
    pub index: u32,
    pub addralign: u64,
    pub entsize: u64,
    pub link: u32,
    pub info: u32,
}

impl SectionAttrs {
    /// The attributes MT-1 asks for, at `index`.
    #[must_use]
    pub const fn conforming(index: u32) -> Self {
        Self { index, addralign: SECTION_ALIGN, entsize: 0, link: 0, info: 0 }
    }
}

/// What a finding is about.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Finding {
    /// `meta-malformed` (MT-14, MT-1): `unit` is the offset of the unit in
    /// the section (`None` when the fault is the section's).
    Malformed { section: u32, unit: Option<u64>, what: Malformation },
    /// `meta-version` (MT-35): a unit of another major, at `unit`.
    Version { section: u32, unit: u64, major: u16 },
}

impl Finding {
    /// The section the finding names.
    #[must_use]
    pub const fn section(&self) -> u32 {
        match self {
            Self::Malformed { section, .. } | Self::Version { section, .. } => *section,
        }
    }

    /// The stable id of the diagnostic (`spec/diag.md` §3.1).
    #[must_use]
    pub const fn id(&self) -> &'static str {
        match self {
            Self::Malformed { .. } => "meta-malformed",
            Self::Version { .. } => "meta-version",
        }
    }

    /// The `reason` of `meta-malformed`: where in the section and what,
    /// with the rule. Empty for `meta-version` (its message carries the
    /// major alone).
    #[must_use]
    pub fn reason(&self) -> String {
        match self {
            Self::Malformed { unit: Some(at), what, .. } => {
                format!("unit at offset {at}: {what} ({})", what.rule())
            }
            Self::Malformed { unit: None, what, .. } => format!("{what} ({})", what.rule()),
            Self::Version { .. } => String::new(),
        }
    }
}

/// How a container or a unit breaks the format. Each is one sentence of
/// MT-1, MT-10, MT-11, MT-12 or MT-14.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Malformation {
    /// MT-1: `sh_addralign`, `sh_entsize`, `sh_link` or `sh_info` is not
    /// what MT-1 says; the section is not read.
    Attribute { field: &'static str, value: u64 },
    /// MT-10: neither the magic nor eight zero bytes at an 8-aligned
    /// offset. The rest of the section is not read (nothing says where a
    /// next unit would start).
    Magic,
    /// MT-10: fewer bytes left than a unit header or padding word. The
    /// rest of the section is not read.
    Truncated { left: u64 },
    /// MT-14: `unit_size` is not a multiple of 8, is smaller than a
    /// header, or runs past the section. The rest of the section is not
    /// read.
    UnitSize { size: u32 },
    /// MT-10, MT-36: `header_size` smaller than 32, not a multiple of 8,
    /// or past the unit.
    HeaderSize { size: u32 },
    /// MT-11, MT-14: block `index` (header at `at`) runs past the unit, or
    /// its `block_size`/`row_size` break MT-11's rules.
    Block { index: u32, at: u32, fault: BlockFault },
    /// MT-11: kinds not strictly increasing (a kind twice, or unsorted).
    BlockOrder { kind: u16, after: u16 },
    /// MT-12: the string table runs past the unit, overlaps the header,
    /// or does not start with the empty string.
    Strtab { offset: u32, size: u32 },
    /// MT-14: a string reference past the table, or a string without NUL.
    String { kind: u16, row: u32, offset: u32 },
    /// MT-11, MT-14 (MT-26: names unique): row `row` of `kind` is out of
    /// order with the row before it.
    Unsorted { kind: u16, row: u32 },
    /// `spec/odr.md` O-19, MT-14: the kind 4 entry at byte `offset` of
    /// the kind 4 block's body (a kind 3 row's `details`) is not one.
    /// `block` is the kind 4 block's offset in its unit (`None`: the unit
    /// has no kind 4 block). Found only when the entry is read, on a
    /// finding (O-19); the unit's rows stay in use.
    Details { block: Option<u32>, offset: u32, fault: crate::details::Fault },
}

/// Why a block's framing is wrong.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BlockFault {
    /// Its header or `block_size` runs past the unit.
    PastUnit,
    /// `block_size` is smaller than the header or not a multiple of 8.
    Size(u32),
    /// `row_size` is not a multiple of 8.
    RowSize(u32),
    /// `row_count × row_size` runs past the block.
    Rows,
    /// A known kind with `version` 0 (versions start at 1).
    Version(u16),
    /// A known kind whose `row_size` is smaller than its v1 row.
    ShortRow(u32),
}

impl Malformation {
    /// The rule the fault breaks.
    #[must_use]
    pub const fn rule(&self) -> &'static str {
        match self {
            Self::Attribute { .. } => "meta.md MT-1",
            Self::Magic | Self::Truncated { .. } => "meta.md MT-10",
            Self::UnitSize { .. }
            | Self::Block { .. }
            | Self::String { .. }
            | Self::Unsorted { .. } => "meta.md MT-14",
            Self::HeaderSize { .. } => "meta.md MT-10, MT-36",
            Self::BlockOrder { .. } => "meta.md MT-11",
            Self::Strtab { .. } => "meta.md MT-12",
            Self::Details { .. } => "odr.md O-19, meta.md MT-14",
        }
    }

    /// Whether the reader stops reading the section after this fault.
    #[must_use]
    pub const fn ends_section(&self) -> bool {
        matches!(
            self,
            Self::Attribute { .. } | Self::Magic | Self::Truncated { .. } | Self::UnitSize { .. }
        )
    }
}

impl fmt::Display for Malformation {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Attribute { field, value } => {
                let want = if *field == "sh_addralign" { SECTION_ALIGN } else { 0 };
                write!(f, "section {field} is {value}, not {want}")
            }
            Self::Magic => f.write_str("neither a unit nor zero padding"),
            Self::Truncated { left } => write!(f, "{left} trailing bytes are not a unit"),
            Self::UnitSize { size } => {
                write!(f, "unit_size {size} is not a multiple of 8, too small, or past the section")
            }
            Self::HeaderSize { size } => {
                write!(f, "header_size {size} is below 32, not a multiple of 8, or past the unit")
            }
            Self::Block { index, at, fault } => {
                write!(f, "block {index} at offset {at}: ")?;
                match fault {
                    BlockFault::PastUnit => f.write_str("runs past the unit"),
                    BlockFault::Size(s) => {
                        write!(f, "block_size {s} is below 16 or not a multiple of 8")
                    }
                    BlockFault::RowSize(s) => write!(f, "row_size {s} is not a multiple of 8"),
                    BlockFault::Rows => f.write_str("row_count x row_size runs past the block"),
                    BlockFault::Version(v) => write!(f, "version {v} (versions start at 1)"),
                    BlockFault::ShortRow(s) => {
                        write!(f, "row_size {s} is smaller than the kind's version 1 row")
                    }
                }
            }
            Self::BlockOrder { kind, after } => {
                write!(f, "block of kind {kind:#06x} after kind {after:#06x}")
            }
            Self::Strtab { offset, size } => write!(
                f,
                "string table at {offset} of {size} bytes is past the unit, in its header, or \
                 does not start with NUL"
            ),
            Self::String { kind, row, offset } => write!(
                f,
                "row {row} of kind {kind:#06x}: string offset {offset} is past the table or \
                 has no NUL"
            ),
            Self::Unsorted { kind, row } => {
                write!(f, "row {row} of kind {kind:#06x} is out of order")
            }
            Self::Details { block: None, offset, .. } => write!(
                f,
                "a kind 3 row names the kind 4 entry at offset {offset}, and the unit has no \
                 kind 4 block"
            ),
            Self::Details { block: Some(block), offset, fault } => {
                write!(f, "kind 4 block at offset {block}: entry at offset {offset}: {fault}")
            }
        }
    }
}

// ---- little-endian access that cannot panic --------------------------------

#[inline]
fn u16_at(b: &[u8], at: usize) -> u16 {
    match b.get(at..at.wrapping_add(2)) {
        Some(&[x, y]) => u16::from_le_bytes([x, y]),
        _ => 0,
    }
}

#[inline]
pub(crate) fn u32_at(b: &[u8], at: usize) -> u32 {
    match b.get(at..at.wrapping_add(4)) {
        Some(&[a, b2, c, d]) => u32::from_le_bytes([a, b2, c, d]),
        _ => 0,
    }
}

#[inline]
pub(crate) fn u64_at(b: &[u8], at: usize) -> u64 {
    let mut out = [0u8; 8];
    if let Some(s) = b.get(at..at.wrapping_add(8)) {
        out.copy_from_slice(s);
    }
    u64::from_le_bytes(out)
}

#[inline]
fn bytes16_at(b: &[u8], at: usize) -> [u8; 16] {
    let mut out = [0u8; 16];
    if let Some(s) = b.get(at..at.wrapping_add(16)) {
        out.copy_from_slice(s);
    }
    out
}

// ---- rows -------------------------------------------------------------------

/// A row layout: decoded from the v1 prefix of a row (MT-25).
pub trait Row: Sized + Copy {
    /// The kind whose block holds these rows.
    const KIND: u16;
    /// The v1 row size.
    const SIZE: u32;
    /// Decode the first [`Self::SIZE`] bytes of `row`.
    fn decode(row: &[u8]) -> Self;
}

/// Kind 1 (MT-26).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SummaryRow {
    pub name: u32,
    pub value: u32,
    pub flags: u32,
}

impl Row for SummaryRow {
    const KIND: u16 = kind::SUMMARY;
    const SIZE: u32 = kind::SUMMARY_ROW;
    fn decode(r: &[u8]) -> Self {
        Self { name: u32_at(r, 0), value: u32_at(r, 4), flags: u32_at(r, 8) }
    }
}

/// Kind 2 (`spec/odr.md` O-15).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DefinitionRow {
    pub key: u64,
    pub hash: [u8; 16],
    /// 0: anchored.
    pub name: u32,
    pub class: u16,
    pub flags: u16,
    pub module: u32,
    pub file: u32,
    pub line: u32,
}

impl Row for DefinitionRow {
    const KIND: u16 = kind::DEFINITIONS;
    const SIZE: u32 = kind::DEFINITION_ROW;
    fn decode(r: &[u8]) -> Self {
        Self {
            key: u64_at(r, 0),
            hash: bytes16_at(r, 8),
            name: u32_at(r, 24),
            class: u16_at(r, 28),
            flags: u16_at(r, 30),
            module: u32_at(r, 32),
            file: u32_at(r, 36),
            line: u32_at(r, 40),
        }
    }
}

/// Kind 3 (`spec/odr.md` O-17).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TypeRow {
    pub key: u64,
    pub hash: [u8; 16],
    pub layout: u64,
    pub name: u32,
    pub size: u32,
    pub align: u16,
    pub flags: u16,
    pub file: u32,
    pub line: u32,
    /// [`crate::NO_DETAILS`]: none.
    pub details: u32,
}

impl Row for TypeRow {
    const KIND: u16 = kind::TYPES;
    const SIZE: u32 = kind::TYPE_ROW;
    fn decode(r: &[u8]) -> Self {
        Self {
            key: u64_at(r, 0),
            hash: bytes16_at(r, 8),
            layout: u64_at(r, 24),
            name: u32_at(r, 32),
            size: u32_at(r, 36),
            align: u16_at(r, 40),
            flags: u16_at(r, 42),
            file: u32_at(r, 44),
            line: u32_at(r, 48),
            details: u32_at(r, 52),
        }
    }
}

/// Kind 5 (`spec/odr.md` O-35).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DeclarationRow {
    pub key: u64,
    pub ty: u64,
    pub size: u64,
    /// 0: anchored.
    pub name: u32,
    pub type_name: u32,
    pub flags: u16,
}

impl Row for DeclarationRow {
    const KIND: u16 = kind::DECLARATIONS;
    const SIZE: u32 = kind::DECLARATION_ROW;
    fn decode(r: &[u8]) -> Self {
        Self {
            key: u64_at(r, 0),
            ty: u64_at(r, 8),
            size: u64_at(r, 16),
            name: u32_at(r, 24),
            type_name: u32_at(r, 28),
            flags: u16_at(r, 32),
        }
    }
}

/// The rows of one block: a view over the unit's bytes.
#[derive(Debug, Clone, Copy)]
pub struct Rows<'a, R> {
    bytes: &'a [u8],
    row_size: usize,
    count: usize,
    _row: core::marker::PhantomData<R>,
}

impl<'a, R: Row> Rows<'a, R> {
    const fn empty() -> Self {
        Self { bytes: &[], row_size: 0, count: 0, _row: core::marker::PhantomData }
    }

    #[must_use]
    pub const fn len(&self) -> usize {
        self.count
    }

    #[must_use]
    pub const fn is_empty(&self) -> bool {
        self.count == 0
    }

    /// The `row_size` of the block (larger than [`Row::SIZE`] for a later
    /// version).
    #[must_use]
    pub const fn row_size(&self) -> usize {
        self.row_size
    }

    /// Row `i`; `None` past the end.
    #[must_use]
    pub fn get(&self, i: usize) -> Option<R> {
        if i >= self.count {
            return None;
        }
        let at = i * self.row_size;
        self.bytes.get(at..at + self.row_size).map(R::decode)
    }

    /// Row `i`'s bytes, `row_size` of them (for a dumper).
    #[must_use]
    pub fn raw(&self, i: usize) -> Option<&'a [u8]> {
        if i >= self.count {
            return None;
        }
        let at = i * self.row_size;
        self.bytes.get(at..at + self.row_size)
    }

    pub fn iter(&self) -> impl ExactSizeIterator<Item = R> + '_ {
        (0..self.count).map(|i| self.get(i).unwrap_or_else(|| R::decode(&[])))
    }

    /// The index of the first row for which `before` is false, by binary
    /// search; the rows must be partitioned by `before` (they are, for a
    /// predicate on the sort key: MT-11).
    pub fn partition_point(&self, mut before: impl FnMut(&R) -> bool) -> usize {
        let (mut lo, mut hi) = (0, self.count);
        while lo < hi {
            let mid = lo + (hi - lo) / 2;
            match self.get(mid) {
                Some(r) if before(&r) => lo = mid + 1,
                _ => hi = mid,
            }
        }
        lo
    }
}

/// Rows whose sort starts with a key (kinds 2, 3, 5).
pub trait Keyed {
    fn key(&self) -> u64;
}

impl Keyed for DefinitionRow {
    fn key(&self) -> u64 {
        self.key
    }
}

impl Keyed for TypeRow {
    fn key(&self) -> u64 {
        self.key
    }
}

impl Keyed for DeclarationRow {
    fn key(&self) -> u64 {
        self.key
    }
}

impl<R: Row + Keyed> Rows<'_, R> {
    /// The rows whose key is `key`, by binary search (MT-11). A key is an
    /// index, never an identity (MT-13): the caller confirms names.
    pub fn with_key(&self, key: u64) -> Range<usize> {
        let lo = self.partition_point(|r| r.key() < key);
        let hi = self.partition_point(|r| r.key() <= key);
        lo..hi
    }
}

// ---- units ------------------------------------------------------------------

/// A block of a known kind, located in its unit.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Located {
    version: u16,
    /// Start of the block's header within the unit.
    at: u32,
    block_size: u32,
    row_size: u32,
    row_count: u32,
}

/// A block header as the unit holds it (every kind, known or not).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BlockHeader {
    pub kind: u16,
    pub version: u16,
    /// The block's offset in its unit.
    pub at: u32,
    pub block_size: u32,
    pub row_size: u32,
    pub row_count: u32,
}

/// One validated unit (MT-10): its header, its blocks and string table.
#[derive(Debug, Clone, Copy)]
pub struct Unit<'a> {
    /// The section it was read from, and its offset there.
    pub section: u32,
    pub offset: u64,
    bytes: &'a [u8],
    pub minor: u16,
    pub header_size: u32,
    pub producer: u16,
    pub flags: u16,
    block_count: u32,
    strtab: &'a [u8],
    strtab_offset: u32,
    /// Kinds 1 to 5, at index `kind - 1`.
    known: [Option<Located>; 5],
}

impl<'a> Unit<'a> {
    /// The unit's bytes (`unit_size` of them).
    #[must_use]
    pub const fn bytes(&self) -> &'a [u8] {
        self.bytes
    }

    /// The string table's offset in the unit and its bytes.
    #[must_use]
    pub const fn strtab(&self) -> (u32, &'a [u8]) {
        (self.strtab_offset, self.strtab)
    }

    /// The string at `offset` without its NUL (MT-12). Every reference of
    /// a known kind was checked when the unit was opened; an offset no row
    /// holds gives the empty string if it is not a string.
    #[must_use]
    pub fn string(&self, offset: u32) -> &'a [u8] {
        string_at(self.strtab, offset).unwrap_or(b"")
    }

    /// Every block header, known or not, in unit order.
    pub fn blocks(&self) -> impl Iterator<Item = BlockHeader> + 'a {
        let b = self.bytes;
        let mut at = self.header_size as usize;
        (0..self.block_count).map_while(move |_| {
            let h = b.get(at..at + BLOCK_HEADER_SIZE as usize)?;
            let out = BlockHeader {
                kind: u16_at(h, 0),
                version: u16_at(h, 2),
                at: at as u32,
                block_size: u32_at(h, 4),
                row_size: u32_at(h, 8),
                row_count: u32_at(h, 12),
            };
            at += out.block_size as usize;
            Some(out)
        })
    }

    fn located(&self, k: u16) -> Option<Located> {
        let i = usize::from(k).checked_sub(1)?;
        *self.known.get(i)?
    }

    /// The version of the block of kind `k` (1-5), if the unit has one.
    #[must_use]
    pub fn version(&self, k: u16) -> Option<u16> {
        self.located(k).map(|l| l.version)
    }

    fn rows<R: Row>(&self) -> Rows<'a, R> {
        let Some(l) = self.located(R::KIND) else { return Rows::empty() };
        let start = (l.at + BLOCK_HEADER_SIZE) as usize;
        let len = l.row_count as usize * l.row_size as usize;
        match self.bytes.get(start..start + len) {
            Some(bytes) => Rows {
                bytes,
                row_size: l.row_size as usize,
                count: l.row_count as usize,
                _row: core::marker::PhantomData,
            },
            None => Rows::empty(),
        }
    }

    /// Kind 1's rows.
    #[must_use]
    pub fn summary(&self) -> Rows<'a, SummaryRow> {
        self.rows()
    }

    /// Kind 2's rows.
    #[must_use]
    pub fn definitions(&self) -> Rows<'a, DefinitionRow> {
        self.rows()
    }

    /// Kind 3's rows.
    #[must_use]
    pub fn types(&self) -> Rows<'a, TypeRow> {
        self.rows()
    }

    /// Kind 5's rows.
    #[must_use]
    pub fn declarations(&self) -> Rows<'a, DeclarationRow> {
        self.rows()
    }

    /// Kind 4's block body (everything after its 16-byte header; a kind 4
    /// block has no rows, `spec/odr.md` O-19), unvalidated: entries are
    /// read one at a time by [`Self::detail_entry`].
    #[must_use]
    pub fn details(&self) -> Option<&'a [u8]> {
        let l = self.located(kind::DETAILS)?;
        self.bytes.get((l.at + BLOCK_HEADER_SIZE) as usize..(l.at + l.block_size) as usize)
    }

    /// The kind 4 entry at byte `offset` of the kind 4 body (a kind 3
    /// row's `details`, `spec/odr.md` O-19), validated now: nothing of
    /// kind 4 is read when the unit is opened (MT-42: read on a finding
    /// only). `Ok(None)`: the block is of a later version than 1, which
    /// this reader skips (MT-25).
    ///
    /// # Errors
    /// [`Malformation::Details`]: no kind 4 block, an offset that is not a
    /// multiple of 8, an entry or its items past the block, or a string
    /// of an item of a known kind past the table or without NUL.
    pub fn detail_entry(
        &self,
        offset: u32,
    ) -> Result<Option<crate::details::Entry<'a>>, Malformation> {
        let Some(l) = self.located(kind::DETAILS) else {
            return Err(Malformation::Details {
                block: None,
                offset,
                fault: crate::details::Fault::NoBlock,
            });
        };
        if l.version != 1 {
            return Ok(None);
        }
        let body = self.details().unwrap_or_default();
        crate::details::entry(body, self.strtab, offset)
            .map(Some)
            .map_err(|fault| Malformation::Details { block: Some(l.at), offset, fault })
    }

    /// Kind 1's value of `name` (MT-26), by binary search.
    #[must_use]
    pub fn summary_value(&self, name: &[u8]) -> Option<(&'a [u8], u32)> {
        let rows = self.summary();
        let i = rows.partition_point(|r| self.string(r.name) < name);
        let r = rows.get(i)?;
        (self.string(r.name) == name).then(|| (self.string(r.value), r.flags))
    }
}

pub(crate) fn string_at(table: &[u8], offset: u32) -> Option<&[u8]> {
    if offset == 0 {
        return Some(b"");
    }
    let rest = table.get(offset as usize..)?;
    let end = rest.iter().position(|&c| c == 0)?;
    rest.get(..end)
}

/// Open one unit (`u` is exactly `unit_size` bytes, major 1).
fn open(section: u32, offset: u64, u: &[u8]) -> Result<Unit<'_>, Malformation> {
    let len = u.len();
    let header_size = u32_at(u, 12);
    if (len as u64) < u64::from(HEADER_SIZE)
        || header_size < HEADER_SIZE
        || !header_size.is_multiple_of(8)
        || header_size as usize > len
    {
        return Err(Malformation::HeaderSize { size: header_size });
    }
    let block_count = u32_at(u, 16);
    let strtab_offset = u32_at(u, 20);
    let strtab_size = u32_at(u, 24);
    let strtab: &[u8] = if strtab_offset == 0 && strtab_size == 0 {
        &[]
    } else {
        let end = u64::from(strtab_offset) + u64::from(strtab_size);
        let ok = strtab_offset >= header_size
            && end <= len as u64
            && (strtab_size == 0 || u.get(strtab_offset as usize) == Some(&0));
        if !ok {
            return Err(Malformation::Strtab { offset: strtab_offset, size: strtab_size });
        }
        u.get(strtab_offset as usize..end as usize).unwrap_or(&[])
    };
    let mut unit = Unit {
        section,
        offset,
        bytes: u,
        minor: u16_at(u, 6),
        header_size,
        producer: u16_at(u, 28),
        flags: u16_at(u, 30),
        block_count,
        strtab,
        strtab_offset,
        known: [None; 5],
    };
    // Blocks (MT-11). Each is at least 16 bytes, so the loop ends within
    // `len / 16` steps whatever `block_count` says.
    let mut at = header_size as usize;
    let mut last: Option<u16> = None;
    for index in 0..block_count {
        let fault = |fault| Malformation::Block { index, at: at as u32, fault };
        let Some(h) = u.get(at..at + BLOCK_HEADER_SIZE as usize) else {
            return Err(fault(BlockFault::PastUnit));
        };
        let k = u16_at(h, 0);
        let version = u16_at(h, 2);
        let block_size = u32_at(h, 4);
        let row_size = u32_at(h, 8);
        let row_count = u32_at(h, 12);
        if block_size < BLOCK_HEADER_SIZE || !block_size.is_multiple_of(8) {
            return Err(fault(BlockFault::Size(block_size)));
        }
        if at as u64 + u64::from(block_size) > len as u64 {
            return Err(fault(BlockFault::PastUnit));
        }
        if !row_size.is_multiple_of(8) {
            return Err(fault(BlockFault::RowSize(row_size)));
        }
        if u64::from(row_count) * u64::from(row_size) > u64::from(block_size - BLOCK_HEADER_SIZE) {
            return Err(fault(BlockFault::Rows));
        }
        if let Some(prev) = last
            && k <= prev
        {
            return Err(Malformation::BlockOrder { kind: k, after: prev });
        }
        last = Some(k);
        if let Some(v1) = kind::row_size(k) {
            if version == 0 {
                return Err(fault(BlockFault::Version(version)));
            }
            if row_count > 0 && row_size < v1 {
                return Err(fault(BlockFault::ShortRow(row_size)));
            }
            unit.known[usize::from(k) - 1] =
                Some(Located { version, at: at as u32, block_size, row_size, row_count });
        }
        at += block_size as usize;
    }
    check_summary(&unit)?;
    check_keyed(&unit, &unit.definitions(), |r| [r.name, r.module, r.file], |r| r.name)?;
    check_keyed(&unit, &unit.types(), |r| [r.name, r.file, 0], |r| r.name)?;
    check_declarations(&unit)?;
    Ok(unit)
}

fn string_ok(unit: &Unit<'_>, k: u16, row: usize, offset: u32) -> Result<(), Malformation> {
    match string_at(unit.strtab, offset) {
        Some(_) => Ok(()),
        None => Err(Malformation::String { kind: k, row: row as u32, offset }),
    }
}

/// Kind 1: strings valid, names strictly increasing (sorted, unique).
fn check_summary(unit: &Unit<'_>) -> Result<(), Malformation> {
    let rows = unit.summary();
    let mut prev: Option<&[u8]> = None;
    for (i, r) in rows.iter().enumerate() {
        string_ok(unit, kind::SUMMARY, i, r.name)?;
        string_ok(unit, kind::SUMMARY, i, r.value)?;
        let name = unit.string(r.name);
        if prev.is_some_and(|p| p >= name) {
            return Err(Malformation::Unsorted { kind: kind::SUMMARY, row: i as u32 });
        }
        prev = Some(name);
    }
    Ok(())
}

/// Kinds 2 and 3: strings valid, sorted by key then name bytes.
fn check_keyed<R: Row + Keyed>(
    unit: &Unit<'_>,
    rows: &Rows<'_, R>,
    strings: impl Fn(&R) -> [u32; 3],
    name: impl Fn(&R) -> u32,
) -> Result<(), Malformation> {
    let mut prev: Option<(u64, &[u8])> = None;
    for (i, r) in rows.iter().enumerate() {
        for s in strings(&r) {
            string_ok(unit, R::KIND, i, s)?;
        }
        let cur = (r.key(), unit.string(name(&r)));
        if prev.is_some_and(|p| p > cur) {
            return Err(Malformation::Unsorted { kind: R::KIND, row: i as u32 });
        }
        prev = Some(cur);
    }
    Ok(())
}

/// Kind 5: strings valid, sorted by key then flags.
fn check_declarations(unit: &Unit<'_>) -> Result<(), Malformation> {
    let mut prev: Option<(u64, u16)> = None;
    for (i, r) in unit.declarations().iter().enumerate() {
        string_ok(unit, kind::DECLARATIONS, i, r.name)?;
        string_ok(unit, kind::DECLARATIONS, i, r.type_name)?;
        let cur = (r.key, r.flags);
        if prev.is_some_and(|p| p > cur) {
            return Err(Malformation::Unsorted { kind: kind::DECLARATIONS, row: i as u32 });
        }
        prev = Some(cur);
    }
    Ok(())
}

/// The units and findings of one file's containers (MT-4: all of them, in
/// section-index order, as one sequence).
#[derive(Debug, Default)]
pub struct FileReader<'a> {
    pub units: Vec<Unit<'a>>,
    pub findings: Vec<Finding>,
    version_reported: bool,
}

impl<'a> FileReader<'a> {
    #[must_use]
    pub fn new() -> Self {
        Self { units: Vec::new(), findings: Vec::new(), version_reported: false }
    }

    /// Read one container: `bytes` are its contents, `attrs` its header.
    /// The caller has applied [`crate::is_container`] and calls this in
    /// section-index order.
    pub fn section(&mut self, attrs: SectionAttrs, bytes: &'a [u8]) {
        let section = attrs.index;
        for (field, value, want) in [
            ("sh_addralign", attrs.addralign, SECTION_ALIGN),
            ("sh_entsize", attrs.entsize, 0),
            ("sh_link", u64::from(attrs.link), 0),
            ("sh_info", u64::from(attrs.info), 0),
        ] {
            if value != want {
                self.findings.push(Finding::Malformed {
                    section,
                    unit: None,
                    what: Malformation::Attribute { field, value },
                });
                return;
            }
        }
        let mut off = 0usize;
        while off < bytes.len() {
            let rest = &bytes[off..];
            let at = off as u64;
            let malformed = |what| Finding::Malformed { section, unit: Some(at), what };
            if rest.len() < 8 {
                self.findings.push(malformed(Malformation::Truncated { left: rest.len() as u64 }));
                return;
            }
            if rest[..8] == [0; 8] {
                off += 8;
                continue;
            }
            if rest[..4] != MAGIC {
                self.findings.push(malformed(Malformation::Magic));
                return;
            }
            if rest.len() < 12 {
                self.findings.push(malformed(Malformation::Truncated { left: rest.len() as u64 }));
                return;
            }
            let size = u32_at(rest, 8);
            if !size.is_multiple_of(8) || size < 16 || size as usize > rest.len() {
                self.findings.push(malformed(Malformation::UnitSize { size }));
                return;
            }
            let unit = &rest[..size as usize];
            off += size as usize;
            let major = u16_at(unit, 4);
            if major != MAJOR {
                if !self.version_reported {
                    self.version_reported = true;
                    self.findings.push(Finding::Version { section, unit: at, major });
                }
                continue;
            }
            match open(section, at, unit) {
                Ok(u) => self.units.push(u),
                Err(what) => self.findings.push(malformed(what)),
            }
        }
    }
}

/// Read one container on its own (a file with one section; tests and the
/// dumper): the units and the findings.
#[must_use]
pub fn read_section(attrs: SectionAttrs, bytes: &[u8]) -> (Vec<Unit<'_>>, Vec<Finding>) {
    let mut r = FileReader::new();
    r.section(attrs, bytes);
    (r.units, r.findings)
}
