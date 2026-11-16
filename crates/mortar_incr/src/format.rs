//! The bytes of a link state, format version 2 (`spec/incr.md` IN-18a):
//! the rows as plain values, the encoder, and the validating decoder.
//! Version 2 is version 1 with the tables `MENTIONS`, `RESOLVED`,
//! `INDEX` (kinds 32-34) and `HOLES` (39) between `LEAVES` and
//! `STRINGS`. A state of version 1 is refused by name (`state-version`).
//!
//! The decoder is the only way in: a state is untrusted input, so every
//! length, hash, enumeration, pad and reference is checked before a row
//! is handed out, and every failure is a [`StateError`] naming the table,
//! row and field. Nothing here panics on any input (the mutation tests
//! feed it every one-byte change and every truncation).

use std::collections::HashMap;
use std::fmt;

use mortar_digest::{L128, l128};

/// `MORTARST`.
pub const MAGIC: [u8; 8] = *b"MORTARST";
/// The format version this crate writes and reads.
pub const FORMAT_VERSION: u32 = 3;
/// The header's size.
pub const HEADER_LEN: usize = 64;
/// A table record's size.
pub const RECORD_LEN: usize = 40;
/// Header flag bit 0: an update is writing the output (IN-46).
pub const FLAG_UPDATING: u32 = 1;
/// An absent row reference.
pub const NONE: u32 = u32::MAX;
/// The longest build id a state records (`META.build_id`; IN-18a).
pub const MAX_BUILD_ID: u32 = 64;

/// Why a state is not used (`spec/incr.md` §7): `state-corrupt` or
/// `state-version`, with a detail.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StateError {
    /// The state fails validation (IN-18).
    Corrupt(String),
    /// Its format or mortar version is not this one's.
    Version(String),
}

impl StateError {
    /// The fall-back reason (`spec/incr.md` §7).
    #[must_use]
    pub fn reason(&self) -> &'static str {
        match self {
            Self::Corrupt(_) => "state-corrupt",
            Self::Version(_) => "state-version",
        }
    }

    /// What is wrong.
    #[must_use]
    pub fn detail(&self) -> &str {
        match self {
            Self::Corrupt(d) | Self::Version(d) => d,
        }
    }
}

impl fmt::Display for StateError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{} ({})", self.reason(), self.detail())
    }
}

fn corrupt(detail: impl Into<String>) -> StateError {
    StateError::Corrupt(detail.into())
}

/// A string: bytes of the `STRINGS` table.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Str {
    pub off: u32,
    pub len: u32,
}

/// A 16-byte hash.
pub type Hash16 = [u8; 16];

// ---- the table kinds --------------------------------------------------------

/// The table kinds of format version 2, in file order (IN-18a): 1 to 22,
/// then 32, 33, 34 and 39, then `STRINGS` last.
pub mod kind {
    pub const META: u32 = 1;
    pub const STAT: u32 = 2;
    pub const FILES: u32 = 3;
    pub const PROBES: u32 = 4;
    pub const OUTSECS: u32 = 5;
    pub const SYMBOLS: u32 = 6;
    pub const SECTIONS: u32 = 7;
    pub const CHUNKS: u32 = 8;
    pub const ORDER: u32 = 9;
    pub const SEGMENTS: u32 = 10;
    pub const FRAGS: u32 = 11;
    pub const SLOTS: u32 = 12;
    pub const EH_RECORDS: u32 = 13;
    pub const SYMTAB_LOCALS: u32 = 14;
    pub const SIZES: u32 = 15;
    pub const SYM_PLACE: u32 = 16;
    pub const DEFINED: u32 = 17;
    pub const PATCHES: u32 = 18;
    pub const REDIRECTS: u32 = 19;
    pub const SIZED: u32 = 20;
    pub const LAYOUT: u32 = 21;
    pub const LEAVES: u32 = 22;
    pub const STRINGS: u32 = 23;
    /// Format version 2's delta tables (WP-063, IN-18a): written after
    /// `LEAVES`, before `HOLES` and `STRINGS`, which stays last.
    pub const MENTIONS: u32 = 32;
    pub const RESOLVED: u32 = 33;
    pub const INDEX: u32 = 34;
    /// Regions and holes (IN-18a's reserved kind 39): format version 2.
    pub const HOLES: u32 = 39;
    /// WP-094 prototype (format version 3): the live fragments by position
    /// (`SECTIONS` row, offset in the section), after `HOLES`.
    pub const FRAG_AT: u32 = 40;
    /// The number of tables of version 3.
    pub const COUNT: u32 = 28;

    /// The name of a kind, for diagnostics.
    #[must_use]
    pub fn name(kind: u32) -> &'static str {
        const NAMES: [&str; 23] = [
            "META",
            "STAT",
            "FILES",
            "PROBES",
            "OUTSECS",
            "SYMBOLS",
            "SECTIONS",
            "CHUNKS",
            "ORDER",
            "SEGMENTS",
            "FRAGS",
            "SLOTS",
            "EH_RECORDS",
            "SYMTAB_LOCALS",
            "SIZES",
            "SYM_PLACE",
            "DEFINED",
            "PATCHES",
            "REDIRECTS",
            "SIZED",
            "LAYOUT",
            "LEAVES",
            "STRINGS",
        ];
        match kind {
            MENTIONS => "MENTIONS",
            RESOLVED => "RESOLVED",
            INDEX => "INDEX",
            HOLES => "HOLES",
            FRAG_AT => "FRAG_AT",
            _ => kind.checked_sub(1).and_then(|i| NAMES.get(i as usize)).copied().unwrap_or("?"),
        }
    }
}

// ---- rows -------------------------------------------------------------------

/// `META` (one row).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Meta {
    pub version: Str,
    pub machine: u16,
    pub class: u8,
    pub output_kind: u8,
    pub leaf_fn: u8,
    pub file_size: u64,
    pub sealed_start: u64,
    pub sealed_end: u64,
    pub build_id: Str,
}

/// `STAT` tags.
pub mod stat_tag {
    pub const DEVICE: u8 = 0;
    pub const INPUT: u8 = 1;
    pub const OUTPUT: u8 = 2;
}

/// `STAT`: a device's stamp, a file opened, or the output (IN-21, IN-22a).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct StatRow {
    pub tag: u8,
    pub trust: u8,
    pub path: Str,
    pub dev: u64,
    pub ino: u64,
    pub size: u64,
    pub mtime_ns: i64,
    pub ctime_ns: i64,
    pub stamp_ns: i64,
    pub hash: Hash16,
    pub fstype: Str,
}

/// `FILES`: one per `FileId`.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct FileRow {
    pub kind: u8,
    pub member: u8,
    pub path: Str,
    pub mention: u32,
    pub member_name: Str,
    pub member_ordinal: u32,
    pub base: u64,
    pub len: u64,
    pub hash: Hash16,
}

/// `PROBES`.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ProbeRow {
    pub path: Str,
    pub present: u8,
}

/// `OUTSECS`.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct OutSecRow {
    pub name: Str,
    pub ordinal: u32,
}

/// `SYMBOLS` kinds.
pub mod sym_kind {
    pub const GLOBAL: u8 = 0;
    pub const VERSIONED: u8 = 1;
    pub const LOCAL: u8 = 2;
}

/// `SYMBOLS`: a key in identity terms.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct SymbolRow {
    pub kind: u8,
    pub file: u32,
    pub ordinal: u32,
    pub name: Str,
    pub version: Str,
    /// WP-094 (format version 3): where the key is read in a file, for
    /// the replay by position: the `FILES` row (a local's own file; a
    /// global's first mention, `NONE` if none) and the index (a local's
    /// symbol-table index; a global's entry among its file's non-local
    /// entries, from `first_global`).
    pub at_file: u32,
    pub at_index: u32,
}

