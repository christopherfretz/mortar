//! The state's format version 5 (`spec/incr.md` IN-18c..IN-18f, IN-46b):
//! read by block and lazily ([`open`]), planned before W1
//! ([`plan_patch`]) and patched in place at W5 ([`patch`]),
//! compacted ([`compact`]), compared canonically ([`canon`], IN-78).
//! WP-213 (U-1) created the files and entry points; filled by U-2
//! (WP-214), which also owns `crate::format` and `crate::capture`. The
//! row types are `crate::update::rows` (U-1's).
//!
//! **The bytes** (IN-18c). An 80-byte header: magic `MORTARSR`, version 5
//! (`u32`), the flags word (`u32` at 12, bit 0 "updating": the only bytes
//! outside every hash), the table count (`u32`), the block size (`u32`,
//! 4096), the file's length (`u64`), the plan hash (16), the directory's
//! L128 (16) and the header's own L128 (16, of bytes 0..12 and 16..64).
//! Then the directory, one [`ENTRY`]-byte entry per table in kind order
//! (kind, row width, rows, capacity, `aux`, `dead`, body offset, list
//! offset, the list's L128), then per table its block list (one L128 per
//! 4 KiB block of its body) followed by its body (`capacity` rows of
//! `width` bytes, unused rows zero). Every integer is little-endian.
//!
//! `rows` is, by class (IN-18e): the rows of a whole, append-only or
//! key-parallel table (bytes for `STRINGS`); the extent (ranges and their
//! slack, the append area's used part) of a file-keyed table; the sorted
//! body plus the used append area of a key-keyed table, whose sorted body
//! is `aux` rows long and which holds `dead` tombstones. `STAT`'s `aux` is
//! the output's row ([`NO_ROW`]: none).

use mortar_core::incr::IncrReason;

use crate::format::{
    ChunkRow, DefinedRow, EhRow, FragAtRow, Hash16, LayoutRow, OutSecRow, PatchRow, ProbeRow, R,
    RedirectRow, Row, SectionRow, SegmentRow, SizeRow, SlotRow, StatRow, StateError, Str,
    SymPlaceRow, W, kind,
};
use crate::update::diags::{DiagRow, Subject};
use crate::update::rows::{
    CloneProbe, CommonRow, CopyRow, EdgeRow, FileIndexRow, FileRow, FileTable, FragRow, GroupRow,
    HoleRow, IndexRow, KeyRefRow, MentionRow, MetaRow, NameRow, RegionRow, ResolvedRow, RowRange,
    SymbolRow, SymtabKeyRow, SymtabRow, key_ref_flags,
};
use crate::update::view::Rows;

pub mod canon;
pub mod compact;
pub mod read;
pub mod write;

pub use canon::{Canon, CanonDiff, canon, canon_of, compare};
pub use compact::compact;
pub use read::{LayoutOf, open, size_report};
pub use write::{PatchPlan, patch, plan_patch};

/// `MORTARSR` (format 4's `MORTARST` is another magic: no reader of one
/// takes the other).
pub const MAGIC: [u8; 8] = *b"MORTARSR";
/// The format version.
pub const VERSION: u32 = 5;
/// The header's size.
pub const HEADER: usize = 80;
/// A directory entry's size.
pub const ENTRY: usize = 72;
/// The block size (IN-18c: 4 KiB).
pub const BLOCK: usize = 4096;
/// "No row" in a `u64` field of the directory.
pub const NO_ROW: u64 = u64::MAX;

/// IN-18e's five classes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Class {
    Whole,
    AppendOnly,
    FileKeyed,
    KeyKeyed,
    KeyParallel,
}

