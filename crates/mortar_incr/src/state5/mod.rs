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
    ChunkRow, DefinedRow, Hash16, LayoutRow, OutSecRow, PatchRow, ProbeRow, R, RedirectRow, Row,
    SectionRow, SegmentRow, SizeRow, StatRow, StateError, Str, SymPlaceRow, W, kind,
};
use crate::update::diags::{DiagRef, DiagRow, Subject};
use crate::update::rows::{
    CommonRow, CopyRow, DebugStrRow, EdgeRow, FileIndexRow, FileRow, FileTable, FragRow,
    FragUseRow, GroupRow, HoleRow, IndexRow, KeyRefRow, LocalRange, MentionRow, MetaRow, NameRow,
    RegionRow, ResolvedRow, RowRange, SlotRow, SymbolRow, SymtabKeyRow, SymtabRow, key_ref_flags,
    sym_kind,
};
use crate::update::view::Rows;

pub mod canon;
pub mod columns;
pub mod compact;
pub mod read;
pub mod write;

pub use canon::{Canon, CanonDiff, canon, canon_of, compare};
pub use compact::{check_capture, compact, eh_columns, live_keys};
pub use read::{LayoutOf, open, size_report};
pub use write::{PatchPlan, patch, plan_patch};

/// `MORTARSR` (format 4's `MORTARST` is another magic: no reader of one
/// takes the other).
pub const MAGIC: [u8; 8] = *b"MORTARSR";
/// The format version.
/// 6 since WP-257a (`SECTIONS`' kind, bits and a place-less row's output
/// section: a version-5 state is refused, never read with zeros). 7 since
/// WP-261 (`FILES.round` written by the full link's capture: a version-6
/// state holds round 0 for every extracted member, which IN-71b reads as
/// a `--whole-archive` member; it is refused, `state-version`, never read).
/// 8 since WP-262 (`SYMTAB_KEYS` written by the full link's capture: a
/// version-7 state holds `NONE` for every key, which IN-49a reads as a
/// key without a global entry; it is refused, `state-version`). 9 since
/// WP-263 (`SYMTAB`'s rows and `META.symtab` written by the full link's
/// capture, and `META.gnu_warning_orphans`: a version-8 state holds no
/// block and zero sizes, which IN-49a's plan reads as an empty table; it
/// is refused, `state-version`). 10 since WP-264 (`REGIONS` written by
/// the full link's capture, file 0's `SYMTAB` range with the internal
/// file's block as its second row, `META.boot_id` written, `META`'s clone
/// probe removed: a version-9 state holds no region, which IN-72 item 2
/// reads as a file with none, and a 228-byte `META`; it is refused,
/// `state-version`). 11 since WP-265 (`DIAGS` written by the full link's
/// capture: a version-10 state of a link that warned holds no row, which
/// IN-75a reads as a link without warnings; it is refused,
/// `state-version`).
pub const VERSION: u32 = 11;
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
/// `EH_RECORDS` (13) and `FRAG_AT` (40) are format 4's only (WP-239: the
/// `FILES` eh columns and `FRAG_USE`, 56, in their place); `FILE_INDEX`'s
/// `EhRecords` range is always empty in format 5. `DEBUG_STRS` (57,
/// WP-228 R-2, seam 8; WP-247) is key-keyed with no tomb, as `NAMES`.
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
    (kind::SLOTS, Class::Whole, SlotRow::WIDTH),
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
    (kind::FRAG_USE, Class::FileKeyed, FragUseRow::WIDTH),
    (kind::DEBUG_STRS, Class::KeyKeyed, DebugStrRow::WIDTH),
];