/// `FRAG_AT` (WP-094, format version 3): a live fragment by the position
/// of its first occurrence, and its offset in its output section.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, PartialOrd, Ord)]
pub struct FragAtRow {
    pub section: u32,
    pub offset: u32,
    pub at: u64,
}

/// `SECTIONS`: one per `SectionId`.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SectionRow {
    pub file: u32,
    pub anchor_kind: u8,
    pub anchor: Str,
    pub name: Str,
    pub ordinal: u32,
    pub sh_type: u32,
    pub sh_flags: u64,
    pub size: u64,
    pub align: u64,
    pub offset: u64,
    pub place_chunk: u32,
    pub place_offset: u64,
}

/// `CHUNKS` bits.
pub mod chunk_bits {
    pub const HAS_ADDR: u8 = 1;
    pub const HAS_SHNDX: u8 = 2;
    pub const RELRO: u8 = 4;
    pub const ADDRESS_DEPENDENT: u8 = 8;
    pub const ALL: u8 = 15;
}

/// `CHUNKS`: one per `ChunkId`.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ChunkRow {
    pub tag: u8,
    pub bits: u8,
    pub align: u8,
    pub name: Str,
    pub ordinal: u32,
    pub out: u32,
    pub part: u32,
    pub sh_type: u32,
    pub sh_flags: u64,
    pub size: u64,
    pub capacity: u64,
    pub addr: u64,
    pub offset: u64,
    pub shndx: u32,
}

/// `SEGMENTS`.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SegmentRow {
    pub p_type: u32,
    pub p_flags: u32,
    pub offset: u64,
    pub addr: u64,
    pub filesz: u64,
    pub memsz: u64,
    pub align: u64,
}

/// `FRAGS`.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct FragRow {
    pub out: u32,
    pub ordinal: u32,
    pub hash: Hash16,
    pub offset: u64,
}

/// `SLOTS`.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SlotRow {
    pub kind: u8,
    pub symbol: u32,
}

/// `EH_RECORDS`.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct EhRow {
    pub section: u32,
    pub piece: u32,
}

/// `SYMTAB_LOCALS`.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SpanRow {
    pub start: u32,
    pub len: u32,
}

/// `SIZES`, `SIZED`.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SizeRow {
    pub chunk: u32,
    pub size: u64,
}

/// `SYM_PLACE`.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SymPlaceRow {
    pub kind: u8,
    pub symbol: u32,
    pub chunk: u32,
    pub offset: u64,
    pub size: u64,
    pub align: u8,
}

/// `DEFINED`.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct DefinedRow {
    pub symbol: u32,
    pub kind: u8,
    pub chunk: u32,
    pub value: u64,
}

/// `PATCHES` kinds.
pub mod patch_kind {
    pub const THUNK: u8 = 0;
    pub const THUNK_PLT: u8 = 1;
    pub const E843419: u8 = 2;
}

/// `PATCHES`.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct PatchRow {
    pub chunk: u32,
    pub kind: u8,
    pub offset: u64,
    pub symbol: u32,
    pub section: u32,
    /// The addend (kind 0) or the site offset (kind 2), as stored.
    pub value: u64,
}

/// `REDIRECTS`.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct RedirectRow {
    pub section: u32,
    pub patch: u32,
    pub offset: u64,
}

/// `LAYOUT` bits.
pub mod layout_bits {
    pub const TLS: u8 = 1;
    pub const ENTRY: u8 = 2;
    pub const TLS_LD: u8 = 4;
    pub const ALL: u8 = 7;
}

/// `LAYOUT` (one row).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct LayoutRow {
    pub bits: u8,
    pub tls_align: u8,
    pub tls_start: u64,
    pub tls_size: u64,
    pub entry: u64,
}

/// `MENTIONS` and `RESOLVED` kinds of a definition (IN-26's columns).
pub mod def_kind {
    pub const UNDEFINED: u8 = 0;
    pub const SECTION: u8 = 1;
    pub const ABSOLUTE: u8 = 2;
    pub const COMMON: u8 = 3;
    /// `RESOLVED` only: a linker-defined key (`Def::Internal`).
    pub const INTERNAL: u8 = 4;
    /// `RESOLVED` only: a shared object's definition.
    pub const SHARED: u8 = 5;
    /// `RESOLVED` only: a COMDAT group's keeper (`name` the signature).
    pub const COMDAT: u8 = 6;
    /// `RESOLVED` only: a live file (`file`).
    pub const LIVE: u8 = 7;
}

/// `MENTIONS` (v2): a global key in the symbol table of a live object,
/// with the entry's attributes (IN-26: what an unchanged file references
/// or defines).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, PartialOrd, Ord)]
pub struct MentionRow {
    pub file: u32,
    pub symbol: u32,
    pub kind: u8,
    pub bind: u8,
    pub typ: u8,
    pub other: u8,
    /// The `SECTIONS` row of a definition in a section, else `NONE`.
    pub section: u32,
}

/// `RESOLVED` (v2): per global key its resolution (kinds 0-5), per
/// COMDAT group its keeper (6), per live file a row (7) (IN-26).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct ResolvedRow {
    pub kind: u8,
    pub typ: u8,
    pub bind: u8,
    pub other: u8,
    pub symbol: u32,
    pub file: u32,
    pub section: u32,
    /// The definition's `st_value` (kind 1, 2), the common's size (3).
    pub value: u64,
    pub size: u64,
    /// The group signature (kind 6).
    pub name: Str,
}

/// `INDEX` kinds (IN-25).
pub mod index_kind {
    /// The unit's relocations name the global key `target` (`SYMBOLS`).
    pub const GLOBAL: u8 = 0;
    /// ... a section `target` of its own file (`SECTIONS`).
    pub const SECTION: u8 = 1;
    /// ... use a slot (GOT, TLS) or the GOT's base for global `target`.
    pub const SLOT: u8 = 2;
    /// ... name an input `.eh_frame` `target` (S-50, IN-34a).
    pub const EH_FRAME: u8 = 3;
    /// The unit is `SHF_LINK_ORDER` with `sh_link` `target`.
    pub const LINK_ORDER: u8 = 4;
    /// ... use a slot of a local of section `target`.
    pub const LOCAL_SLOT: u8 = 5;
}

/// `INDEX` (v2): one (referring unit, target) pair (IN-25).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct IndexRow {
    pub unit: u32,
    pub kind: u8,
    pub target: u32,
}

/// `HOLES` kinds (`mortar_core::layout::HoleKind`, in its order).
pub mod hole_kind {
    pub const FILL: u8 = 0;
    pub const EH_PAD: u8 = 1;
    pub const DEBUG: u8 = 2;
    pub const ABBREV_TABLE: u8 = 3;
    pub const DEAD_DEBUG: u8 = 4;
    pub const DEAD_FRAGMENT: u8 = 5;
    pub const DEAD_CIE: u8 = 6;
}

/// `HOLES`: a range of a chunk no live unit owns (IN-3, IN-3a, IN-30),
/// sorted by `(chunk, offset)`, never overlapping, inside the chunk's size
/// (a capacity tail past the size is not a hole, L-141).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct HoleRow {
    pub chunk: u32,
    pub kind: u8,
    pub offset: u64,
    pub size: u64,
}

/// A whole state, as rows (`spec/incr.md` IN-18a).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct State {
    pub flags: u32,
    pub plan_hash: Hash16,
    pub meta: Meta,
    pub stat: Vec<StatRow>,
    pub files: Vec<FileRow>,
    pub probes: Vec<ProbeRow>,
    pub outsecs: Vec<OutSecRow>,
    pub symbols: Vec<SymbolRow>,
    pub sections: Vec<SectionRow>,
    pub chunks: Vec<ChunkRow>,
    pub order: Vec<u32>,
    pub segments: Vec<SegmentRow>,
    pub frags: Vec<FragRow>,
    pub slots: Vec<SlotRow>,
    pub eh_records: Vec<EhRow>,
    pub symtab_locals: Vec<SpanRow>,
    pub sizes: Vec<SizeRow>,
    pub sym_place: Vec<SymPlaceRow>,
    pub defined: Vec<DefinedRow>,
    pub patches: Vec<PatchRow>,
    pub redirects: Vec<RedirectRow>,
    pub sized: Vec<SizeRow>,
    pub layout: LayoutRow,
    pub leaves: Vec<[u8; 32]>,
    pub mentions: Vec<MentionRow>,
    pub resolved: Vec<ResolvedRow>,
    pub index: Vec<IndexRow>,
    pub holes: Vec<HoleRow>,
    pub frag_at: Vec<FragAtRow>,
    pub strings: Vec<u8>,
}