/// Every table of format 5, in file (kind) order, with its class and row
/// width. A kind not here is `state-corrupt` (IN-18e). `ODR_CLASSES` (47)
/// and `ODR_REFS` (48) are not written (supervisor's ruling on WP-220,
/// R-1: provisional until WP-236; kinds 53 and 54 reserved for it).
pub const TABLES: [(u32, Class, usize); 38] = [
    (kind::META, Class::Whole, MetaRow::WIDTH),
    (kind::STAT, Class::AppendOnly, <StatRow as Rec>::WIDTH),
    (kind::FILES, Class::AppendOnly, FileRow::WIDTH),
    (kind::PROBES, Class::Whole, <ProbeRow as Rec>::WIDTH),
    (kind::OUTSECS, Class::Whole, <OutSecRow as Rec>::WIDTH),
    (kind::SYMBOLS, Class::KeyParallel, <SymbolRow as Rec>::WIDTH),
    (kind::SECTIONS, Class::FileKeyed, <SectionRow as Rec>::WIDTH),
    (kind::CHUNKS, Class::Whole, <ChunkRow as Rec>::WIDTH),
    (kind::ORDER, Class::Whole, <u32 as Rec>::WIDTH),
    (kind::SEGMENTS, Class::Whole, <SegmentRow as Rec>::WIDTH),
    (kind::FRAGS, Class::KeyKeyed, FragRow::WIDTH),
    (kind::SLOTS, Class::Whole, <SlotRow as Rec>::WIDTH),
    (kind::EH_RECORDS, Class::FileKeyed, <EhRow as Rec>::WIDTH),
    (kind::SIZES, Class::Whole, <SizeRow as Rec>::WIDTH),
    (kind::SYM_PLACE, Class::Whole, <SymPlaceRow as Rec>::WIDTH),
    (kind::DEFINED, Class::Whole, <DefinedRow as Rec>::WIDTH),
    (kind::PATCHES, Class::Whole, <PatchRow as Rec>::WIDTH),
    (kind::REDIRECTS, Class::Whole, <RedirectRow as Rec>::WIDTH),
    (kind::SIZED, Class::Whole, <SizeRow as Rec>::WIDTH),
    (kind::LAYOUT, Class::Whole, <LayoutRow as Rec>::WIDTH),
    (kind::LEAVES, Class::Whole, <[u8; 32] as Rec>::WIDTH),
    (kind::STRINGS, Class::AppendOnly, 1),
    (kind::MENTIONS, Class::FileKeyed, MentionRow::WIDTH),
    (kind::RESOLVED, Class::KeyParallel, ResolvedRow::WIDTH),
    (kind::INDEX, Class::FileKeyed, <IndexRow as Rec>::WIDTH),
    (kind::HOLES, Class::KeyKeyed, HoleRow::WIDTH),
    (kind::FRAG_AT, Class::FileKeyed, <FragAtRow as Rec>::WIDTH),
    (kind::FILE_INDEX, Class::AppendOnly, FileIndexRow::WIDTH),
    (kind::EDGES, Class::FileKeyed, EdgeRow::WIDTH),
    (kind::SYMTAB, Class::FileKeyed, <SymtabRow as Rec>::WIDTH),
    (kind::GROUPS, Class::KeyKeyed, GroupRow::WIDTH),
    (kind::KEY_REFS, Class::KeyKeyed, KeyRefRow::WIDTH),
    (kind::COMMONS, Class::KeyKeyed, CommonRow::WIDTH),
    (kind::DIAGS, Class::KeyKeyed, DiagRaw::WIDTH),
    (kind::SYMTAB_KEYS, Class::KeyParallel, SymtabKeyRow::WIDTH),
    (kind::REGIONS, Class::FileKeyed, RegionRow::WIDTH),
    (kind::NAMES, Class::KeyKeyed, NameRow::WIDTH),
    (kind::COPIES, Class::KeyKeyed, CopyRow::WIDTH),
];

/// The position of `kind` in [`TABLES`].
#[must_use]
pub fn table_index(k: u32) -> Option<usize> {
    TABLES.iter().position(|t| t.0 == k)
}

/// A table's name for messages (format 4's names, and the new ones).
#[must_use]
pub fn name(k: u32) -> &'static str {
    match k {
        kind::FILE_INDEX => "FILE_INDEX",
        kind::EDGES => "EDGES",
        kind::SYMTAB => "SYMTAB",
        kind::GROUPS => "GROUPS",
        kind::KEY_REFS => "KEY_REFS",
        kind::COMMONS => "COMMONS",
        kind::ODR_CLASSES => "ODR_CLASSES",
        kind::ODR_REFS => "ODR_REFS",
        kind::DIAGS => "DIAGS",
        kind::SYMTAB_KEYS => "SYMTAB_KEYS",
        kind::REGIONS => "REGIONS",
        kind::NAMES => "NAMES",
        kind::COPIES => "COPIES",
        k => kind::name(k),
    }
}

/// IN-18e's capacity for `rows` rows of a table of class `class` written
/// fresh (a file-keyed table's per-file slack is the writer's; this is
/// the append area past its extent).
#[must_use]
pub fn fresh_capacity(class: Class, rows: u64) -> u64 {
    match class {
        Class::Whole => rows + (rows / 8).max(64),
        Class::AppendOnly | Class::FileKeyed | Class::KeyKeyed | Class::KeyParallel => {
            rows + (rows / 64).max(4096)
        }
    }
}

/// A file's range capacity for `count` rows (IN-18e: `count + max(4,
/// count/8)`).
#[must_use]
pub fn range_capacity(count: u32) -> u32 {
    count.saturating_add((count / 8).max(4))
}

pub(crate) fn corrupt(detail: impl Into<String>) -> IncrReason {
    IncrReason::StateCorrupt(detail.into())
}