/// The file-keyed tables of format 5 ([`FileTable::ALL`] without
/// `EhRecords`, format 4's only; its `FILE_INDEX` range is all zero).
pub(crate) const FILE_TABLES: [FileTable; FileTable::COUNT - 1] = [
    FileTable::Sections,
    FileTable::Mentions,
    FileTable::Index,
    FileTable::Edges,
    FileTable::FragUse,
    FileTable::Symtab,
    FileTable::Regions,
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
        kind::DEBUG_STRS => "DEBUG_STRS",
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
    ChunkRow,
    u32,
    SegmentRow,
    SizeRow,
    SymPlaceRow,
    DefinedRow,
    PatchRow,
    RedirectRow,
    LayoutRow,
    [u8; 32]
);

/// `SECTIONS` (format 4's 76 bytes) with WP-257a's `kind` and `bits` in
/// two of format 4's three pad bytes, and a place-less row's output
/// section in `place_offset` ([`crate::format::section_facts`]): a code
/// or bit a reader does not know is refused here, the `OUTSECS` range by
/// the facts' reader (it needs the table's count).
impl Rec for SectionRow {
    const WIDTH: usize = <SectionRow as Row>::WIDTH;
    fn put(&self, w: &mut W<'_>) {
        Row::put(self, w);
    }
    fn get(r: &mut R<'_>) -> Self {
        <SectionRow as Row>::get(r)
    }
    fn ok(&self) -> bool {
        crate::format::section_codes_ok(self)
    }
}

/// `SYMBOLS` (format 4's 36 bytes) with `option` in byte 1, one of format
/// 4's three pad bytes (WP-214 seam 6, ruling 2: 0 or 1; another value,
/// or a set byte 2 or 3, is refused).
impl Rec for SymbolRow {
    const WIDTH: usize = 36;
    fn put(&self, w: &mut W<'_>) {
        w.u8(self.kind).u8(u8::from(self.option)).pad(2).u32(self.file).u32(self.ordinal);
        w.s(self.name).s(self.version).u32(self.at_file).u32(self.at_index);
    }
    fn get(r: &mut R<'_>) -> Self {
        let kind = r.u8();
        let option = r.u8();
        r.pad(2);
        if option > 1 {
            r.bad_pad = true;
        }
        Self {
            kind,
            file: r.u32(),
            ordinal: r.u32(),
            name: r.s(),
            version: r.s(),
            at_file: r.u32(),
            at_index: r.u32(),
            option: option == 1,
        }
    }
    fn ok(&self) -> bool {
        // the bit is a global's (a local, a `GONE` row: never set)
        !self.option || matches!(self.kind, sym_kind::GLOBAL | sym_kind::VERSIONED)
    }
}

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
    const WIDTH: usize = 196;
    fn put(&self, w: &mut W<'_>) {
        w.s(self.version).u16(self.machine).u8(self.class).u8(self.output_kind);
        w.u8(self.leaf_fn).u8(self.build_id_len).u8(u8::from(self.output_synced));
        // A pad byte (zero): the clone probe's flag before WP-264.
        w.u8(0);
        w.u64(self.file_size).u64(self.sealed_start).u64(self.sealed_end);
        w.bytes(&self.build_id).bytes(&self.boot_id).u32(self.compactions).u32(self.iterations);
        let t = &self.symtab;
        w.u32(t.sh_info).u32(t.tail_first).u32(t.tail_capacity).u32(self.versioned);
        w.u64(t.symtab_size).u64(t.symtab_slack).u64(t.strtab_size).u64(t.strtab_slack);
        w.bytes(&self.live_hash).u32(self.gnu_warning_orphans);
    }
    fn get(r: &mut R<'_>) -> Self {
        let (version, machine, class, output_kind) = (r.s(), r.u16(), r.u8(), r.u8());
        let (leaf_fn, build_id_len, synced, pad) = (r.u8(), r.u8(), r.u8(), r.u8());
        let (file_size, sealed_start, sealed_end) = (r.u64(), r.u64(), r.u64());
        let build_id: [u8; 64] = r.take();
        let boot_id: [u8; 16] = r.take();
        let (compactions, iterations) = (r.u32(), r.u32());
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
        let live_hash: [u8; 16] = r.take();
        let gnu_warning_orphans = r.u32();
        if synced > 1 || pad != 0 || u32::from(build_id_len) > crate::format::MAX_BUILD_ID {
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
            symtab,
            live_hash,
            gnu_warning_orphans,
        }
    }
}