impl State {
    /// The bytes of a string; empty if out of range (never after
    /// [`decode`], which checked every string).
    #[must_use]
    pub fn str(&self, s: Str) -> &[u8] {
        let start = s.off as usize;
        start
            .checked_add(s.len as usize)
            .and_then(|end| self.strings.get(start..end))
            .unwrap_or_default()
    }
}

/// The string pool a writer builds: each distinct string once, in first-use
/// order (deterministic).
#[derive(Debug, Default)]
pub struct Strings {
    bytes: Vec<u8>,
    /// WP-094: by the fixed hash of the bytes (`spec/model.md` §6 rule 5),
    /// the first string with it; no copy of the bytes is kept.
    seen: mortar_core::hash::HashMap<u64, Str>,
    /// The strings whose hash an earlier, different string has.
    collided: Vec<(u64, Str)>,
}

impl Strings {
    fn bytes_of(&self, at: Str) -> &[u8] {
        let start = at.off as usize;
        self.bytes.get(start..start + at.len as usize).unwrap_or_default()
    }

    /// The string's place in the pool, adding it if new.
    pub fn add(&mut self, s: &[u8]) -> Str {
        if s.is_empty() {
            return Str::default();
        }
        let h = mortar_core::hash::hash_bytes(s);
        let first = self.seen.get(&h).copied();
        if let Some(at) = first {
            if self.bytes_of(at) == s {
                return at;
            }
            if let Some(&(_, at)) =
                self.collided.iter().find(|(k, at)| *k == h && self.bytes_of(*at) == s)
            {
                return at;
            }
        }
        let at = Str {
            off: u32::try_from(self.bytes.len()).unwrap_or(u32::MAX),
            len: u32::try_from(s.len()).unwrap_or(u32::MAX),
        };
        self.bytes.extend_from_slice(s);
        if first.is_some() {
            self.collided.push((h, at));
        } else {
            self.seen.insert(h, at);
        }
        at
    }

    /// Whether the pool still fits the format (offsets are `u32`).
    #[must_use]
    pub fn fits(&self) -> bool {
        u32::try_from(self.bytes.len()).is_ok()
    }

    /// The pool's bytes.
    #[must_use]
    pub fn into_bytes(self) -> Vec<u8> {
        self.bytes
    }
}

// ---- reading and writing fields ----------------------------------------------

struct W<'a>(&'a mut Vec<u8>);

impl W<'_> {
    fn u8(&mut self, v: u8) -> &mut Self {
        self.0.push(v);
        self
    }
    fn u16(&mut self, v: u16) -> &mut Self {
        self.0.extend_from_slice(&v.to_le_bytes());
        self
    }
    fn u32(&mut self, v: u32) -> &mut Self {
        self.0.extend_from_slice(&v.to_le_bytes());
        self
    }
    fn u64(&mut self, v: u64) -> &mut Self {
        self.0.extend_from_slice(&v.to_le_bytes());
        self
    }
    fn i64(&mut self, v: i64) -> &mut Self {
        self.0.extend_from_slice(&v.to_le_bytes());
        self
    }
    fn pad(&mut self, n: usize) -> &mut Self {
        self.0.extend(std::iter::repeat_n(0, n));
        self
    }
    fn s(&mut self, s: Str) -> &mut Self {
        self.u32(s.off).u32(s.len)
    }
    fn bytes(&mut self, b: &[u8]) -> &mut Self {
        self.0.extend_from_slice(b);
        self
    }
}

/// A cursor over one row's bytes. Reads past the end give zeros (never
/// reached: the row's width was checked), and pad fields that are not zero
/// are remembered.
struct R<'a> {
    b: &'a [u8],
    at: usize,
    bad_pad: bool,
}

impl<'a> R<'a> {
    fn new(b: &'a [u8]) -> Self {
        Self { b, at: 0, bad_pad: false }
    }
    fn take<const N: usize>(&mut self) -> [u8; N] {
        let mut out = [0u8; N];
        if let Some(src) = self.at.checked_add(N).and_then(|end| self.b.get(self.at..end)) {
            out.copy_from_slice(src);
        }
        self.at = self.at.saturating_add(N);
        out
    }
    fn u8(&mut self) -> u8 {
        self.take::<1>()[0]
    }
    fn u16(&mut self) -> u16 {
        u16::from_le_bytes(self.take())
    }
    fn u32(&mut self) -> u32 {
        u32::from_le_bytes(self.take())
    }
    fn u64(&mut self) -> u64 {
        u64::from_le_bytes(self.take())
    }
    fn i64(&mut self) -> i64 {
        i64::from_le_bytes(self.take())
    }
    fn h(&mut self) -> Hash16 {
        self.take()
    }
    fn s(&mut self) -> Str {
        Str { off: self.u32(), len: self.u32() }
    }
    fn pad(&mut self, n: usize) {
        for _ in 0..n {
            if self.u8() != 0 {
                self.bad_pad = true;
            }
        }
    }
}