pub(crate) fn state_corrupt(detail: impl Into<String>) -> StateError {
    StateError::Corrupt(detail.into())
}

// ---- the header and the directory -----------------------------------------------

/// The header's fields.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Header {
    pub flags: u32,
    pub count: u32,
    pub block: u32,
    pub len: u64,
    pub plan_hash: Hash16,
    pub dir_hash: Hash16,
}

impl Header {
    /// The 80 bytes, the header hash computed.
    #[must_use]
    pub fn bytes(&self) -> [u8; HEADER] {
        let mut v = Vec::with_capacity(HEADER);
        let mut w = W(&mut v);
        w.bytes(&MAGIC).u32(VERSION).u32(self.flags).u32(self.count).u32(self.block);
        w.u64(self.len).bytes(&self.plan_hash).bytes(&self.dir_hash);
        let h = header_hash(&v);
        v.extend_from_slice(&h);
        let mut out = [0u8; HEADER];
        out.copy_from_slice(&v);
        out
    }

    /// The header checked: magic, version (format 4: `state-version`),
    /// hash, flags.
    ///
    /// # Errors
    /// `state-corrupt`, `state-version`.
    pub fn parse(b: &[u8]) -> Result<Self, StateError> {
        let b = b.get(..HEADER).ok_or_else(|| state_corrupt("shorter than its header"))?;
        if b.get(..8) == Some(&crate::format::MAGIC[..]) {
            return Err(StateError::Version(format!(
                "format version 4 (MORTARST), this mortar reads {VERSION}"
            )));
        }
        let mut r = R::new(b);
        if r.take::<8>() != MAGIC {
            return Err(state_corrupt("not a link state (magic)"));
        }
        let version = r.u32();
        if version != VERSION {
            return Err(StateError::Version(format!(
                "format version {version}, this mortar reads {VERSION}"
            )));
        }
        let h = Self {
            flags: r.u32(),
            count: r.u32(),
            block: r.u32(),
            len: r.u64(),
            plan_hash: r.h(),
            dir_hash: r.h(),
        };
        if r.h() != header_hash(b) {
            return Err(state_corrupt("header hash differs"));
        }
        if h.flags & !crate::format::FLAG_UPDATING != 0 {
            return Err(state_corrupt(format!("unknown header flags {:#x}", h.flags)));
        }
        if h.block as usize != BLOCK || h.count as usize != TABLES.len() {
            return Err(state_corrupt(format!(
                "{} tables of {}-byte blocks, format 5 has {} of {BLOCK}",
                h.count,
                h.block,
                TABLES.len()
            )));
        }
        Ok(h)
    }
}

/// The header hash: bytes 0..12 and 16..64 (the flags word is outside).
fn header_hash(b: &[u8]) -> Hash16 {
    let mut h = mortar_digest::L128::new();
    h.update(b.get(0..12).unwrap_or_default());
    h.update(b.get(16..64).unwrap_or_default());
    h.finish()
}

/// One directory entry.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Entry {
    pub kind: u32,
    pub width: u32,
    pub rows: u64,
    pub capacity: u64,
    pub aux: u64,
    pub dead: u64,
    pub body: u64,
    pub list: u64,
    pub list_hash: Hash16,
}

impl Entry {
    /// The body's bytes.
    #[must_use]
    pub fn body_len(&self) -> u64 {
        self.capacity.saturating_mul(u64::from(self.width))
    }
    /// Blocks of the body.
    #[must_use]
    pub fn blocks(&self) -> u64 {
        self.body_len().div_ceil(BLOCK as u64)
    }
    /// The list's bytes.
    #[must_use]
    pub fn list_len(&self) -> u64 {
        self.blocks() * 16
    }
    pub(crate) fn put(&self, out: &mut Vec<u8>) {
        let mut w = W(out);
        w.u32(self.kind).u32(self.width).u64(self.rows).u64(self.capacity).u64(self.aux);
        w.u64(self.dead).u64(self.body).u64(self.list).bytes(&self.list_hash);
    }
    fn get(b: &[u8]) -> Self {
        let mut r = R::new(b);
        Self {
            kind: r.u32(),
            width: r.u32(),
            rows: r.u64(),
            capacity: r.u64(),
            aux: r.u64(),
            dead: r.u64(),
            body: r.u64(),
            list: r.u64(),
            list_hash: r.h(),
        }
    }
}

/// The directory's bytes.
#[must_use]
pub fn directory_bytes(dir: &[Entry]) -> Vec<u8> {
    let mut out = Vec::with_capacity(dir.len() * ENTRY);
    for e in dir {
        e.put(&mut out);
    }
    out
}