/// `FILES`: format 4's columns, the `.eh_frame` range (WP-239), then
/// seam 6's `eh_fdes` (`u32`), `stack` (`u8`, `file_stack`) and
/// `properties` (`Str`, `rows::encode_properties`): 97 bytes.
impl Rec for FileRow {
    const WIDTH: usize = 97;
    fn put(&self, w: &mut W<'_>) {
        w.u8(self.kind).u8(self.member).u8(self.flags).pad(1).s(self.path).u32(self.mention);
        w.s(self.member_name).u32(self.member_ordinal).u64(self.base).u64(self.len);
        w.bytes(&self.hash).u64(self.rank).u32(self.round);
        w.u32(self.eh_offset).u32(self.eh_capacity).u32(self.eh_records);
        w.u32(self.eh_fdes).u8(self.stack).s(self.properties);
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
            eh_offset: r.u32(),
            eh_capacity: r.u32(),
            eh_records: r.u32(),
            eh_fdes: r.u32(),
            stack: r.u8(),
            properties: r.s(),
        }
    }
    fn ok(&self) -> bool {
        self.flags & !crate::update::rows::file_flags::ALL == 0
            && self.member <= 1
            && (self.eh_records == 0 || self.eh_capacity > 0)
            && self.eh_fdes <= self.eh_records
            && self.stack <= crate::update::rows::file_stack::EXEC
    }
}

/// `FILE_INDEX`: per file-keyed table `(first, count, capacity)`, then
/// `first_global`, `gone` (pad 3) and seam 6's `locals` (`first`,
/// `count`).
impl Rec for FileIndexRow {
    const WIDTH: usize = FileTable::COUNT * 12 + 16;
    fn put(&self, w: &mut W<'_>) {
        for r in &self.ranges {
            w.u32(r.first).u32(r.count).u32(r.capacity);
        }
        w.u32(self.first_global).u8(u8::from(self.gone)).pad(3);
        w.u32(self.locals.first).u32(self.locals.count);
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
        row.locals = LocalRange { first: r.u32(), count: r.u32() };
        row
    }
    fn ok(&self) -> bool {
        self.ranges.iter().all(|r| r.count <= r.capacity)
            && self.ranges[FileTable::EhRecords as usize] == RowRange::default()
            && self.locals.first.checked_add(self.locals.count).is_some()
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
    const WIDTH: usize = 28;
    fn put(&self, w: &mut W<'_>) {
        w.u8(self.kind).u8(self.typ).u8(self.bind).u8(self.other).u32(self.section);
        w.u64(self.value).u64(self.size).u32(self.facts);
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
            facts: r.u32(),
        }
    }
}

/// `SLOTS` (format 4's 8 bytes plus `refs`, WP-239).
impl Rec for SlotRow {
    const WIDTH: usize = 12;
    fn put(&self, w: &mut W<'_>) {
        w.u8(self.kind).pad(3).u32(self.symbol).u32(self.refs);
    }
    fn get(r: &mut R<'_>) -> Self {
        let kind = r.u8();
        r.pad(3);
        Self { kind, symbol: r.u32(), refs: r.u32() }
    }
}