/// One row type: its width and its two conversions.
trait Row: Sized {
    const WIDTH: usize;
    fn put(&self, w: &mut W<'_>);
    fn get(r: &mut R<'_>) -> Self;
}

impl Row for Meta {
    const WIDTH: usize = 48;
    fn put(&self, w: &mut W<'_>) {
        w.s(self.version).u16(self.machine).u8(self.class).u8(self.output_kind).u8(self.leaf_fn);
        w.pad(3).u64(self.file_size).u64(self.sealed_start).u64(self.sealed_end).s(self.build_id);
    }
    fn get(r: &mut R<'_>) -> Self {
        let (version, machine, class, output_kind, leaf_fn) =
            (r.s(), r.u16(), r.u8(), r.u8(), r.u8());
        r.pad(3);
        Self {
            version,
            machine,
            class,
            output_kind,
            leaf_fn,
            file_size: r.u64(),
            sealed_start: r.u64(),
            sealed_end: r.u64(),
            build_id: r.s(),
        }
    }
}

impl Row for StatRow {
    const WIDTH: usize = 88;
    fn put(&self, w: &mut W<'_>) {
        w.u8(self.tag).u8(self.trust).pad(6).s(self.path).u64(self.dev).u64(self.ino);
        w.u64(self.size).i64(self.mtime_ns).i64(self.ctime_ns).i64(self.stamp_ns);
        w.bytes(&self.hash).s(self.fstype);
    }
    fn get(r: &mut R<'_>) -> Self {
        let (tag, trust) = (r.u8(), r.u8());
        r.pad(6);
        Self {
            tag,
            trust,
            path: r.s(),
            dev: r.u64(),
            ino: r.u64(),
            size: r.u64(),
            mtime_ns: r.i64(),
            ctime_ns: r.i64(),
            stamp_ns: r.i64(),
            hash: r.h(),
            fstype: r.s(),
        }
    }
}

impl Row for FileRow {
    const WIDTH: usize = 60;
    fn put(&self, w: &mut W<'_>) {
        w.u8(self.kind).u8(self.member).pad(2).s(self.path).u32(self.mention);
        w.s(self.member_name).u32(self.member_ordinal).u64(self.base).u64(self.len);
        w.bytes(&self.hash);
    }
    fn get(r: &mut R<'_>) -> Self {
        let (kind, member) = (r.u8(), r.u8());
        r.pad(2);
        Self {
            kind,
            member,
            path: r.s(),
            mention: r.u32(),
            member_name: r.s(),
            member_ordinal: r.u32(),
            base: r.u64(),
            len: r.u64(),
            hash: r.h(),
        }
    }
}

impl Row for ProbeRow {
    const WIDTH: usize = 12;
    fn put(&self, w: &mut W<'_>) {
        w.s(self.path).u8(self.present).pad(3);
    }
    fn get(r: &mut R<'_>) -> Self {
        let out = Self { path: r.s(), present: r.u8() };
        r.pad(3);
        out
    }
}

impl Row for OutSecRow {
    const WIDTH: usize = 12;
    fn put(&self, w: &mut W<'_>) {
        w.s(self.name).u32(self.ordinal);
    }
    fn get(r: &mut R<'_>) -> Self {
        Self { name: r.s(), ordinal: r.u32() }
    }
}

impl Row for SymbolRow {
    const WIDTH: usize = 36;
    fn put(&self, w: &mut W<'_>) {
        w.u8(self.kind).pad(3).u32(self.file).u32(self.ordinal).s(self.name).s(self.version);
        w.u32(self.at_file).u32(self.at_index);
    }
    fn get(r: &mut R<'_>) -> Self {
        let kind = r.u8();
        r.pad(3);
        Self {
            kind,
            file: r.u32(),
            ordinal: r.u32(),
            name: r.s(),
            version: r.s(),
            at_file: r.u32(),
            at_index: r.u32(),
        }
    }
}

impl Row for FragAtRow {
    const WIDTH: usize = 16;
    fn put(&self, w: &mut W<'_>) {
        w.u32(self.section).u32(self.offset).u64(self.at);
    }
    fn get(r: &mut R<'_>) -> Self {
        Self { section: r.u32(), offset: r.u32(), at: r.u64() }
    }
}

impl Row for SectionRow {
    const WIDTH: usize = 76;
    fn put(&self, w: &mut W<'_>) {
        w.u32(self.file).u8(self.anchor_kind).pad(3).s(self.anchor).s(self.name);
        w.u32(self.ordinal).u32(self.sh_type).u64(self.sh_flags).u64(self.size);
        w.u64(self.align).u64(self.offset).u32(self.place_chunk).u64(self.place_offset);
    }
    fn get(r: &mut R<'_>) -> Self {
        let (file, anchor_kind) = (r.u32(), r.u8());
        r.pad(3);
        Self {
            file,
            anchor_kind,
            anchor: r.s(),
            name: r.s(),
            ordinal: r.u32(),
            sh_type: r.u32(),
            sh_flags: r.u64(),
            size: r.u64(),
            align: r.u64(),
            offset: r.u64(),
            place_chunk: r.u32(),
            place_offset: r.u64(),
        }
    }
}

impl Row for ChunkRow {
    const WIDTH: usize = 72;
    fn put(&self, w: &mut W<'_>) {
        w.u8(self.tag).u8(self.bits).u8(self.align).pad(1).s(self.name).u32(self.ordinal);
        w.u32(self.out).u32(self.part).u32(self.sh_type).u64(self.sh_flags).u64(self.size);
        w.u64(self.capacity).u64(self.addr).u64(self.offset).u32(self.shndx);
    }
    fn get(r: &mut R<'_>) -> Self {
        let (tag, bits, align) = (r.u8(), r.u8(), r.u8());
        r.pad(1);
        Self {
            tag,
            bits,
            align,
            name: r.s(),
            ordinal: r.u32(),
            out: r.u32(),
            part: r.u32(),
            sh_type: r.u32(),
            sh_flags: r.u64(),
            size: r.u64(),
            capacity: r.u64(),
            addr: r.u64(),
            offset: r.u64(),
            shndx: r.u32(),
        }
    }
}

impl Row for u32 {
    const WIDTH: usize = 4;
    fn put(&self, w: &mut W<'_>) {
        w.u32(*self);
    }
    fn get(r: &mut R<'_>) -> Self {
        r.u32()
    }
}

impl Row for SegmentRow {
    const WIDTH: usize = 48;
    fn put(&self, w: &mut W<'_>) {
        w.u32(self.p_type).u32(self.p_flags).u64(self.offset).u64(self.addr);
        w.u64(self.filesz).u64(self.memsz).u64(self.align);
    }
    fn get(r: &mut R<'_>) -> Self {
        Self {
            p_type: r.u32(),
            p_flags: r.u32(),
            offset: r.u64(),
            addr: r.u64(),
            filesz: r.u64(),
            memsz: r.u64(),
            align: r.u64(),
        }
    }
}

impl Row for FragRow {
    const WIDTH: usize = 32;
    fn put(&self, w: &mut W<'_>) {
        w.u32(self.out).u32(self.ordinal).bytes(&self.hash).u64(self.offset);
    }
    fn get(r: &mut R<'_>) -> Self {
        Self { out: r.u32(), ordinal: r.u32(), hash: r.h(), offset: r.u64() }
    }
}

impl Row for SlotRow {
    const WIDTH: usize = 8;
    fn put(&self, w: &mut W<'_>) {
        w.u8(self.kind).pad(3).u32(self.symbol);
    }
    fn get(r: &mut R<'_>) -> Self {
        let kind = r.u8();
        r.pad(3);
        Self { kind, symbol: r.u32() }
    }
}

impl Row for EhRow {
    const WIDTH: usize = 8;
    fn put(&self, w: &mut W<'_>) {
        w.u32(self.section).u32(self.piece);
    }
    fn get(r: &mut R<'_>) -> Self {
        Self { section: r.u32(), piece: r.u32() }
    }
}

impl Row for SpanRow {
    const WIDTH: usize = 8;
    fn put(&self, w: &mut W<'_>) {
        w.u32(self.start).u32(self.len);
    }
    fn get(r: &mut R<'_>) -> Self {
        Self { start: r.u32(), len: r.u32() }
    }
}

impl Row for SizeRow {
    const WIDTH: usize = 12;
    fn put(&self, w: &mut W<'_>) {
        w.u32(self.chunk).u64(self.size);
    }
    fn get(r: &mut R<'_>) -> Self {
        Self { chunk: r.u32(), size: r.u64() }
    }
}

impl Row for SymPlaceRow {
    const WIDTH: usize = 32;
    fn put(&self, w: &mut W<'_>) {
        w.u8(self.kind).pad(3).u32(self.symbol).u32(self.chunk).u64(self.offset);
        w.u64(self.size).u8(self.align).pad(3);
    }
    fn get(r: &mut R<'_>) -> Self {
        let kind = r.u8();
        r.pad(3);
        let out = Self {
            kind,
            symbol: r.u32(),
            chunk: r.u32(),
            offset: r.u64(),
            size: r.u64(),
            align: r.u8(),
        };
        r.pad(3);
        out
    }
}

impl Row for DefinedRow {
    const WIDTH: usize = 20;
    fn put(&self, w: &mut W<'_>) {
        w.u32(self.symbol).u8(self.kind).pad(3).u32(self.chunk).u64(self.value);
    }
    fn get(r: &mut R<'_>) -> Self {
        let (symbol, kind) = (r.u32(), r.u8());
        r.pad(3);
        Self { symbol, kind, chunk: r.u32(), value: r.u64() }
    }
}

impl Row for PatchRow {
    const WIDTH: usize = 32;
    fn put(&self, w: &mut W<'_>) {
        w.u32(self.chunk).u8(self.kind).pad(3).u64(self.offset).u32(self.symbol);
        w.u32(self.section).u64(self.value);
    }
    fn get(r: &mut R<'_>) -> Self {
        let (chunk, kind) = (r.u32(), r.u8());
        r.pad(3);
        Self { chunk, kind, offset: r.u64(), symbol: r.u32(), section: r.u32(), value: r.u64() }
    }
}

impl Row for RedirectRow {
    const WIDTH: usize = 16;
    fn put(&self, w: &mut W<'_>) {
        w.u32(self.section).u32(self.patch).u64(self.offset);
    }
    fn get(r: &mut R<'_>) -> Self {
        Self { section: r.u32(), patch: r.u32(), offset: r.u64() }
    }
}

impl Row for LayoutRow {
    const WIDTH: usize = 32;
    fn put(&self, w: &mut W<'_>) {
        w.u8(self.bits).u8(self.tls_align).pad(6).u64(self.tls_start).u64(self.tls_size);
        w.u64(self.entry);
    }
    fn get(r: &mut R<'_>) -> Self {
        let (bits, tls_align) = (r.u8(), r.u8());
        r.pad(6);
        Self { bits, tls_align, tls_start: r.u64(), tls_size: r.u64(), entry: r.u64() }
    }
}

impl Row for MentionRow {
    const WIDTH: usize = 16;
    fn put(&self, w: &mut W<'_>) {
        w.u32(self.file).u32(self.symbol).u8(self.kind).u8(self.bind).u8(self.typ).u8(self.other);
        w.u32(self.section);
    }
    fn get(r: &mut R<'_>) -> Self {
        Self {
            file: r.u32(),
            symbol: r.u32(),
            kind: r.u8(),
            bind: r.u8(),
            typ: r.u8(),
            other: r.u8(),
            section: r.u32(),
        }
    }
}

impl Row for ResolvedRow {
    const WIDTH: usize = 40;
    fn put(&self, w: &mut W<'_>) {
        w.u8(self.kind).u8(self.typ).u8(self.bind).u8(self.other).u32(self.symbol);
        w.u32(self.file).u32(self.section).u64(self.value).u64(self.size).s(self.name);
    }
    fn get(r: &mut R<'_>) -> Self {
        Self {
            kind: r.u8(),
            typ: r.u8(),
            bind: r.u8(),
            other: r.u8(),
            symbol: r.u32(),
            file: r.u32(),
            section: r.u32(),
            value: r.u64(),
            size: r.u64(),
            name: r.s(),
        }
    }
}

impl Row for IndexRow {
    const WIDTH: usize = 12;
    fn put(&self, w: &mut W<'_>) {
        w.u32(self.unit).u8(self.kind).pad(3).u32(self.target);
    }
    fn get(r: &mut R<'_>) -> Self {
        let (unit, kind) = (r.u32(), r.u8());
        r.pad(3);
        Self { unit, kind, target: r.u32() }
    }
}

impl Row for HoleRow {
    const WIDTH: usize = 24;
    fn put(&self, w: &mut W<'_>) {
        w.u32(self.chunk).u8(self.kind).pad(3).u64(self.offset).u64(self.size);
    }
    fn get(r: &mut R<'_>) -> Self {
        let (chunk, kind) = (r.u32(), r.u8());
        r.pad(3);
        Self { chunk, kind, offset: r.u64(), size: r.u64() }
    }
}

impl Row for [u8; 32] {
    const WIDTH: usize = 32;
    fn put(&self, w: &mut W<'_>) {
        w.bytes(self);
    }
    fn get(r: &mut R<'_>) -> Self {
        r.take()
    }
}

// ---- encoding -------------------------------------------------------------------

/// A table's record, its rows written in place after its head (no copy
/// of the body), the head filled in once the body's hash is known.
fn table<T: Row>(out: &mut Vec<u8>, kind: u32, rows: &[T]) {
    let head = out.len();
    out.resize(head + RECORD_LEN, 0);
    out.reserve(rows.len().saturating_mul(T::WIDTH));
    let mut w = W(out);
    for row in rows {
        row.put(&mut w);
    }
    let body = &out[head + RECORD_LEN..];
    let mut h = Vec::with_capacity(RECORD_LEN);
    W(&mut h).u32(kind).u32(0).u64(rows.len() as u64).u64(body.len() as u64).bytes(&l128(body));
    out[head..head + RECORD_LEN].copy_from_slice(&h);
}

/// The bytes a table's record takes.
fn table_len<T: Row>(rows: &[T]) -> usize {
    RECORD_LEN + rows.len().saturating_mul(T::WIDTH)
}

fn record(out: &mut Vec<u8>, kind: u32, rows: u64, body: &[u8]) {
    let mut w = W(out);
    w.u32(kind).u32(0).u64(rows).u64(body.len() as u64).bytes(&l128(body));
    out.extend_from_slice(body);
}

/// The header hash: bytes 0..12 and 16..48 (IN-18a).
fn header_hash(header: &[u8]) -> Hash16 {
    let mut h = L128::new();
    h.update(header.get(0..12).unwrap_or_default());
    h.update(header.get(16..48).unwrap_or_default());
    h.finish()
}

/// The bytes of a state.
#[must_use]
pub fn encode(s: &State) -> Vec<u8> {
    // The whole size up front: one allocation, no growth by copies.
    let len = HEADER_LEN
        + table_len(std::slice::from_ref(&s.meta))
        + table_len(&s.stat)
        + table_len(&s.files)
        + table_len(&s.probes)
        + table_len(&s.outsecs)
        + table_len(&s.symbols)
        + table_len(&s.sections)
        + table_len(&s.chunks)
        + table_len(&s.order)
        + table_len(&s.segments)
        + table_len(&s.frags)
        + table_len(&s.slots)
        + table_len(&s.eh_records)
        + table_len(&s.symtab_locals)
        + table_len(&s.sizes)
        + table_len(&s.sym_place)
        + table_len(&s.defined)
        + table_len(&s.patches)
        + table_len(&s.redirects)
        + table_len(&s.sized)
        + table_len(std::slice::from_ref(&s.layout))
        + table_len(&s.leaves)
        + table_len(&s.mentions)
        + table_len(&s.resolved)
        + table_len(&s.index)
        + table_len(&s.holes)
        + table_len(&s.frag_at)
        + RECORD_LEN
        + s.strings.len();
    let mut out = Vec::with_capacity(len);
    out.resize(HEADER_LEN, 0);
    table(&mut out, kind::META, std::slice::from_ref(&s.meta));
    table(&mut out, kind::STAT, &s.stat);
    table(&mut out, kind::FILES, &s.files);
    table(&mut out, kind::PROBES, &s.probes);
    table(&mut out, kind::OUTSECS, &s.outsecs);
    table(&mut out, kind::SYMBOLS, &s.symbols);
    table(&mut out, kind::SECTIONS, &s.sections);
    table(&mut out, kind::CHUNKS, &s.chunks);
    table(&mut out, kind::ORDER, &s.order);
    table(&mut out, kind::SEGMENTS, &s.segments);
    table(&mut out, kind::FRAGS, &s.frags);
    table(&mut out, kind::SLOTS, &s.slots);
    table(&mut out, kind::EH_RECORDS, &s.eh_records);
    table(&mut out, kind::SYMTAB_LOCALS, &s.symtab_locals);
    table(&mut out, kind::SIZES, &s.sizes);
    table(&mut out, kind::SYM_PLACE, &s.sym_place);
    table(&mut out, kind::DEFINED, &s.defined);
    table(&mut out, kind::PATCHES, &s.patches);
    table(&mut out, kind::REDIRECTS, &s.redirects);
    table(&mut out, kind::SIZED, &s.sized);
    table(&mut out, kind::LAYOUT, std::slice::from_ref(&s.layout));
    table(&mut out, kind::LEAVES, &s.leaves);
    table(&mut out, kind::MENTIONS, &s.mentions);
    table(&mut out, kind::RESOLVED, &s.resolved);
    table(&mut out, kind::INDEX, &s.index);
    table(&mut out, kind::HOLES, &s.holes);
    table(&mut out, kind::FRAG_AT, &s.frag_at);
    record(&mut out, kind::STRINGS, s.strings.len() as u64, &s.strings);
    let len = out.len() as u64;
    let mut header = Vec::with_capacity(HEADER_LEN);
    let mut w = W(&mut header);
    w.bytes(&MAGIC).u32(FORMAT_VERSION).u32(s.flags).u32(kind::COUNT).u32(0).u64(len);
    w.bytes(&s.plan_hash);
    let hash = header_hash(&header);
    header.extend_from_slice(&hash);
    out[..HEADER_LEN].copy_from_slice(&header);
    out
}

/// The header's flags word at offset 12, to set and clear the updating
/// flag in place (IN-46).
pub const FLAGS_OFFSET: u64 = 12;

// ---- decoding -------------------------------------------------------------------

struct Tables<'a> {
    bytes: &'a [u8],
    at: usize,
}

impl<'a> Tables<'a> {
    /// The next table's rows: its record checked (kind, size, hash). The
    /// caller asks for the kinds in file order.
    fn next(&mut self, kind: u32, width: usize) -> Result<(u64, &'a [u8]), StateError> {
        let name = kind::name(kind);
        let rec = self
            .at
            .checked_add(RECORD_LEN)
            .and_then(|end| self.bytes.get(self.at..end))
            .ok_or_else(|| corrupt(format!("{name}: truncated record")))?;
        let mut r = R::new(rec);
        let (k, pad, rows, len, hash) = (r.u32(), r.u32(), r.u64(), r.u64(), r.h());
        if k != kind {
            return Err(corrupt(format!("table {k} where {name} ({kind}) was expected")));
        }
        if pad != 0 {
            return Err(corrupt(format!("{name}: record pad not zero")));
        }
        if rows.checked_mul(width as u64) != Some(len) {
            return Err(corrupt(format!("{name}: {len} bytes for {rows} rows of {width}")));
        }
        let start = self.at + RECORD_LEN;
        let body = usize::try_from(len)
            .ok()
            .and_then(|l| start.checked_add(l))
            .and_then(|end| self.bytes.get(start..end))
            .ok_or_else(|| corrupt(format!("{name}: rows past the end of the state")))?;
        if l128(body) != hash {
            return Err(corrupt(format!("{name}: hash of the rows differs")));
        }
        self.at = start + body.len();
        Ok((rows, body))
    }