/// The directory checked against the header (its hash, every entry's
/// kind, width, class bounds and byte ranges inside the file, in order,
/// not overlapping).
///
/// # Errors
/// `state-corrupt`.
#[allow(clippy::many_single_char_names)]
pub fn parse_directory(h: &Header, b: &[u8]) -> Result<Vec<Entry>, StateError> {
    if mortar_digest::l128(b) != h.dir_hash {
        return Err(state_corrupt("directory hash differs"));
    }
    let mut at = (HEADER + TABLES.len() * ENTRY) as u64;
    let mut dir = Vec::with_capacity(TABLES.len());
    for (i, chunk) in b.chunks_exact(ENTRY).enumerate() {
        let e = Entry::get(chunk);
        let (k, class, width) = TABLES[i];
        let n = name(k);
        if e.kind != k || e.width as usize != width {
            return Err(state_corrupt(format!(
                "directory entry {i}: table {} of width {} where {n} of {width}",
                e.kind, e.width
            )));
        }
        let ok = e.rows <= e.capacity
            && match class {
                Class::KeyKeyed => e.aux <= e.rows && e.dead <= e.rows,
                Class::AppendOnly if k == kind::STAT => {
                    (e.aux < e.rows || e.aux == NO_ROW) && e.dead == 0
                }
                _ => e.aux == 0 && e.dead == 0,
            };
        if !ok {
            return Err(state_corrupt(format!("{n}: rows, capacity or counts out of range")));
        }
        let list_end = e.list.checked_add(e.list_len());
        let body_end = e.body.checked_add(e.body_len());
        if e.list != at || list_end != Some(e.body) || body_end.is_none_or(|end| end > h.len) {
            return Err(state_corrupt(format!("{n}: body or list outside the state")));
        }
        at = body_end.unwrap_or(h.len);
        dir.push(e);
    }
    if dir.len() != TABLES.len() || at != h.len {
        return Err(state_corrupt("the directory does not cover the state"));
    }
    Ok(dir)
}

// ---- the rows -----------------------------------------------------------------------