/// `FRAG_USE` (kind 56, file-keyed): a file's fragments by key.
impl Rec for FragUseRow {
    const WIDTH: usize = 28;
    fn put(&self, w: &mut W<'_>) {
        w.u32(self.file).u32(self.out).u32(self.class).bytes(&self.hash);
    }
    fn get(r: &mut R<'_>) -> Self {
        Self { file: r.u32(), out: r.u32(), class: r.u32(), hash: r.h() }
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

/// `EDGES` (WP-238 seam 9, S1): 13 bytes. `ok`: the kind exists, a
/// `Key`/`Zero`/`Code` row has offset 0, a `Zero` row target 0, a
/// `REFUSE` row a known code (F4, P0-1).
impl Rec for EdgeRow {
    const WIDTH: usize = 13;
    fn put(&self, w: &mut W<'_>) {
        w.u8(self.kind).u32(self.source).u32(self.target).u32(self.offset);
    }
    fn get(r: &mut R<'_>) -> Self {
        Self { kind: r.u8(), source: r.u32(), target: r.u32(), offset: r.u32() }
    }
    fn ok(&self) -> bool {
        use crate::update::rows::edge_kind::{self, Target, refuse};
        match edge_kind::target(self.kind) {
            None => false,
            Some(Target::Key) => self.offset == 0,
            Some(Target::Section) => true,
            Some(Target::Zero) => self.target == 0 && self.offset == 0,
            Some(Target::Code) => (1..=refuse::LAST).contains(&self.target) && self.offset == 0,
        }
    }
}

/// The reader's check of an `EDGES` row (`Rec::ok`), for the capture: a
/// row the reader would refuse is never written.
pub(crate) fn edge_row_ok(row: &EdgeRow) -> bool {
    row.ok()
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
    const WIDTH: usize = 14;
    fn put(&self, w: &mut W<'_>) {
        let flags =
            (self.flags & !key_ref_flags::TOMB) | if self.tomb { key_ref_flags::TOMB } else { 0 };
        w.u32(self.key).u32(self.file).u8(self.def).u8(self.bind).u8(self.typ).u8(flags);
        w.u16(self.demand);
    }
    fn get(r: &mut R<'_>) -> Self {
        let (key, file, def, bind, typ, flags) = (r.u32(), r.u32(), r.u8(), r.u8(), r.u8(), r.u8());
        let demand = r.u16();
        if flags & !(key_ref_flags::TOMB * 2 - 1) != 0
            || mortar_core::demands::DemandFlags::from_bits(demand).is_none()
        {
            r.bad_pad = true;
        }
        Self {
            demand,
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

/// `FRAGS`: key `(out, class, hash)`, `offset`, seam 6's `size`, `refs`,
/// the tomb: 41 bytes.
impl Rec for FragRow {
    const WIDTH: usize = 41;
    fn put(&self, w: &mut W<'_>) {
        w.u32(self.out).u32(self.class).bytes(&self.hash).u64(self.offset).u32(self.size);
        w.u32(self.refs).u8(u8::from(self.tomb));
    }
    fn get(r: &mut R<'_>) -> Self {
        Self {
            out: r.u32(),
            class: r.u32(),
            hash: r.h(),
            offset: r.u64(),
            size: r.u32(),
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
        self.kind <= crate::format::hole_kind::DEAD_SLOT
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

/// `DEBUG_STRS` (kind 57; WP-228 R-2, seam 8): `out` (`u8`,
/// `rows::debug_str_class`: 0 or 1, another value refused), `hash`
/// (`u64`), `offset` (`u32`): 13 bytes. No tomb.
impl Rec for DebugStrRow {
    const WIDTH: usize = 13;
    fn put(&self, w: &mut W<'_>) {
        w.u8(self.out).u64(self.hash).u32(self.offset);
    }
    fn get(r: &mut R<'_>) -> Self {
        Self { out: r.u8(), hash: r.u64(), offset: r.u32() }
    }
    fn ok(&self) -> bool {
        use crate::update::rows::debug_str_class;
        matches!(self.out, debug_str_class::STR | debug_str_class::LINE_STR)
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
/// `STRINGS`): the diagnostic's key, the tomb, and four strings (the
/// subjects, tag-encoded; the code; the text; the references,
/// [`refs_bytes`]), then the reference count (WP-221 seam 7). 55 bytes.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct DiagRaw {
    pub phase: u16,
    pub unit: u32,
    pub seq: u64,
    pub tomb: bool,
    pub subjects: Str,
    pub code: Str,
    pub text: Str,
    pub refs: Str,
    pub ref_count: u64,
}

impl Rec for DiagRaw {
    const WIDTH: usize = 55;
    fn put(&self, w: &mut W<'_>) {
        w.u16(self.phase).u32(self.unit).u64(self.seq).u8(u8::from(self.tomb));
        w.s(self.subjects).s(self.code).s(self.text).s(self.refs).u64(self.ref_count);
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
            refs: r.s(),
            ref_count: r.u64(),
        }
    }
}

/// A `DIAGS` row's references' bytes (WP-221 seam 7): per reference its
/// file (`u32`), seq and offset (`u64` each), 20 bytes, in the row's
/// (sorted) order.
#[must_use]
pub fn refs_bytes(refs: &[DiagRef]) -> Vec<u8> {
    let mut v = Vec::with_capacity(refs.len() * 20);
    let mut w = W(&mut v);
    for r in refs {
        w.u32(r.file.0).u64(r.seq).u64(r.offset);
    }
    v
}

/// [`refs_bytes`]' inverse, or `None` for bytes it does not make or
/// references not strictly sorted.
#[must_use]
pub fn refs_of(b: &[u8]) -> Option<Vec<DiagRef>> {
    if !b.len().is_multiple_of(20) {
        return None;
    }
    let refs: Vec<DiagRef> = b
        .chunks_exact(20)
        .map(|c| {
            let mut r = R::new(c);
            DiagRef { file: mortar_core::FileId(r.u32()), seq: r.u64(), offset: r.u64() }
        })
        .collect();
    refs.windows(2).all(|w| w[0] < w[1]).then_some(refs)
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
    /// Rows of one key are not an error of the state (`DEBUG_STRS`: an
    /// accelerator whose every hit the caller checks, WP-228 R-2.1): its
    /// sorted parts are checked non-decreasing, not increasing.
    const DUPLICATES: bool = false;
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
keyed!(FragRow, (u32, u32, Hash16), |x| (x.out, x.class, x.hash));
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

/// `DEBUG_STRS` has no tomb (a string stays in the output until the
/// compaction, IN-56) and may hold two rows of one key (WP-228 R-2.1).
impl Keyed for DebugStrRow {
    type K = (u8, u64);
    fn key(&self, _s: StrFn<'_>) -> Rows<(u8, u64)> {
        Ok((self.out, self.hash))
    }
    fn tomb(&self) -> bool {
        false
    }
    fn set_tomb(&mut self, _t: bool) {}
    const DUPLICATES: bool = true;
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
        refs: add(&refs_bytes(&d.refs)),
        ref_count: d.ref_count,
    }
}

/// A stored `DIAGS` row as a [`DiagRow`].
pub(crate) fn diag_row(r: &DiagRaw, s: StrFn<'_>) -> Rows<DiagRow> {
    let subjects = subjects_of(&s(r.subjects)?).ok_or_else(|| corrupt("DIAGS: subjects"))?;
    let text = |x: Str| -> Rows<String> {
        String::from_utf8(s(x)?).map_err(|_| corrupt("DIAGS: a string is not UTF-8"))
    };
    let refs = refs_of(&s(r.refs)?).ok_or_else(|| corrupt("DIAGS: references"))?;
    if (refs.len() as u64) > r.ref_count {
        return Err(corrupt("DIAGS: more references than the count"));
    }
    Ok(DiagRow {
        subjects,
        code: text(r.code)?,
        text: text(r.text)?,
        key: mortar_diag::Key { phase: r.phase, unit: r.unit, seq: r.seq },
        refs,
        ref_count: r.ref_count,
        tomb: r.tomb,
    })
}

#[cfg(test)]
mod tests {
    use super::{Rec, decode_rows, encode_rows, kind};
    use crate::update::rows::{EdgeRow, MetaRow, edge_kind};

    fn reads(r: EdgeRow) -> bool {
        decode_rows::<EdgeRow>(&encode_rows(&[r]), kind::EDGES, 0).is_ok()
    }

    // spec: WP-238 "Step 2" S1, F4, F8 and P0-1 (the reader refuses a
    // kind that does not exist, a `REFUSE` code other than 1 and 2, and
    // a target or an offset the kind's class forbids: `state-corrupt`)
    #[test]
    fn edge_row_ok() {
        let row = |kind, target, offset| EdgeRow { kind, source: 5, target, offset };
        assert_eq!(encode_rows(&[row(0, 0, 0)]).len(), 13);
        // accepted: every class at its rule
        assert!(reads(row(edge_kind::RELOC, 9, 0)));
        assert!(reads(row(edge_kind::LOCAL, 9, 7)));
        assert!(reads(row(edge_kind::FDE_SECTION, 9, u32::MAX)));
        assert!(reads(row(edge_kind::GROUP, 9, 0)));
        assert!(reads(row(edge_kind::ROOT, 0, 0)));
        assert!(reads(row(edge_kind::REFUSE, edge_kind::refuse::FDE_GLOBAL, 0)));
        assert!(reads(row(edge_kind::REFUSE, edge_kind::refuse::DEF_DROPPED, 0)));
        // a kind that does not exist
        assert!(!reads(row(edge_kind::LAST + 1, 0, 0)));
        assert!(!reads(row(u8::MAX, 0, 0)));
        // REFUSE: code 0 and an unknown code; an offset
        assert!(!reads(row(edge_kind::REFUSE, 0, 0)));
        assert!(!reads(row(edge_kind::REFUSE, edge_kind::refuse::LAST + 1, 0)));
        assert!(!reads(row(edge_kind::REFUSE, edge_kind::refuse::FDE_GLOBAL, 1)));
        // Zero: a target or an offset
        for k in [edge_kind::ROOT, edge_kind::NAMED, edge_kind::SPLIT] {
            assert!(!reads(row(k, 1, 0)), "kind {k} target");
            assert!(!reads(row(k, 0, 1)), "kind {k} offset");
        }
        // Key: an offset (F8: only a split section target has one)
        for k in [edge_kind::RELOC, edge_kind::START_STOP, edge_kind::FDE_KEY] {
            assert!(!reads(row(k, 3, 4)), "kind {k} offset");
        }
        assert!(!reads(row(edge_kind::NONALLOC_KEY, 3, u32::MAX)));
        // the bytes of a refused row are field for field what was written
        let r = row(edge_kind::LINK_ORDER, 0x0102_0304, 0x0506_0708);
        assert_eq!(decode_rows::<EdgeRow>(&encode_rows(&[r]), kind::EDGES, 0), Ok(vec![r]));
    }

    // spec: WP-238 "Step 2" S4 (`META.live_hash`, 16 bytes after
    // `versioned`'s area), WP-263 (`gnu_warning_orphans`, 4 bytes after
    // it); WP-264 (the clone probe's 32 bytes removed: META is 196 bytes)
    #[test]
    fn meta_live_hash_is_read_back() {
        let m = MetaRow {
            live_hash: *b"live-set-hash-16",
            gnu_warning_orphans: 0x0102_0304,
            ..MetaRow::default()
        };
        let b = encode_rows(std::slice::from_ref(&m));
        assert_eq!(b.len(), <MetaRow as Rec>::WIDTH);
        assert_eq!(b.len(), 196);
        assert_eq!(&b[176..192], &m.live_hash);
        assert_eq!(&b[192..], &0x0102_0304u32.to_le_bytes());
        let back = decode_rows::<MetaRow>(&b, kind::META, 0).expect("reads");
        assert_eq!(back, vec![m]);
    }
}