    fn rows<T: Row>(&mut self, kind: u32) -> Result<Vec<T>, StateError> {
        let (_, body) = self.next(kind, T::WIDTH)?;
        let mut out = Vec::with_capacity(body.len() / T::WIDTH.max(1));
        for (i, chunk) in body.chunks_exact(T::WIDTH).enumerate() {
            let mut r = R::new(chunk);
            let row = T::get(&mut r);
            if r.bad_pad {
                return Err(corrupt(format!("{} row {i}: pad not zero", kind::name(kind))));
            }
            out.push(row);
        }
        Ok(out)
    }

    fn one<T: Row>(&mut self, kind: u32) -> Result<T, StateError> {
        let mut rows = self.rows::<T>(kind)?;
        if rows.len() != 1 {
            return Err(corrupt(format!("{}: {} rows, not 1", kind::name(kind), rows.len())));
        }
        Ok(rows.remove(0))
    }
}

/// The header's fields, checked: what [`decode`] reads first.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Header {
    pub flags: u32,
    pub plan_hash: Hash16,
}

/// Reads and checks the header alone (magic, version, hash, length).
pub fn header(bytes: &[u8]) -> Result<Header, StateError> {
    let head = bytes.get(..HEADER_LEN).ok_or_else(|| corrupt("shorter than its header"))?;
    let mut r = R::new(head);
    if r.take::<8>() != MAGIC {
        return Err(corrupt("not a link state (magic)"));
    }
    let version = r.u32();
    // Version 1 (no MENTIONS, RESOLVED, INDEX, HOLES) is refused by name
    // like any other: an update cannot start from it (IN-18a).
    if version != FORMAT_VERSION {
        return Err(StateError::Version(format!(
            "format version {version}, this mortar reads {FORMAT_VERSION}"
        )));
    }
    let tables = kind::COUNT;
    let flags = r.u32();
    let (count, pad, len, plan_hash, hash) = (r.u32(), r.u32(), r.u64(), r.h(), r.h());
    if header_hash(head) != hash {
        return Err(corrupt("header hash differs"));
    }
    if flags & !FLAG_UPDATING != 0 {
        return Err(corrupt(format!("unknown header flags {flags:#x}")));
    }
    if count != tables || pad != 0 {
        return Err(corrupt(format!("{count} tables, format {version} has {tables}")));
    }
    if len != bytes.len() as u64 {
        return Err(corrupt(format!("{} bytes, the header says {len}", bytes.len())));
    }
    Ok(Header { flags, plan_hash })
}