/// A row of format 5: its width and its two conversions (format 4's
/// `Row` where the row is unchanged).
pub(crate) trait Rec: Sized + Clone {
    const WIDTH: usize;
    fn put(&self, w: &mut W<'_>);
    fn get(r: &mut R<'_>) -> Self;
    /// Fields a reader refuses (beyond the pads, which `R` reports).
    fn ok(&self) -> bool {
        true
    }
}

macro_rules! same_as_format4 {
    ($($t:ty),*) => {$(
        impl Rec for $t {
            const WIDTH: usize = <$t as Row>::WIDTH;
            fn put(&self, w: &mut W<'_>) {
                Row::put(self, w);
            }
            fn get(r: &mut R<'_>) -> Self {
                <$t as Row>::get(r)
            }
        }
    )*};
}

same_as_format4!(
    StatRow,
    ProbeRow,
    OutSecRow,
    SymbolRow,
    FragAtRow,
    SectionRow,
    ChunkRow,
    u32,
    SegmentRow,
    SlotRow,
    EhRow,
    SizeRow,
    SymPlaceRow,
    DefinedRow,
    PatchRow,
    RedirectRow,
    LayoutRow,
    [u8; 32]
);

impl Rec for u8 {
    const WIDTH: usize = 1;
    fn put(&self, w: &mut W<'_>) {
        w.u8(*self);
    }
    fn get(r: &mut R<'_>) -> Self {
        r.u8()
    }
}

impl Rec for MetaRow {
    const WIDTH: usize = 208;
    fn put(&self, w: &mut W<'_>) {
        let p = self.clone_probe.unwrap_or_default();
        w.s(self.version).u16(self.machine).u8(self.class).u8(self.output_kind);
        w.u8(self.leaf_fn).u8(self.build_id_len).u8(u8::from(self.output_synced));
        w.u8(u8::from(self.clone_probe.is_some()));
        w.u64(self.file_size).u64(self.sealed_start).u64(self.sealed_end);
        w.bytes(&self.build_id).bytes(&self.boot_id).u32(self.compactions).u32(self.iterations);
        w.u64(p.dev).bytes(&p.fs_type).u64(p.copy_ns_per_mib);
        let t = &self.symtab;
        w.u32(t.sh_info).u32(t.tail_first).u32(t.tail_capacity).u32(self.versioned);
        w.u64(t.symtab_size).u64(t.symtab_slack).u64(t.strtab_size).u64(t.strtab_slack);
    }
    fn get(r: &mut R<'_>) -> Self {
        let (version, machine, class, output_kind) = (r.s(), r.u16(), r.u8(), r.u8());
        let (leaf_fn, build_id_len, synced, probed) = (r.u8(), r.u8(), r.u8(), r.u8());
        let (file_size, sealed_start, sealed_end) = (r.u64(), r.u64(), r.u64());
        let build_id: [u8; 64] = r.take();
        let boot_id: [u8; 16] = r.take();
        let (compactions, iterations) = (r.u32(), r.u32());
        let probe = CloneProbe { dev: r.u64(), fs_type: r.take(), copy_ns_per_mib: r.u64() };
        let (sh_info, tail_first, tail_capacity) = (r.u32(), r.u32(), r.u32());
        let versioned = r.u32();
        let symtab = mortar_core::incr::SymtabTables {
            sh_info,
            tail_first,
            tail_capacity,
            symtab_size: r.u64(),
            symtab_slack: r.u64(),
            strtab_size: r.u64(),
            strtab_slack: r.u64(),
        };
        if synced > 1 || probed > 1 || u32::from(build_id_len) > crate::format::MAX_BUILD_ID {
            r.bad_pad = true;
        }
        if probed == 0 && probe != CloneProbe::default() {
            r.bad_pad = true;
        }
        Self {
            version,
            machine,
            class,
            output_kind,
            leaf_fn,
            file_size,
            sealed_start,
            sealed_end,
            build_id_len,
            build_id,
            boot_id,
            output_synced: synced == 1,
            compactions,
            iterations,
            versioned,
            clone_probe: (probed == 1).then_some(probe),
            symtab,
        }
    }
}

impl Rec for FileRow {
    const WIDTH: usize = 72;
    fn put(&self, w: &mut W<'_>) {
        w.u8(self.kind).u8(self.member).u8(self.flags).pad(1).s(self.path).u32(self.mention);
        w.s(self.member_name).u32(self.member_ordinal).u64(self.base).u64(self.len);
        w.bytes(&self.hash).u64(self.rank).u32(self.round);
    }
    fn get(r: &mut R<'_>) -> Self {
        let (kind, member, flags) = (r.u8(), r.u8(), r.u8());
        r.pad(1);
        Self {
            kind,
            member,
            flags,
            path: r.s(),
            mention: r.u32(),
            member_name: r.s(),
            member_ordinal: r.u32(),
            base: r.u64(),
            len: r.u64(),
            hash: r.h(),
            rank: r.u64(),
            round: r.u32(),
        }
    }
    fn ok(&self) -> bool {
        self.flags & !crate::update::rows::file_flags::ALL == 0 && self.member <= 1
    }
}

impl Rec for FileIndexRow {
    const WIDTH: usize = FileTable::COUNT * 12 + 8;
    fn put(&self, w: &mut W<'_>) {
        for r in &self.ranges {
            w.u32(r.first).u32(r.count).u32(r.capacity);
        }
        w.u32(self.first_global).u8(u8::from(self.gone)).pad(3);
    }
    fn get(r: &mut R<'_>) -> Self {
        let mut row = Self::default();
        for x in &mut row.ranges {
            *x = RowRange { first: r.u32(), count: r.u32(), capacity: r.u32() };
        }
        row.first_global = r.u32();
        let gone = r.u8();
        r.pad(3);
        if gone > 1 {
            r.bad_pad = true;
        }
        row.gone = gone == 1;
        row
    }
    fn ok(&self) -> bool {
        self.ranges.iter().all(|r| r.count <= r.capacity)
    }
}

impl Rec for MentionRow {
    const WIDTH: usize = 12;
    fn put(&self, w: &mut W<'_>) {
        w.u32(self.symbol).u8(self.kind).u8(self.bind).u8(self.typ).u8(self.other);
        w.u32(self.section);
    }
    fn get(r: &mut R<'_>) -> Self {
        Self {
            symbol: r.u32(),
            kind: r.u8(),
            bind: r.u8(),
            typ: r.u8(),
            other: r.u8(),
            section: r.u32(),
        }
    }
}

impl Rec for ResolvedRow {
    const WIDTH: usize = 24;
    fn put(&self, w: &mut W<'_>) {
        w.u8(self.kind).u8(self.typ).u8(self.bind).u8(self.other).u32(self.section);
        w.u64(self.value).u64(self.size);
    }
    fn get(r: &mut R<'_>) -> Self {
        Self {
            kind: r.u8(),
            typ: r.u8(),
            bind: r.u8(),
            other: r.u8(),
            section: r.u32(),
            value: r.u64(),
            size: r.u64(),
        }
    }
}

/// `INDEX` packed to 9 bytes (IN-18e "Shrinking"; format 4's `Row` of the
/// same type is 12).
impl Rec for IndexRow {
    const WIDTH: usize = 9;
    fn put(&self, w: &mut W<'_>) {
        w.u32(self.unit).u8(self.kind).u32(self.target);
    }
    fn get(r: &mut R<'_>) -> Self {
        Self { unit: r.u32(), kind: r.u8(), target: r.u32() }
    }
}

impl Rec for EdgeRow {
    const WIDTH: usize = 17;
    fn put(&self, w: &mut W<'_>) {
        w.u8(self.kind).u32(self.source).u32(self.target).u64(self.offset);
    }
    fn get(r: &mut R<'_>) -> Self {
        Self { kind: r.u8(), source: r.u32(), target: r.u32(), offset: r.u64() }
    }
    fn ok(&self) -> bool {
        self.kind <= crate::update::rows::edge_kind::NONALLOC_SECTION
    }
}

impl Rec for SymtabRow {
    const WIDTH: usize = 36;
    fn put(&self, w: &mut W<'_>) {
        w.u32(self.first).u32(self.count).u32(self.capacity);
        w.u64(self.strtab_offset).u64(self.strtab_len).u64(self.strtab_capacity);
    }
    fn get(r: &mut R<'_>) -> Self {
        Self {
            first: r.u32(),
            count: r.u32(),
            capacity: r.u32(),
            strtab_offset: r.u64(),
            strtab_len: r.u64(),
            strtab_capacity: r.u64(),
        }
    }
}

impl Rec for RegionRow {
    const WIDTH: usize = 29;
    fn put(&self, w: &mut W<'_>) {
        w.u32(self.out).u8(self.category).u32(self.ordinal).u32(self.chunk);
        w.u64(self.offset).u64(self.capacity);
    }
    fn get(r: &mut R<'_>) -> Self {
        Self {
            out: r.u32(),
            category: r.u8(),
            ordinal: r.u32(),
            chunk: r.u32(),
            offset: r.u64(),
            capacity: r.u64(),
        }
    }
}

/// A tomb byte: 0 or 1 (another value: the row is refused).
fn tomb(r: &mut R<'_>) -> bool {
    let t = r.u8();
    if t > 1 {
        r.bad_pad = true;
    }
    t == 1
}

impl Rec for GroupRow {
    const WIDTH: usize = 13;
    fn put(&self, w: &mut W<'_>) {
        w.s(self.signature).u32(self.file).u8(u8::from(self.tomb));
    }
    fn get(r: &mut R<'_>) -> Self {
        Self { signature: r.s(), file: r.u32(), tomb: tomb(r) }
    }
}

impl Rec for CopyRow {
    const WIDTH: usize = 24;
    fn put(&self, w: &mut W<'_>) {
        w.u32(self.key).u32(self.file).u32(self.index).u64(self.size).u8(self.typ);
        w.u8(u8::from(self.tomb)).pad(2);
    }
    fn get(r: &mut R<'_>) -> Self {
        let (key, file, index, size, typ) = (r.u32(), r.u32(), r.u32(), r.u64(), r.u8());
        let row = Self { key, file, index, size, typ, tomb: tomb(r) };
        r.pad(2);
        row
    }
}

impl Rec for KeyRefRow {
    const WIDTH: usize = 12;
    fn put(&self, w: &mut W<'_>) {
        let flags =
            (self.flags & !key_ref_flags::TOMB) | if self.tomb { key_ref_flags::TOMB } else { 0 };
        w.u32(self.key).u32(self.file).u8(self.def).u8(self.bind).u8(self.typ).u8(flags);
    }
    fn get(r: &mut R<'_>) -> Self {
        let (key, file, def, bind, typ, flags) = (r.u32(), r.u32(), r.u8(), r.u8(), r.u8(), r.u8());
        if flags & !(key_ref_flags::TOMB * 2 - 1) != 0 {
            r.bad_pad = true;
        }
        Self {
            key,
            file,
            def,
            bind,
            typ,
            flags: flags & !key_ref_flags::TOMB,
            tomb: flags & key_ref_flags::TOMB != 0,
        }
    }
}

impl Rec for FragRow {
    const WIDTH: usize = 37;
    fn put(&self, w: &mut W<'_>) {
        w.u32(self.out).u32(self.ordinal).bytes(&self.hash).u64(self.offset).u32(self.refs);
        w.u8(u8::from(self.tomb));
    }
    fn get(r: &mut R<'_>) -> Self {
        Self {
            out: r.u32(),
            ordinal: r.u32(),
            hash: r.h(),
            offset: r.u64(),
            refs: r.u32(),
            tomb: tomb(r),
        }
    }
}

impl Rec for HoleRow {
    const WIDTH: usize = 24;
    fn put(&self, w: &mut W<'_>) {
        w.u32(self.chunk).u8(self.kind).u8(u8::from(self.tomb)).pad(2);
        w.u64(self.offset).u64(self.size);
    }
    fn get(r: &mut R<'_>) -> Self {
        let (chunk, kind, t) = (r.u32(), r.u8(), tomb(r));
        r.pad(2);
        Self { chunk, kind, offset: r.u64(), size: r.u64(), tomb: t }
    }
    fn ok(&self) -> bool {
        self.kind <= crate::format::hole_kind::DEAD_CIE
    }
}

impl Rec for CommonRow {
    const WIDTH: usize = 25;
    fn put(&self, w: &mut W<'_>) {
        w.u32(self.key).u32(self.file).u64(self.size).u64(self.align).u8(u8::from(self.tomb));
    }
    fn get(r: &mut R<'_>) -> Self {
        Self { key: r.u32(), file: r.u32(), size: r.u64(), align: r.u64(), tomb: tomb(r) }
    }
}

impl Rec for SymtabKeyRow {
    const WIDTH: usize = 8;
    fn put(&self, w: &mut W<'_>) {
        w.u32(self.index).u32(self.strtab);
    }
    fn get(r: &mut R<'_>) -> Self {
        Self { index: r.u32(), strtab: r.u32() }
    }
}

impl Rec for NameRow {
    const WIDTH: usize = 12;
    fn put(&self, w: &mut W<'_>) {
        w.u64(self.hash).u32(self.row);
    }
    fn get(r: &mut R<'_>) -> Self {
        Self { hash: r.u64(), row: r.u32() }
    }
}

/// A `DIAGS` row as stored (WP-213 §P: its variable-length fields in
/// `STRINGS`): the diagnostic's key, the tomb, and three strings (the
/// subjects, tag-encoded; the code; the text). 39 bytes.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct DiagRaw {
    pub phase: u16,
    pub unit: u32,
    pub seq: u64,
    pub tomb: bool,
    pub subjects: Str,
    pub code: Str,
    pub text: Str,
}

impl Rec for DiagRaw {
    const WIDTH: usize = 39;
    fn put(&self, w: &mut W<'_>) {
        w.u16(self.phase).u32(self.unit).u64(self.seq).u8(u8::from(self.tomb));
        w.s(self.subjects).s(self.code).s(self.text);
    }
    fn get(r: &mut R<'_>) -> Self {
        Self {
            phase: r.u16(),
            unit: r.u32(),
            seq: r.u64(),
            tomb: tomb(r),
            subjects: r.s(),
            code: r.s(),
            text: r.s(),
        }
    }
}

/// The subjects' bytes: per subject a tag byte (0 file, 1 key, 2 ODR key,
/// 3 output section, 4 the link) and its fields.
#[must_use]
pub fn subjects_bytes(subjects: &[Subject]) -> Vec<u8> {
    let mut v = Vec::with_capacity(subjects.len() * 5);
    let mut w = W(&mut v);
    for s in subjects {
        match *s {
            Subject::File(f) => w.u8(0).u32(f.0),
            Subject::Key(k) => w.u8(1).u32(k.0),
            Subject::OdrKey { name, class } => w.u8(2).s(name).u8(class),
            Subject::OutputSection(o) => w.u8(3).u32(o.0),
            Subject::Link => w.u8(4),
        };
    }
    v
}

/// [`subjects_bytes`]' inverse, or `None` for bytes it does not make.
#[must_use]
pub fn subjects_of(b: &[u8]) -> Option<Vec<Subject>> {
    use mortar_core::{FileId, OutSecId, SymbolId};
    let mut out = Vec::new();
    let mut at = 0;
    while at < b.len() {
        let (s, n) = match b[at] {
            0 if b.len() >= at + 5 => (Subject::File(FileId(word(b, at + 1))), 5),
            1 if b.len() >= at + 5 => (Subject::Key(SymbolId(word(b, at + 1))), 5),
            2 if b.len() >= at + 10 => {
                let name = Str { off: word(b, at + 1), len: word(b, at + 5) };
                (Subject::OdrKey { name, class: b[at + 9] }, 10)
            }
            3 if b.len() >= at + 5 => (Subject::OutputSection(OutSecId(word(b, at + 1))), 5),
            4 => (Subject::Link, 1),
            _ => return None,
        };
        out.push(s);
        at += n;
    }
    Some(out)
}

fn word(b: &[u8], at: usize) -> u32 {
    let mut x = [0u8; 4];
    x.copy_from_slice(b.get(at..at + 4).unwrap_or(&[0; 4]));
    u32::from_le_bytes(x)
}

/// A `DIAGS` row's key: the diagnostic's key, then its code and text.
pub(crate) type DiagKey = (u16, u32, u64, Vec<u8>, Vec<u8>);

/// The strings a keyed row's key reads (`GROUPS`' signature, `DIAGS`'
/// code and text).
pub(crate) type StrFn<'a> = &'a dyn Fn(Str) -> Rows<Vec<u8>>;

/// A key-keyed table's row (IN-18e): its key's order, its tomb.
pub(crate) trait Keyed: Rec {
    type K: Ord + Clone + std::fmt::Debug;
    /// The key (strings read through `s`).
    fn key(&self, s: StrFn<'_>) -> Rows<Self::K>;
    fn tomb(&self) -> bool;
    fn set_tomb(&mut self, t: bool);
}

macro_rules! keyed {
    ($t:ty, $k:ty, |$x:ident| $e:expr) => {
        impl Keyed for $t {
            type K = $k;
            fn key(&self, _s: StrFn<'_>) -> Rows<$k> {
                let $x = self;
                Ok($e)
            }
            fn tomb(&self) -> bool {
                self.tomb
            }
            fn set_tomb(&mut self, t: bool) {
                self.tomb = t;
            }
        }
    };
}

keyed!(KeyRefRow, (u32, u32), |x| (x.key, x.file));
keyed!(FragRow, (u32, Hash16, u32), |x| (x.out, x.hash, x.ordinal));
keyed!(HoleRow, (u32, u64), |x| (x.chunk, x.offset));
keyed!(CommonRow, (u32, u32), |x| (x.key, x.file));
keyed!(CopyRow, (u32, u32, u32), |x| (x.key, x.file, x.index));

impl Keyed for GroupRow {
    type K = (Vec<u8>, u32);
    fn key(&self, s: StrFn<'_>) -> Rows<(Vec<u8>, u32)> {
        Ok((s(self.signature)?, self.file))
    }
    fn tomb(&self) -> bool {
        self.tomb
    }
    fn set_tomb(&mut self, t: bool) {
        self.tomb = t;
    }
}

impl Keyed for DiagRaw {
    type K = DiagKey;
    fn key(&self, s: StrFn<'_>) -> Rows<DiagKey> {
        Ok((self.phase, self.unit, self.seq, s(self.code)?, s(self.text)?))
    }
    fn tomb(&self) -> bool {
        self.tomb
    }
    fn set_tomb(&mut self, t: bool) {
        self.tomb = t;
    }
}

/// `NAMES` has no tomb (a `GONE` key keeps its row, IN-18e).
impl Keyed for NameRow {
    type K = (u64, u32);
    fn key(&self, _s: StrFn<'_>) -> Rows<(u64, u32)> {
        Ok((self.hash, self.row))
    }
    fn tomb(&self) -> bool {
        false
    }
    fn set_tomb(&mut self, _t: bool) {}
}

/// Rows of `T` from bytes (`bytes.len()` a multiple of the width), each
/// checked (pads, enumerations).
pub(crate) fn decode_rows<T: Rec>(bytes: &[u8], what: u32, first: u64) -> Rows<Vec<T>> {
    let mut out = Vec::with_capacity(bytes.len() / T::WIDTH.max(1));
    for (i, chunk) in bytes.chunks_exact(T::WIDTH).enumerate() {
        let mut r = R::new(chunk);
        let row = T::get(&mut r);
        if r.bad_pad || !row.ok() {
            return Err(corrupt(format!(
                "{} row {}: field out of range",
                name(what),
                first + i as u64
            )));
        }
        out.push(row);
    }
    Ok(out)
}

/// The bytes of `rows`.
pub(crate) fn encode_rows<T: Rec>(rows: &[T]) -> Vec<u8> {
    let mut v = Vec::with_capacity(rows.len() * T::WIDTH);
    let mut w = W(&mut v);
    for r in rows {
        r.put(&mut w);
    }
    v
}

/// A [`DiagRow`] as stored, its strings appended through `add`.
pub(crate) fn diag_raw(d: &DiagRow, add: &mut dyn FnMut(&[u8]) -> Str) -> DiagRaw {
    DiagRaw {
        phase: d.key.phase,
        unit: d.key.unit,
        seq: d.key.seq,
        tomb: d.tomb,
        subjects: add(&subjects_bytes(&d.subjects)),
        code: add(d.code.as_bytes()),
        text: add(d.text.as_bytes()),
    }
}

/// A stored `DIAGS` row as a [`DiagRow`].
pub(crate) fn diag_row(r: &DiagRaw, s: StrFn<'_>) -> Rows<DiagRow> {
    let subjects = subjects_of(&s(r.subjects)?).ok_or_else(|| corrupt("DIAGS: subjects"))?;
    let text = |x: Str| -> Rows<String> {
        String::from_utf8(s(x)?).map_err(|_| corrupt("DIAGS: a string is not UTF-8"))
    };
    Ok(DiagRow {
        subjects,
        code: text(r.code)?,
        text: text(r.text)?,
        key: mortar_diag::Key { phase: r.phase, unit: r.unit, seq: r.seq },
        tomb: r.tomb,
    })
}