/// Decodes and validates a state (IN-18a's reader). `version` is this
/// mortar's version string: a state of another is `state-version`.
pub fn decode(bytes: &[u8], version: &[u8]) -> Result<State, StateError> {
    let Header { flags, plan_hash } = header(bytes)?;
    let mut t = Tables { bytes, at: HEADER_LEN };
    let mut s = State {
        flags,
        plan_hash,
        meta: t.one(kind::META)?,
        stat: t.rows(kind::STAT)?,
        files: t.rows(kind::FILES)?,
        probes: t.rows(kind::PROBES)?,
        outsecs: t.rows(kind::OUTSECS)?,
        symbols: t.rows(kind::SYMBOLS)?,
        sections: t.rows(kind::SECTIONS)?,
        chunks: t.rows(kind::CHUNKS)?,
        order: t.rows(kind::ORDER)?,
        segments: t.rows(kind::SEGMENTS)?,
        frags: t.rows(kind::FRAGS)?,
        slots: t.rows(kind::SLOTS)?,
        eh_records: t.rows(kind::EH_RECORDS)?,
        symtab_locals: t.rows(kind::SYMTAB_LOCALS)?,
        sizes: t.rows(kind::SIZES)?,
        sym_place: t.rows(kind::SYM_PLACE)?,
        defined: t.rows(kind::DEFINED)?,
        patches: t.rows(kind::PATCHES)?,
        redirects: t.rows(kind::REDIRECTS)?,
        sized: t.rows(kind::SIZED)?,
        layout: t.one(kind::LAYOUT)?,
        leaves: t.rows(kind::LEAVES)?,
        mentions: t.rows(kind::MENTIONS)?,
        resolved: t.rows(kind::RESOLVED)?,
        index: t.rows(kind::INDEX)?,
        holes: t.rows(kind::HOLES)?,
        frag_at: t.rows(kind::FRAG_AT)?,
        strings: Vec::new(),
    };
    let (_, strings) = t.next(kind::STRINGS, 1)?;
    s.strings = strings.to_vec();
    if t.at != bytes.len() {
        return Err(corrupt("bytes after the last table"));
    }
    validate(&s)?;
    if s.str(s.meta.version) != version {
        return Err(StateError::Version(format!(
            "written by mortar {}, this is {}",
            String::from_utf8_lossy(s.str(s.meta.version)),
            String::from_utf8_lossy(version)
        )));
    }
    Ok(s)
}

/// The checks of every row (IN-18a's reader, after the tables' hashes).
/// A `SYMBOLS` row's key, for the duplicate check.
type SymbolKey<'k> = (u8, u32, u32, &'k [u8], &'k [u8]);

#[allow(clippy::too_many_lines)] // one block per table, in IN-18a's order
fn validate(s: &State) -> Result<(), StateError> {
    let strings = s.strings.len() as u64;
    let str_ok = |x: Str| u64::from(x.off) + u64::from(x.len) <= strings;
    let reference = |n: usize, v: u32, none: bool| (v as usize) < n || (none && v == NONE);
    let bad = |table: u32, row: usize, what: &str| {
        corrupt(format!("{} row {row}: {what}", kind::name(table)))
    };
    let m = &s.meta;
    if !str_ok(m.version) || !str_ok(m.build_id) {
        return Err(bad(kind::META, 0, "string out of range"));
    }
    if !matches!(m.class, 1 | 2) || m.output_kind != 0 || m.leaf_fn > 2 {
        return Err(bad(kind::META, 0, "enumeration out of range"));
    }
    if m.sealed_start > m.sealed_end || m.sealed_end > m.file_size {
        return Err(bad(kind::META, 0, "build-id descriptor outside the output"));
    }
    // The descriptor holds exactly the build id, which is short (I-73: 20
    // or 32 bytes for a hash; a given id at most MAX_BUILD_ID): the output
    // check reads that many bytes and no more.
    if m.build_id.len > MAX_BUILD_ID || m.sealed_end - m.sealed_start != u64::from(m.build_id.len) {
        return Err(bad(kind::META, 0, "build-id descriptor is not the build id's length"));
    }
    // STAT: device rows by dev, then inputs, then one output row.
    let mut phase = stat_tag::DEVICE;
    let mut last_dev = None;
    for (i, row) in s.stat.iter().enumerate() {
        if row.tag > stat_tag::OUTPUT || row.trust > 1 || row.tag < phase {
            return Err(bad(kind::STAT, i, "tag or trust out of range or out of order"));
        }
        if !str_ok(row.path) || !str_ok(row.fstype) {
            return Err(bad(kind::STAT, i, "string out of range"));
        }
        if row.tag == stat_tag::DEVICE {
            if last_dev.is_some_and(|d| d >= row.dev) {
                return Err(bad(kind::STAT, i, "device rows not sorted"));
            }
            last_dev = Some(row.dev);
        }
        if phase == stat_tag::OUTPUT {
            return Err(bad(kind::STAT, i, "a row after the output's"));
        }
        phase = row.tag;
    }
    if phase != stat_tag::OUTPUT {
        return Err(corrupt("STAT: no output row"));
    }
    let output = s.stat.last().map_or(0, |o| o.size);
    if output != m.file_size {
        return Err(corrupt(format!(
            "META: output size {} but the output row says {output}",
            m.file_size
        )));
    }
    let mut paths: Vec<&[u8]> =
        s.stat.iter().filter(|r| r.tag == stat_tag::INPUT).map(|r| s.str(r.path)).collect();
    paths.sort_unstable();
    if paths.windows(2).any(|w| w[0] == w[1]) {
        return Err(corrupt("STAT: an input path appears twice"));
    }
    for (i, f) in s.files.iter().enumerate() {
        if f.kind > 2 || f.member > 1 || !str_ok(f.path) || !str_ok(f.member_name) {
            return Err(bad(kind::FILES, i, "field out of range"));
        }
        if f.member == 0 && (f.member_name != Str::default() || f.member_ordinal != 0) {
            return Err(bad(kind::FILES, i, "member fields of a file that is not a member"));
        }
        if f.base.checked_add(f.len).is_none() {
            return Err(bad(kind::FILES, i, "byte range past 2^64"));
        }
    }
    for (i, p) in s.probes.iter().enumerate() {
        if p.present > 1 || !str_ok(p.path) {
            return Err(bad(kind::PROBES, i, "field out of range"));
        }
    }
    let mut seen: HashMap<&[u8], u32> = HashMap::new();
    for (i, o) in s.outsecs.iter().enumerate() {
        if !str_ok(o.name) {
            return Err(bad(kind::OUTSECS, i, "string out of range"));
        }
        let n = seen.entry(s.str(o.name)).or_insert(0);
        if *n != o.ordinal {
            return Err(bad(kind::OUTSECS, i, "ordinal is not its rank among its name"));
        }
        *n += 1;
    }
    let (files, outsecs, symbols) = (s.files.len(), s.outsecs.len(), s.symbols.len());
    let (sections, chunks, patches) = (s.sections.len(), s.chunks.len(), s.patches.len());
    for (i, y) in s.symbols.iter().enumerate() {
        let ok = match y.kind {
            sym_kind::GLOBAL => y.file == NONE && y.ordinal == 0 && y.version == Str::default(),
            sym_kind::VERSIONED => y.file == NONE && y.ordinal == 0,
            sym_kind::LOCAL => {
                reference(files, y.file, false)
                    && y.version == Str::default()
                    && y.at_file == y.file
            }
            _ => false,
        };
        let ok = ok
            && reference(files, y.at_file, y.kind != sym_kind::LOCAL)
            && (y.at_file != NONE || y.at_index == 0);
        if !ok || !str_ok(y.name) || !str_ok(y.version) {
            return Err(bad(kind::SYMBOLS, i, "field out of range"));
        }
    }
    let mut sorted: Vec<SymbolKey<'_>> = s
        .symbols
        .iter()
        .map(|y| (y.kind, y.file, y.ordinal, s.str(y.name), s.str(y.version)))
        .collect();
    sorted.sort_unstable();
    if sorted.windows(2).any(|w| w[0] == w[1]) {
        return Err(corrupt("SYMBOLS: a key appears twice"));
    }
    for (i, x) in s.sections.iter().enumerate() {
        let ok = reference(files, x.file, false)
            && x.anchor_kind <= 3
            && (x.anchor_kind != 0 || x.anchor == Str::default())
            && str_ok(x.anchor)
            && str_ok(x.name)
            && reference(chunks, x.place_chunk, true)
            && (x.place_chunk != NONE || x.place_offset == 0);
        if !ok {
            return Err(bad(kind::SECTIONS, i, "field out of range"));
        }
    }
    for (i, c) in s.chunks.iter().enumerate() {
        let ok = c.tag <= 8
            && c.bits & !chunk_bits::ALL == 0
            && c.align < 64
            && str_ok(c.name)
            && reference(outsecs, c.out, true)
            && (c.bits & chunk_bits::HAS_ADDR != 0 || c.addr == 0)
            && (c.bits & chunk_bits::HAS_SHNDX != 0 || c.shndx == 0)
            && c.size <= c.capacity;
        if !ok {
            return Err(bad(kind::CHUNKS, i, "field out of range"));
        }
    }
    let mut placed = vec![false; chunks];
    for (i, &c) in s.order.iter().enumerate() {
        if !reference(chunks, c, false) {
            return Err(bad(kind::ORDER, i, "chunk out of range"));
        }
        if std::mem::replace(&mut placed[c as usize], true) {
            return Err(bad(kind::ORDER, i, "a chunk appears twice"));
        }
    }
    if let Some(c) = placed.iter().position(|p| !p) {
        return Err(corrupt(format!("ORDER: chunk {c} is not in the order")));
    }
    for (i, g) in s.segments.iter().enumerate() {
        if g.align >= 64 {
            return Err(bad(kind::SEGMENTS, i, "alignment out of range"));
        }
    }
    for (i, f) in s.frags.iter().enumerate() {
        if !reference(outsecs, f.out, false) {
            return Err(bad(kind::FRAGS, i, "output section out of range"));
        }
    }
    for (i, w) in s.frags.windows(2).enumerate() {
        if (w[0].out, w[0].hash, w[0].ordinal) >= (w[1].out, w[1].hash, w[1].ordinal) {
            return Err(bad(kind::FRAGS, i + 1, "rows not sorted"));
        }
    }
    for (i, f) in s.frag_at.iter().enumerate() {
        if !reference(s.sections.len(), f.section, false) {
            return Err(bad(kind::FRAG_AT, i, "section out of range"));
        }
    }
    for (i, w) in s.frag_at.windows(2).enumerate() {
        if (w[0].section, w[0].offset) >= (w[1].section, w[1].offset) {
            return Err(bad(kind::FRAG_AT, i + 1, "rows not sorted"));
        }
    }
    let mut last_kind = 0;
    for (i, x) in s.slots.iter().enumerate() {
        if x.kind > 5 || x.kind < last_kind || !reference(symbols, x.symbol, false) {
            return Err(bad(kind::SLOTS, i, "field out of range or out of order"));
        }
        last_kind = x.kind;
    }
    let mut slots: Vec<(u8, u32)> = s.slots.iter().map(|x| (x.kind, x.symbol)).collect();
    slots.sort_unstable();
    if slots.windows(2).any(|w| w[0] == w[1]) {
        return Err(corrupt("SLOTS: a symbol appears twice in one list"));
    }
    for (i, e) in s.eh_records.iter().enumerate() {
        if !reference(sections, e.section, false) {
            return Err(bad(kind::EH_RECORDS, i, "section out of range"));
        }
    }
    if !s.symtab_locals.is_empty() && s.symtab_locals.len() != files {
        return Err(corrupt("SYMTAB_LOCALS: neither empty nor one row per file"));
    }
    for (table, rows) in [(kind::SIZES, &s.sizes), (kind::SIZED, &s.sized)] {
        for (i, z) in rows.iter().enumerate() {
            if !reference(chunks, z.chunk, false) {
                return Err(bad(table, i, "chunk out of range"));
            }
        }
    }
    for (i, p) in s.sym_place.iter().enumerate() {
        let ok = p.kind == 0
            && reference(symbols, p.symbol, false)
            && reference(chunks, p.chunk, false)
            && p.align < 64;
        if !ok {
            return Err(bad(kind::SYM_PLACE, i, "field out of range"));
        }
    }
    for (i, d) in s.defined.iter().enumerate() {
        let ok = reference(symbols, d.symbol, false)
            && match d.kind {
                0 => d.chunk == NONE,
                1 => reference(chunks, d.chunk, false),
                _ => false,
            };
        if !ok {
            return Err(bad(kind::DEFINED, i, "field out of range"));
        }
    }
    let mut defined: Vec<u32> = s.defined.iter().map(|d| d.symbol).collect();
    defined.sort_unstable();
    if defined.windows(2).any(|w| w[0] == w[1]) {
        return Err(corrupt("DEFINED: a symbol appears twice"));
    }
    for (i, p) in s.patches.iter().enumerate() {
        let ok = reference(chunks, p.chunk, false)
            && match p.kind {
                patch_kind::THUNK => reference(symbols, p.symbol, false) && p.section == NONE,
                patch_kind::THUNK_PLT => {
                    reference(symbols, p.symbol, false) && p.section == NONE && p.value == 0
                }
                patch_kind::E843419 => p.symbol == NONE && reference(sections, p.section, false),
                _ => false,
            };
        if !ok {
            return Err(bad(kind::PATCHES, i, "field out of range"));
        }
    }
    for (i, r) in s.redirects.iter().enumerate() {
        if !reference(sections, r.section, false) || !reference(patches, r.patch, false) {
            return Err(bad(kind::REDIRECTS, i, "reference out of range"));
        }
    }
    let l = &s.layout;
    let ok = l.bits & !layout_bits::ALL == 0
        && l.tls_align < 64
        && (l.bits & layout_bits::TLS != 0 || (l.tls_start, l.tls_size, l.tls_align) == (0, 0, 0))
        && (l.bits & layout_bits::ENTRY != 0 || l.entry == 0);
    if !ok {
        return Err(bad(kind::LAYOUT, 0, "field out of range"));
    }
    let want = mortar_digest::leaf_count(m.file_size);
    if s.leaves.len() as u64 != want {
        return Err(corrupt(format!("LEAVES: {} rows, the output needs {want}", s.leaves.len())));
    }
    for (i, h) in s.holes.iter().enumerate() {
        let ok = h.kind <= hole_kind::DEAD_CIE
            && h.size != 0
            && s.chunks
                .get(h.chunk as usize)
                .is_some_and(|c| h.offset.checked_add(h.size).is_some_and(|end| end <= c.size));
        if !ok {
            return Err(bad(kind::HOLES, i, "field out of range"));
        }
    }
    for (i, w) in s.holes.windows(2).enumerate() {
        if (w[0].chunk, w[0].offset + w[0].size) > (w[1].chunk, w[1].offset) {
            return Err(bad(kind::HOLES, i + 1, "rows not sorted or overlapping"));
        }
    }
    let leaf_len = match m.leaf_fn {
        0 => 16,
        1 => 20,
        _ => 32,
    };
    if s.leaves.iter().any(|l| l[leaf_len..].iter().any(|&b| b != 0)) {
        return Err(corrupt("LEAVES: bytes past the digest not zero"));
    }
    validate_delta(s)
}

/// The checks of version 2's delta tables: references in range, rows
/// sorted and unique, enumerations known.
fn validate_delta(s: &State) -> Result<(), StateError> {
    let strings = s.strings.len() as u64;
    let str_ok = |x: Str| u64::from(x.off) + u64::from(x.len) <= strings;
    let (files, symbols, sections) = (s.files.len(), s.symbols.len(), s.sections.len());
    let reference = |n: usize, v: u32, none: bool| (v as usize) < n || (none && v == NONE);
    let bad = |table: u32, row: usize, what: &str| {
        corrupt(format!("{} row {row}: {what}", kind::name(table)))
    };
    for (i, m) in s.mentions.iter().enumerate() {
        let ok = reference(files, m.file, false)
            && reference(symbols, m.symbol, false)
            && m.kind <= def_kind::COMMON
            && reference(sections, m.section, true)
            && (m.kind == def_kind::SECTION) == (m.section != NONE);
        if !ok {
            return Err(bad(kind::MENTIONS, i, "field out of range"));
        }
    }
    for (i, w) in s.mentions.windows(2).enumerate() {
        if (w[0].file, w[0].symbol) >= (w[1].file, w[1].symbol) {
            return Err(bad(kind::MENTIONS, i + 1, "rows not sorted"));
        }
    }
    // RESOLVED: the keys' rows by symbol, then the groups' by name, then
    // the live files' by file.
    let mut last: (u8, u32, &[u8]) = (0, 0, &[]);
    for (i, r) in s.resolved.iter().enumerate() {
        let ok = match r.kind {
            def_kind::UNDEFINED..=def_kind::SHARED => {
                reference(symbols, r.symbol, false)
                    && reference(files, r.file, true)
                    && reference(sections, r.section, true)
                    && r.name == Str::default()
            }
            def_kind::COMDAT => {
                r.symbol == NONE
                    && reference(files, r.file, false)
                    && r.section == NONE
                    && str_ok(r.name)
            }
            def_kind::LIVE => {
                r.symbol == NONE
                    && reference(files, r.file, false)
                    && r.section == NONE
                    && r.name == Str::default()
            }
            _ => false,
        };
        if !ok {
            return Err(bad(kind::RESOLVED, i, "field out of range"));
        }
        let class = match r.kind {
            def_kind::COMDAT => 1,
            def_kind::LIVE => 2,
            _ => 0,
        };
        let key: (u8, u32, &[u8]) = match class {
            0 => (0, r.symbol, &[]),
            1 => (1, 0, s.str(r.name)),
            _ => (2, r.file, &[]),
        };
        if i > 0 && key <= last {
            return Err(bad(kind::RESOLVED, i, "rows not sorted"));
        }
        last = key;
    }
    for (i, x) in s.index.iter().enumerate() {
        let ok = reference(sections, x.unit, false)
            && match x.kind {
                index_kind::GLOBAL | index_kind::SLOT => reference(symbols, x.target, false),
                index_kind::SECTION
                | index_kind::EH_FRAME
                | index_kind::LINK_ORDER
                | index_kind::LOCAL_SLOT => reference(sections, x.target, false),
                _ => false,
            };
        if !ok {
            return Err(bad(kind::INDEX, i, "field out of range"));
        }
    }
    for (i, w) in s.index.windows(2).enumerate() {
        if w[0] >= w[1] {
            return Err(bad(kind::INDEX, i + 1, "rows not sorted"));
        }
    }
    Ok(())
}
