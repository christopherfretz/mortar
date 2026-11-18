//! Delta resolution and the refill set (`spec/incr.md` IN-25..IN-29,
//! IN-27's table, IN-34a's references into `.eh_frame`).
//!
//! [`compute`] compares two states in identity terms: the old one (the
//! previous link) and the new one (a link of the present inputs, with
//! its resolution, layout and liveness). In the dry run of WP-063 the new
//! side is the full link that runs anyway: its resolution is IN-26's
//! re-resolution (which must equal a full link's), its liveness IN-29's,
//! and its layout the full link's. The refill set is therefore the set
//! of units that may differ between the two full links' outputs, which is
//! what the two-full-link oracle (IN-63a) checks. WP-064 calls the same
//! function with the update's layout as the new side.
//!
//! Every unit carries every reason that put it in the set (one bit per
//! rule, [`reason`]); dropping a rule (`drop`, IN-63b's mutations) is
//! removing its bit. A rule that cannot answer exactly is a named
//! fall-back (§7), never "unchanged".

use mortar_core::hash::{HashMap, HashSet};
use mortar_core::incr::IncrReason;

use crate::detect::{Changes, FileState};
use crate::format::{
    ChunkRow, EhRow, Hash16, MentionRow, NONE, ResolvedRow, SectionRow, State, def_kind, index_kind,
};

/// The reasons a unit, hole or chunk is in the set: one bit per rule.
pub mod reason {
    /// IN-27 row 1: its file changed, appeared or was extracted.
    pub const FILE: u32 = 1 << 0;
    /// IN-27 row 2: its file disappeared or is no longer extracted (holes).
    pub const GONE: u32 = 1 << 1;
    /// IN-27 row 3: a global it names was defined elsewhere, moved, or
    /// changed value, size or kind.
    pub const GLOBAL: u32 = 1 << 2;
    /// IN-27 row 4: a COMDAT keeper changed (the group's members, the
    /// referrers of its symbols).
    pub const COMDAT: u32 = 1 << 3;
    /// IN-27 row 4: ... the old and the new keeper's `.eh_frame` and
    /// non-allocated units.
    pub const COMDAT_FILE: u32 = 1 << 4;
    /// IN-27 rows 5 and 7: a slot table moved or changed (GOT, PLT,
    /// IPLT, TLS slots; demand flags are slot decisions in a static
    /// link): the units that use a slot, and the IPLT keys' referrers.
    pub const SLOTS: u32 = 1 << 5;
    /// IN-27 row 6: the section of an unchanged file moved.
    pub const MOVED: u32 = 1 << 6;
    /// IN-27 row 6: ... its referrers through its own file's symbols.
    pub const MOVED_REF: u32 = 1 << 7;
    /// IN-27 row 6: ... the non-allocated units of its file.
    pub const MOVED_FILE: u32 = 1 << 8;
    /// A fragment of a merged section of an unchanged file got another
    /// offset (two full links; an update appends, S-110): that file's
    /// non-allocated units and its section's referrers.
    pub const FRAG: u32 = 1 << 9;
    /// IN-27 row 7: a synthetic table (chunks only).
    pub const SYNTH: u32 = 1 << 10;
    /// IN-27 row 8: a patch or thunk was added, moved or removed.
    pub const PATCH: u32 = 1 << 11;
    /// IN-27 row 9: liveness changed (gc): the section, its globals'
    /// referrers.
    pub const LIVE: u32 = 1 << 12;
    /// IN-27 row 9: ... its file's `.eh_frame` and non-allocated units.
    pub const LIVE_FILE: u32 = 1 << 13;
    /// IN-34a: an `.eh_frame` contribution after the first record that
    /// differs (per-file ranges in full-link order).
    pub const EH: u32 = 1 << 14;
    /// IN-27 row 10: a reference into an input `.eh_frame` (S-50).
    pub const EH_REF: u32 = 1 << 15;
    /// The file header, program and section headers (chunks only).
    pub const HEADERS: u32 = 1 << 16;
    /// A body chunk's fill between units (chunks only).
    pub const GAPS: u32 = 1 << 17;

    /// The names, in bit order (the report's spelling).
    pub const NAMES: [&str; 18] = [
        "file",
        "gone",
        "global",
        "comdat",
        "comdat-file",
        "slots",
        "moved",
        "moved-ref",
        "moved-file",
        "frag",
        "synthetic",
        "patch",
        "liveness",
        "liveness-file",
        "eh-frame",
        "eh-ref",
        "headers",
        "gaps",
    ];

    /// `a,b,c` for a mask.
    #[must_use]
    pub fn names(mask: u32) -> String {
        let mut out = String::new();
        for (i, n) in NAMES.iter().enumerate() {
            if mask & (1 << i) != 0 {
                if !out.is_empty() {
                    out.push(',');
                }
                out.push_str(n);
            }
        }
        out
    }
}

/// IN-63b's rows: a name, the reason bits it sets, and the fall-backs it
/// raises. Dropping a row removes both.
pub const ROWS: [(&str, u32, &[&str]); 14] = [
    ("changed", reason::FILE, &[]),
    ("gone", reason::GONE, &[]),
    ("global", reason::GLOBAL, &[]),
    ("comdat", reason::COMDAT | reason::COMDAT_FILE, &[]),
    ("slots", reason::SLOTS, &[]),
    ("moved", reason::MOVED | reason::MOVED_REF | reason::MOVED_FILE, &[]),
    ("frag", reason::FRAG, &[]),
    ("synthetic", reason::SYNTH, &[]),
    ("patch", reason::PATCH, &[]),
    ("liveness", reason::LIVE | reason::LIVE_FILE, &[]),
    ("eh-frame", reason::EH, &[]),
    ("eh-ref", reason::EH_REF, &[]),
    ("chunks", reason::HEADERS | reason::GAPS, &[]),
    (
        "fallbacks",
        0,
        &[
            "tls-layout-changed",
            "symbol-kind-changed",
            "new-output-section",
            "property-changed",
            "link-order-moved",
            "inputs-reordered",
            "options-changed",
        ],
    ),
];

/// What a mutation (IN-63b) drops: reason bits and fall-back names.
#[derive(Debug, Clone, Copy, Default)]
pub struct Drop<'a> {
    pub reasons: u32,
    pub fallbacks: &'a [&'a str],
}

impl Drop<'_> {
    /// Nothing dropped.
    pub const NONE: Drop<'static> = Drop { reasons: 0, fallbacks: &[] };
}

/// A fraction of the policy constant IN-16 names: `too-much-changed`
/// when more than `num/den` of the allocated bytes are refilled or of
/// the live inputs changed.
pub const TOO_MUCH: (u64, u64) = (1, 2);

/// What the new link knows that its state does not store.
#[derive(Debug, Clone, Default)]
pub struct Extra {
    /// Per new `SECTIONS` row, its section index in its file (the
    /// report names units by it, as `--mortar-map` does).
    pub shndx: Vec<u32>,
    /// The fragments of the new link's merged sections: `(SECTIONS row,
    /// OUTSECS row, content hash)`.
    pub frags: Vec<(u32, u32, Hash16)>,
    /// The new link's `.eh_frame` pieces: `(SECTIONS row, first index in
    /// eh_cie)` sorted by row, and per piece its CIE's position among its
    /// section's pieces (`u32::MAX` for a CIE, `split.md` S-45).
    pub eh_sections: Vec<(u32, u32)>,
    pub eh_cie: Vec<u32>,
}

/// What an `.eh_frame` piece of the new link is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EhCie {
    Cie,
    /// An FDE, with its CIE's position among its section's pieces.
    Fde(u32),
}

impl Extra {
    /// The number of `.eh_frame` pieces of section `section` in the new
    /// link.
    #[must_use]
    pub fn eh_pieces(&self, section: u32) -> Option<u32> {
        let at = self.eh_sections.binary_search_by_key(&section, |x| x.0).ok()?;
        let end = self.eh_sections.get(at + 1).map_or(self.eh_cie.len(), |x| x.1 as usize);
        Some(end as u32 - self.eh_sections[at].1)
    }

    /// Piece `piece` of section `section`: `None` when the new link has
    /// no such piece.
    #[must_use]
    pub fn eh_cie(&self, section: u32, piece: u32) -> Option<EhCie> {
        let at = self.eh_sections.binary_search_by_key(&section, |x| x.0).ok()?;
        let (_, first) = self.eh_sections[at];
        let end = self.eh_sections.get(at + 1).map_or(self.eh_cie.len(), |x| x.1 as usize);
        let i = first as usize + piece as usize;
        if i >= end {
            return None;
        }
        Some(if self.eh_cie[i] == u32::MAX { EhCie::Cie } else { EhCie::Fde(self.eh_cie[i]) })
    }
}

/// A refilled unit (a new `SECTIONS` row).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Unit {
    pub section: u32,
    pub reasons: u32,
}

/// A hole: an old placed unit whose bytes are not where the new link
/// puts that unit (an old `SECTIONS` row; `new` its row now, if any).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Hole {
    pub old: u32,
    pub new: u32,
    pub reasons: u32,
}

/// A refilled chunk (a new `CHUNKS` row).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ChunkLine {
    pub chunk: u32,
    pub reasons: u32,
}

/// A named fall-back (§7) and what triggered it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Fallback {
    /// The reason, with its detail (WP-213: the one reason type).
    pub reason: IncrReason,
}

/// The symbol part of the delta.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct SymCounts {
    pub new: usize,
    pub gone: usize,
    pub definer: usize,
    pub value: usize,
    pub size: usize,
    pub kind: usize,
    pub attrs: usize,
    pub comdat: usize,
    pub commons: usize,
    pub live: usize,
    pub dead: usize,
    pub tls: usize,
}

/// The delta and the refill set.
#[derive(Debug, Clone, Default)]
pub struct Delta {
    pub symbols: SymCounts,
    /// `(what, key name)` per global that changed.
    pub symbol_lines: Vec<(&'static str, String)>,
    pub fallbacks: Vec<Fallback>,
    /// Policy (IN-16), not correctness: `too-much-changed` and why.
    pub policy: Option<String>,
    pub units: Vec<Unit>,
    pub holes: Vec<Hole>,
    pub chunks: Vec<ChunkLine>,
    /// The allocated bytes of the units, and of the output.
    pub unit_bytes: u64,
    pub alloc_bytes: u64,
}

const SHF_ALLOC: u64 = 2;
const SHF_TLS: u64 = 0x400;
const EM_AARCH64: u16 = 183;
/// `ChunkTag` bytes (`capture::TAGS`).
const TAG_BODY: u8 = 3;
const TAG_MERGED: u8 = 4;
const TAG_PATCHES: u8 = 5;
const TAG_SYNTHETIC: u8 = 6;

/// A global key's identity: kind (global, versioned), name, version.
type KeyId<'s> = (u8, &'s [u8], &'s [u8]);

fn key_id(s: &State, row: u32) -> Option<KeyId<'_>> {
    let y = s.symbols.get(row as usize)?;
    Some((y.kind, s.str(y.name), s.str(y.version)))
}

/// A section's identity within its file: anchor, name, ordinal.
type SecId<'s> = (u8, &'s [u8], &'s [u8], u32);

fn sec_id<'s>(s: &'s State, x: &SectionRow) -> SecId<'s> {
    (x.anchor_kind, s.str(x.anchor), s.str(x.name), x.ordinal)
}

/// Where a placed unit is: allocated by address; non-allocated by its
/// output section and offset in it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Pos<'s> {
    Alloc(u64),
    NonAlloc(&'s [u8], u32, u64),
}

/// A merged fragment: output section number, hash, place.
type Frag = (u32, Hash16, Option<u64>);

/// Items grouped by a key in `0..n`, each group in push order (a
/// counting sort, no map). A key outside `0..n` (a damaged state) is kept
/// aside and searched, so a lookup is never less than what was pushed.
struct Csr {
    start: Vec<u32>,
    items: Vec<u32>,
    other: Vec<(u32, u32)>,
}

impl Csr {
    fn new(n: usize, pairs: impl Iterator<Item = (u32, u32)> + Clone) -> Self {
        let mut start = vec![0u32; n + 1];
        let mut other = Vec::new();
        for (k, v) in pairs.clone() {
            match start.get_mut(k as usize + 1) {
                Some(c) if (k as usize) < n => *c += 1,
                _ => other.push((k, v)),
            }
        }
        let mut sum = 0;
        for c in &mut start {
            sum += *c;
            *c = sum;
        }
        let mut fill = start[..n].to_vec();
        let mut items = vec![0; sum as usize];
        for (k, v) in pairs {
            if let Some(at) = fill.get_mut(k as usize) {
                items[*at as usize] = v;
                *at += 1;
            }
        }
        Self { start, items, other }
    }

    fn get(&self, k: u32) -> impl Iterator<Item = u32> + '_ {
        let k = k as usize;
        let range = match (self.start.get(k), self.start.get(k + 1)) {
            (Some(&a), Some(&b)) => a as usize..b as usize,
            _ => 0..0,
        };
        self.items[range]
            .iter()
            .copied()
            .chain(self.other.iter().filter(move |x| x.0 as usize == k).map(|x| x.1))
    }
}

struct Side<'s> {
    s: &'s State,
    /// Per `OUTSECS` row, the least file offset of its chunks.
    out_start: Vec<u64>,
    /// `DEFINED` and `SYM_PLACE` rows by key (the first row of a key),
    /// and per section its `SECTION`-kind mentions: indexed once, so a
    /// lookup per key or per flipped section is not a scan.
    defined_at: HashMap<u32, usize>,
    common_at: HashMap<u32, usize>,
    globals_of: std::cell::OnceCell<Csr>,
    /// Per `FILES` row, its `SECTIONS` rows.
    by_file: Csr,
    /// `RESOLVED`'s COMDAT block and whether it is sorted (on first use).
    comdat: std::cell::OnceCell<(usize, usize, bool)>,
}

impl<'s> Side<'s> {
    fn new(s: &'s State) -> Self {
        let mut out_start = vec![u64::MAX; s.outsecs.len()];
        for c in &s.chunks {
            if let Some(o) = out_start.get_mut(c.out as usize) {
                *o = (*o).min(c.offset);
            }
        }
        let mut defined_at = HashMap::default();
        for (i, d) in s.defined.iter().enumerate() {
            defined_at.entry(d.symbol).or_insert(i);
        }
        let mut common_at = HashMap::default();
        for (i, p) in s.sym_place.iter().enumerate() {
            common_at.entry(p.symbol).or_insert(i);
        }
        let by_file =
            Csr::new(s.files.len(), s.sections.iter().enumerate().map(|(i, x)| (x.file, i as u32)));
        Self {
            s,
            out_start,
            defined_at,
            common_at,
            globals_of: std::cell::OnceCell::new(),
            by_file,
            comdat: std::cell::OnceCell::new(),
        }
    }

    /// The `SECTION`-kind mentions of section `sec`, in `MENTIONS` order.
    fn globals_in(&self, sec: u32) -> impl Iterator<Item = &'s MentionRow> + '_ {
        let s = self.s;
        // Built on first use: only a liveness flip asks.
        let csr = self.globals_of.get_or_init(|| {
            Csr::new(
                s.sections.len(),
                s.mentions
                    .iter()
                    .enumerate()
                    .filter(|(_, x)| x.kind == def_kind::SECTION)
                    .map(|(i, x)| (x.section, i as u32)),
            )
        });
        csr.get(sec).map(move |i| &s.mentions[i as usize])
    }

    /// Marks in `c` the keys the files `wanted` says mention: per file its
    /// range when `MENTIONS` is sorted by file (IN-18a), else by a scan.
    fn mark_mentions(&self, c: &mut [bool], wanted: impl Fn(u32) -> bool) {
        let m = &self.s.mentions;
        let mut mark = |x: &MentionRow| {
            if let Some(v) = c.get_mut(x.symbol as usize) {
                *v = true;
            }
        };
        if !m.windows(2).all(|w| w[0].file <= w[1].file) {
            m.iter().filter(|x| wanted(x.file)).for_each(mark);
            return;
        }
        let n = self.s.files.len() as u32;
        for f in 0..n {
            if wanted(f) {
                let lo = m.partition_point(|x| x.file < f);
                let hi = m.partition_point(|x| x.file <= f);
                m[lo..hi].iter().for_each(&mut mark);
            }
        }
        // Rows of no file (a damaged state).
        let tail = m.partition_point(|x| x.file < n);
        m[tail..].iter().filter(|x| wanted(x.file)).for_each(mark);
    }

    fn chunk(&self, c: u32) -> Option<&'s ChunkRow> {
        self.s.chunks.get(c as usize)
    }

    fn pos(&self, row: u32) -> Option<Pos<'s>> {
        let x = self.s.sections.get(row as usize)?;
        let c = self.chunk(x.place_chunk)?;
        if x.sh_flags & SHF_ALLOC != 0 {
            return Some(Pos::Alloc(c.addr.wrapping_add(x.place_offset)));
        }
        let o = self.s.outsecs.get(c.out as usize);
        let name = o.map_or_else(|| self.s.str(c.name), |o| self.s.str(o.name));
        let ordinal = o.map_or(c.ordinal, |o| o.ordinal);
        let start = self.out_start.get(c.out as usize).copied().unwrap_or(c.offset);
        Some(Pos::NonAlloc(
            name,
            ordinal,
            c.offset.wrapping_sub(start).wrapping_add(x.place_offset),
        ))
    }

    fn addr(&self, row: u32) -> Option<u64> {
        match self.pos(row)? {
            Pos::Alloc(a) => Some(a),
            Pos::NonAlloc(..) => None,
        }
    }

    fn placed(&self, row: u32) -> bool {
        self.s.sections.get(row as usize).is_some_and(|x| x.place_chunk != NONE)
    }

    fn chunk_key(&self, c: &ChunkRow) -> (u8, &'s [u8], u32) {
        (c.tag, self.s.str(c.name), c.ordinal)
    }

    fn chunk_by_name(&self, name: &[u8]) -> Option<&'s ChunkRow> {
        self.s.chunks.iter().find(|c| self.s.str(c.name) == name)
    }

    /// The output address of a linker-defined key (`DEFINED`).
    fn defined_value(&self, symbol: u32) -> Option<u64> {
        let d = &self.s.defined[*self.defined_at.get(&symbol)?];
        if d.kind == 0 {
            return Some(d.value);
        }
        Some(self.chunk(d.chunk)?.addr.wrapping_add(d.value))
    }

    /// A common's allocation (`SYM_PLACE`): address and size.
    fn common_place(&self, symbol: u32) -> Option<(u64, u64)> {
        let p = &self.s.sym_place[*self.common_at.get(&symbol)?];
        Some((self.chunk(p.chunk)?.addr.wrapping_add(p.offset), p.size))
    }

    /// A COMDAT group's keeper (`RESOLVED` kind 6, sorted by signature):
    /// `None` when the link has no such group, else the keeper's file.
    fn keeper(&self, sig: &[u8]) -> Option<u32> {
        let r = &self.s.resolved;
        let &(lo, hi, sorted) = self.comdat.get_or_init(|| {
            let lo = r.partition_point(|x| x.kind < def_kind::COMDAT);
            let hi = lo + r[lo..].partition_point(|x| x.kind == def_kind::COMDAT);
            let block = &r[lo..hi];
            let sorted = r.windows(2).all(|w| w[0].kind <= w[1].kind)
                && block.windows(2).all(|w| self.s.str(w[0].name) < self.s.str(w[1].name));
            (lo, hi, sorted)
        });
        let block = &r[lo..hi];
        if sorted {
            let at = block.binary_search_by(|x| self.s.str(x.name).cmp(sig)).ok()?;
            Some(block[at].file)
        } else {
            r.iter()
                .find(|x| x.kind == def_kind::COMDAT && self.s.str(x.name) == sig)
                .map(|x| x.file)
        }
    }
}

fn is_eh(s: &State, x: &SectionRow) -> bool {
    s.str(x.name) == b".eh_frame"
}

/// The state's TLS variant: AArch64's I (a 16-byte TCB), x86's II
/// (arch.md §3.8, `mortar_arch::TlsAbi::tp_rel`).
fn tp_offset(s: &State, addr: u64) -> i64 {
    let l = &s.layout;
    let align = 1u64 << l.tls_align;
    let up = |v: u64| v.div_ceil(align).wrapping_mul(align);
    let off = addr.wrapping_sub(l.tls_start);
    if s.meta.machine == EM_AARCH64 {
        off.wrapping_add(up(16)) as i64
    } else {
        off.wrapping_sub(up(l.tls_size)) as i64
    }
}

/// A slot's key: a global by identity, a local by (new file row,
/// ordinal, name).
type SlotKey<'s> = (u8, Option<KeyId<'s>>, Option<u32>, u32);

fn slot_list<'s>(s: &'s State, file: &dyn Fn(u32) -> Option<u32>) -> Vec<SlotKey<'s>> {
    s.slots
        .iter()
        .map(|x| match s.symbols.get(x.symbol as usize) {
            Some(y) if y.kind == 2 => {
                (x.kind, Some((2, s.str(y.name), &[][..])), file(y.file), y.ordinal)
            }
            _ => (x.kind, key_id(s, x.symbol), None, 0),
        })
        .collect()
}

fn patch_view<'s>(s: &Side<'s>) -> Vec<(u8, u64, Option<KeyId<'s>>, u64, u64)> {
    s.s.patches
        .iter()
        .map(|p| {
            (
                p.kind,
                p.offset,
                key_id(s.s, p.symbol),
                p.value,
                s.chunk(p.chunk).map_or(0, |c| c.addr),
            )
        })
        .collect()
}

struct Set {
    units: Vec<u32>,
    drop: u32,
}

impl Set {
    fn add(&mut self, row: u32, r: u32) {
        if let Some(u) = self.units.get_mut(row as usize) {
            *u |= r & !self.drop;
        }
    }
}

/// Nothing changed between the two links (IN-25): the same options and
/// target, every input path the same content, every live file the same
/// in the same order, every search answered the same. A link is a
/// function of those (the output is deterministic), so the delta is
/// empty and nothing is refilled; [`compute`] then answers without
/// resolving anything.
#[must_use]
pub fn nothing_changed(old: &State, new: &State, changes: &Changes) -> bool {
    use crate::detect::PathState;
    let same_path =
        |p: &(PathState, Vec<u8>)| matches!(p.0, PathState::Clean | PathState::HashedSame);
    let same_file =
        |f: &(Option<u32>, FileState)| matches!(f.1, FileState::Unchanged | FileState::Dead);
    let mut last = None;
    let ordered = changes.old_files.iter().all(|&(n, state)| {
        if state != FileState::Unchanged {
            return true;
        }
        let ok = n.is_some() && last <= n;
        last = n;
        ok
    });
    old.plan_hash == new.plan_hash
        && (old.meta.machine, old.meta.class) == (new.meta.machine, new.meta.class)
        && old.files.len() == new.files.len()
        && changes.paths.iter().all(same_path)
        && changes.new_files.iter().all(same_file)
        && changes.old_files.iter().all(same_file)
        && changes.stat_missed.is_empty()
        && changes.probes.is_empty()
        && changes.script_changed.is_none()
        && ordered
}

/// The delta from `old` to `new` and the refill set (IN-25..IN-29).
#[allow(clippy::too_many_lines, clippy::many_single_char_names, clippy::similar_names)] // IN-27's rows, in its order
#[must_use]
pub fn compute(
    old: &State,
    new: &State,
    changes: &Changes,
    extra: &Extra,
    drop: Drop<'_>,
) -> Delta {
    let mut d = Delta::default();
    if nothing_changed(old, new, changes) {
        d.alloc_bytes =
            new.chunks.iter().filter(|c| c.sh_flags & SHF_ALLOC != 0).map(|c| c.size).sum();
        return d;
    }
    let (a, b) = (Side::new(old), Side::new(new));
    let nfile = |row: u32| changes.new_files.get(row as usize).map_or(FileState::Dead, |f| f.1);
    let unchanged_new = |row: u32| nfile(row) == FileState::Unchanged;
    let ofile = |row: u32| changes.old_files.get(row as usize).map_or(FileState::Dead, |f| f.1);
    let old_to_new_file = |row: u32| changes.old_files.get(row as usize).and_then(|f| f.0);
    let fallback = |d: &mut Delta, reason: IncrReason| {
        let name = reason.name();
        if !drop.fallbacks.contains(&name) && !d.fallbacks.iter().any(|f| f.reason.name() == name) {
            d.fallbacks.push(Fallback { reason });
        }
    };
    // ---- sections by identity -------------------------------------------
    // Within a file pair: by position when both list the same identities
    // in the same order (an unchanged file always does), else by a map of
    // that file's sections.
    let mut sec_o2n: Vec<u32> = vec![NONE; old.sections.len()];
    let mut sec_n2o: Vec<u32> = vec![NONE; new.sections.len()];
    for of in 0..old.files.len() as u32 {
        let Some(nf) = old_to_new_file(of) else { continue };
        let os: Vec<u32> = a.by_file.get(of).collect();
        let ns: Vec<u32> = b.by_file.get(nf).collect();
        let id_o = |o: u32| sec_id(old, &old.sections[o as usize]);
        let id_n = |n: u32| sec_id(new, &new.sections[n as usize]);
        if os.len() == ns.len() && os.iter().zip(&ns).all(|(&o, &n)| id_o(o) == id_n(n)) {
            for (&o, &n) in os.iter().zip(&ns) {
                sec_o2n[o as usize] = n;
                sec_n2o[n as usize] = o;
            }
        } else {
            let by_id: HashMap<SecId<'_>, u32> = ns.iter().map(|&n| (id_n(n), n)).collect();
            for &o in &os {
                if let Some(&n) = by_id.get(&id_o(o)) {
                    sec_o2n[o as usize] = n;
                    sec_n2o[n as usize] = o;
                }
            }
        }
    }
    // ---- fall-backs on the shape of the output ---------------------------
    let outs = |s: &'_ State| -> Vec<(Vec<u8>, u32)> {
        let mut v: Vec<_> = s.outsecs.iter().map(|o| (s.str(o.name).to_vec(), o.ordinal)).collect();
        v.sort();
        v
    };
    if outs(old) != outs(new) || old.segments.len() != new.segments.len() {
        let (o, n) = (outs(old), outs(new));
        let diff: Vec<String> = n
            .iter()
            .filter(|x| !o.contains(x))
            .map(|x| format!("+{}", String::from_utf8_lossy(&x.0)))
            .chain(
                o.iter()
                    .filter(|x| !n.contains(x))
                    .map(|x| format!("-{}", String::from_utf8_lossy(&x.0))),
            )
            .collect();
        let detail = if diff.is_empty() {
            format!("{} segments, now {}", old.segments.len(), new.segments.len())
        } else {
            diff.join(" ")
        };
        fallback(&mut d, IncrReason::NewOutputSection(detail));
    }
    let prop = |s: &Side<'_>| s.chunk_by_name(b".note.gnu.property").map(|c| c.size);
    if prop(&a) != prop(&b) {
        fallback(
            &mut d,
            IncrReason::PropertyChanged(format!("{:?} -> {:?} bytes", prop(&a), prop(&b))),
        );
    }
    if let Some(p) = &changes.script_changed {
        fallback(
            &mut d,
            IncrReason::OptionsChanged(format!(
                "{} changed and is not an object or archive",
                String::from_utf8_lossy(p)
            )),
        );
    }
    // inputs-reordered: two unchanged inputs' relative order changed.
    let mut last = None;
    for (o, &(n, state)) in changes.old_files.iter().enumerate() {
        if state != FileState::Unchanged {
            continue;
        }
        let Some(n) = n else { continue };
        if last.is_some_and(|(_, l)| l > n) {
            let name =
                |s: &State, r: usize| String::from_utf8_lossy(s.str(s.files[r].path)).into_owned();
            fallback(
                &mut d,
                IncrReason::InputsReordered(format!(
                    "{} moved before {}",
                    name(old, o),
                    name(new, n as usize)
                )),
            );
            break;
        }
        last = Some((o, n));
    }
    // tls-layout-changed: a TLS section live in both whose TP offset moved.
    for (o, x) in old.sections.iter().enumerate() {
        let n = sec_o2n[o];
        if n == NONE || x.sh_flags & SHF_TLS == 0 {
            continue;
        }
        if let (Some(p), Some(q)) = (a.addr(o as u32), b.addr(n)) {
            let (tp_a, tp_b) = (tp_offset(old, p), tp_offset(new, q));
            if tp_a != tp_b {
                d.symbols.tls += 1;
                let name = String::from_utf8_lossy(old.str(x.name)).into_owned();
                let file =
                    String::from_utf8_lossy(old.str(old.files[x.file as usize].path)).into_owned();
                fallback(
                    &mut d,
                    IncrReason::TlsLayoutChanged(format!("{file}:{name} {tp_a:#x} -> {tp_b:#x}")),
                );
            }
        }
    }
    // ---- moved, liveness, merged fragments, .eh_frame ------------------
    let nsec = new.sections.len();
    let mut moved = vec![false; nsec];
    let mut flipped = vec![false; nsec];
    for (n, &o) in sec_n2o.iter().enumerate() {
        if o == NONE {
            continue;
        }
        match (a.pos(o), b.pos(n as u32)) {
            (Some(p), Some(q)) => moved[n] = p != q,
            (None, None) => {}
            _ => flipped[n] = true,
        }
    }
    // Merged sections: a fragment whose place differs, or that is in one
    // link only. A FRAGS offset is relative to its output section's merged
    // chunk, so the place is that chunk's address (allocated) or its
    // offset in the output section (non-allocated) plus the offset: a
    // merged chunk that moves moves every fragment in it (the oracle's
    // `os` scenario). A hash with several offsets (ordinal rows) compares
    // as the list of its places; an output section with several merged
    // chunks has no one base, and its fragments count as moved.
    //
    // Output sections are named by one number on both sides: per
    // (name, ordinal), a new `OUTSECS` row, or past them an old row the
    // new link has not.
    let mut out_ids: HashMap<(&[u8], u32), u32> = HashMap::default();
    let new_out: Vec<u32> = new
        .outsecs
        .iter()
        .enumerate()
        .map(|(i, o)| *out_ids.entry((new.str(o.name), o.ordinal)).or_insert(i as u32))
        .collect();
    let mut next = new.outsecs.len() as u32;
    let old_out: Vec<u32> = old
        .outsecs
        .iter()
        .map(|o| {
            *out_ids.entry((old.str(o.name), o.ordinal)).or_insert_with(|| {
                next += 1;
                next - 1
            })
        })
        .collect();
    // Per side: (output section number, hash, place) in FRAGS order,
    // sorted by the first two (stably: a hash's rows stay in ordinal
    // order). The place is `None` when the output section has no one
    // merged chunk.
    let frag_list = |side: &Side<'_>, ids: &[u32]| -> Vec<Frag> {
        let s = side.s;
        // Per OUTSECS row: no merged chunk, one (its base), several.
        let mut base: Vec<Option<Option<u64>>> = vec![None; s.outsecs.len()];
        for c in &s.chunks {
            if c.tag == TAG_MERGED
                && let Some(e) = base.get_mut(c.out as usize)
            {
                let at = if c.sh_flags & SHF_ALLOC != 0 {
                    c.addr
                } else {
                    c.offset.wrapping_sub(
                        side.out_start.get(c.out as usize).copied().unwrap_or(c.offset),
                    )
                };
                *e = Some(if e.is_some() { None } else { Some(at) });
            }
        }
        let mut v: Vec<Frag> = s
            .frags
            .iter()
            .filter_map(|f| {
                let &id = ids.get(f.out as usize)?;
                let at = base.get(f.out as usize).copied().flatten().flatten();
                Some((id, f.hash, at.map(|b| b.wrapping_add(f.offset))))
            })
            .collect();
        if !v.windows(2).all(|w| (w[0].0, w[0].1) <= (w[1].0, w[1].1)) {
            v.sort_by_key(|x| (x.0, x.1));
        }
        v
    };
    let (fa, fb) = (frag_list(&a, &old_out), frag_list(&b, &new_out));
    // The rows of one (output section, hash).
    let group = |v: &[Frag], k: (u32, Hash16)| -> std::ops::Range<usize> {
        let lo = v.partition_point(|x| (x.0, x.1) < k);
        lo..lo + v[lo..].partition_point(|x| (x.0, x.1) == k)
    };
    // The same: in both links, every place known, the same places in the
    // same order.
    let same = |x: &[Frag], y: &[Frag]| {
        !x.is_empty()
            && x.len() == y.len()
            && x.iter().zip(y).all(|(p, q)| p.2.is_some() && p.2 == q.2)
    };
    // One walk over both lists: per hash of the new link, the same or
    // not; any hash not the same changes the merged chunks.
    let mut frags_changed = false;
    let mut same_of: HashMap<(u32, Hash16), bool> = HashMap::default();
    same_of.reserve(fb.len());
    let (mut i, mut j) = (0, 0);
    while i < fa.len() || j < fb.len() {
        let k = match (fa.get(i), fb.get(j)) {
            (Some(p), Some(q)) => (p.0, p.1).min((q.0, q.1)),
            (Some(p), None) => (p.0, p.1),
            (None, Some(q)) => (q.0, q.1),
            (None, None) => break,
        };
        let (x, y) = (group(&fa[i..], k), group(&fb[j..], k));
        let s = same(&fa[i..][x.clone()], &fb[j..][y.clone()]);
        frags_changed |= !s;
        if !y.is_empty() {
            same_of.insert(k, s);
        }
        i += x.len();
        j += y.len();
    }
    // A section's fragment whose hash is not the same, or in neither
    // link's FRAGS, moved.
    let mut merged_moved = vec![false; nsec];
    for &(sec, out, h) in &extra.frags {
        let Some(&id) = new_out.get(out as usize) else { continue };
        if !same_of.get(&(id, h)).copied().unwrap_or(false) {
            if let Some(m) = merged_moved.get_mut(sec as usize) {
                *m = true;
            }
            frags_changed = true;
        }
    }
    std::mem::drop((fa, fb));
    // .eh_frame (IN-34a): the records in output order. S-47 packs them
    // back to back, each record's size a function of its input bytes. A
    // record is the same on both sides when its file is unchanged and it
    // is the same piece of the same section.
    let (eh_a, eh_b) = (a.chunk_by_name(b".eh_frame"), b.chunk_by_name(b".eh_frame"));
    let eh_chunk_moved = eh_a.map(|c| c.addr) != eh_b.map(|c| c.addr);
    let (ro, rn) = (&old.eh_records, &new.eh_records);
    let same_record = |x: &EhRow, y: &EhRow| {
        let file = new.sections.get(y.section as usize).map_or(NONE, |s| s.file);
        sec_o2n.get(x.section as usize) == Some(&y.section)
            && x.piece == y.piece
            && unchanged_new(file)
    };
    // The prefix: same records from the start are at the same offsets, and
    // an FDE's CIE precedes it (S-46), so it is in the prefix too.
    let mut k = 0;
    if !eh_chunk_moved {
        while k < ro.len() && k < rn.len() && same_record(&ro[k], &rn[k]) {
            k += 1;
        }
    }
    // The suffix: when the chunk keeps its address and size, same records
    // from the end are at the same offsets too, whatever changed between
    // (a same-size edit). What an FDE of the suffix points to must be the
    // same CIE at the same place on both sides:
    // - its own CIE, kept in the new link, at or after the suffix's start:
    //   the suffixes are equal, so the old link kept it at the same place;
    // - its own CIE not kept (S-44: an equal CIE with a lesser
    //   `(FileId, sh_index, record)`, earlier in S-46's order), when no
    //   CIE is kept between the prefix and the suffix in the new link.
    //   The new link's kept copy is then in the prefix or the suffix. The
    //   old link's is the same record: one in the prefix is the least on
    //   both sides; one in the old middle would make the new kept copy,
    //   being later, a CIE the old link removed and the new one keeps,
    //   which unequal suffixes or a CIE in the new middle would show.
    // Anything else (its own CIE kept before the suffix, a piece the new
    // link's split does not know) starts the suffix after that FDE.
    let mut tail = 0;
    if !eh_chunk_moved && eh_a.map(|c| c.size) == eh_b.map(|c| c.size) {
        while tail < ro.len() - k
            && tail < rn.len() - k
            && same_record(&ro[ro.len() - 1 - tail], &rn[rn.len() - 1 - tail])
        {
            tail += 1;
        }
        let first = rn.len() - tail;
        // A record by (section, piece): the records are in S-46's order,
        // sorted by both, so a search; a map when they are not.
        let sorted = rn.windows(2).all(|w| (w[0].section, w[0].piece) < (w[1].section, w[1].piece));
        let at_map: HashMap<(u32, u32), usize> = if sorted {
            HashMap::default()
        } else {
            rn.iter().enumerate().map(|(i, y)| ((y.section, y.piece), i)).collect()
        };
        let at = |key: (u32, u32)| -> Option<usize> {
            if sorted {
                rn.binary_search_by_key(&key, |y| (y.section, y.piece)).ok()
            } else {
                at_map.get(&key).copied()
            }
        };
        let suffix_start = |shared_ok: bool| {
            let mut start = first;
            for (i, y) in rn.iter().enumerate().skip(first) {
                let ok = match extra.eh_cie(y.section, y.piece) {
                    Some(EhCie::Cie) => true,
                    Some(EhCie::Fde(cie)) => match at((y.section, cie)) {
                        Some(j) => j >= start,
                        None => shared_ok,
                    },
                    None => false,
                };
                if !ok {
                    start = i + 1;
                }
            }
            start
        };
        let middle_has_cie = |start: usize| {
            // A piece the split does not know counts as a CIE.
            rn[k..start]
                .iter()
                .any(|y| !matches!(extra.eh_cie(y.section, y.piece), Some(EhCie::Fde(_))))
        };
        let mut start = suffix_start(true);
        if middle_has_cie(start) {
            start = suffix_start(false);
        }
        tail = rn.len() - start;
    }
    let eh_changed = k + tail < ro.len().max(rn.len());
    let mut eh_moved = vec![false; nsec];
    for y in &rn[k..rn.len() - tail] {
        if let Some(m) = eh_moved.get_mut(y.section as usize) {
            *m = true;
        }
    }
    // Present units: placed, in `.eh_frame`'s records, or merged.
    let mut present = vec![false; nsec];
    for (i, x) in new.sections.iter().enumerate() {
        present[i] = x.place_chunk != NONE;
    }
    for y in &new.eh_records {
        if let Some(p) = present.get_mut(y.section as usize) {
            *p = true;
        }
    }
    for (sec, _, _) in &extra.frags {
        let alloc = new.sections.get(*sec as usize).is_some_and(|x| x.sh_flags & SHF_ALLOC != 0);
        if alloc && let Some(p) = present.get_mut(*sec as usize) {
            *p = true;
        }
    }
    // Per new file: its present eh units and its placed non-alloc units.
    let present_of = |eh: bool| {
        let present = &present;
        Csr::new(
            new.files.len(),
            new.sections
                .iter()
                .enumerate()
                .filter(move |&(i, x)| {
                    present[i]
                        && if eh {
                            is_eh(new, x)
                        } else {
                            !is_eh(new, x) && x.sh_flags & SHF_ALLOC == 0 && x.place_chunk != NONE
                        }
                })
                .map(|(i, x)| (x.file, i as u32)),
        )
    };
    let (eh_of, nonalloc_of) = (present_of(true), present_of(false));
    // The index backwards: target -> units.
    let index_of = |kind: u8, n: usize| {
        Csr::new(n, new.index.iter().filter(move |x| x.kind == kind).map(|x| (x.target, x.unit)))
    };
    let by_global = index_of(index_kind::GLOBAL, new.symbols.len());
    let by_section = index_of(index_kind::SECTION, nsec);
    let slot_users: Vec<u32> = new
        .index
        .iter()
        .filter(|x| matches!(x.kind, index_kind::SLOT | index_kind::LOCAL_SLOT))
        .map(|x| x.unit)
        .collect();
    // Mentions by key (new SYMBOLS row), for the non-alloc referrers.
    let files_mentioning =
        Csr::new(new.symbols.len(), new.mentions.iter().map(|m| (m.symbol, m.file)));
    let mut set = Set { units: vec![0; nsec], drop: drop.reasons };
    let referrers_of_key = |set: &mut Set, key: u32, r: u32| {
        for u in by_global.get(key) {
            set.add(u, r);
        }
        for f in files_mentioning.get(key) {
            for u in nonalloc_of.get(f) {
                set.add(u, r);
            }
        }
    };
    // Row 1: changed, appeared, extracted files.
    for (i, x) in new.sections.iter().enumerate() {
        if present[i] && nfile(x.file).refills_all() {
            set.add(i as u32, reason::FILE);
        }
    }
    // ---- globals (IN-26) -----------------------------------------------
    let file_id = |side: bool, f: u32| -> Option<u32> {
        // As a NEW files row (the old side translated).
        if f == NONE {
            return None;
        }
        if side { Some(f) } else { old_to_new_file(f) }
    };
    let sec_new = |side: bool, sec: u32| -> u32 {
        if sec == NONE || side { sec } else { sec_o2n.get(sec as usize).copied().unwrap_or(NONE) }
    };
    let section_moved = |n: u32| -> bool {
        n == NONE
            || moved.get(n as usize).copied().unwrap_or(true)
            || flipped.get(n as usize).copied().unwrap_or(true)
            || merged_moved.get(n as usize).copied().unwrap_or(false)
            || eh_moved.get(n as usize).copied().unwrap_or(false)
    };
    // The keys that can differ. A key that only unchanged files mention
    // (in both links: they are the same files with the same bytes) is
    // resolved from the same mentions in the same order (IN-26; another
    // order is `inputs-reordered`), so it has the same definer and the
    // same kind, size and attributes, and it is in both links. Its value
    // differs only when its section moved (row 6, the merged fragments,
    // the `.eh_frame` records, a liveness flip) or when the layout
    // gives it (internal, common, shared, absolute: compared always).
    // So the candidates are:
    // - the keys a new file other than unchanged mentions, and those an
    //   old one other than unchanged mentions (gone, unextracted,
    //   changed): per file, its `MENTIONS` range;
    // - the keys defined in a section that moved, flipped or whose
    //   fragments or records moved, on either side;
    // - the layout's own kinds, on either side;
    // - the keys of an old section that flipped (the flip below asks for
    //   their new rows).
    let mut cand_new = vec![false; new.symbols.len()];
    let mut cand_old = vec![false; old.symbols.len()];
    let mark = |c: &mut Vec<bool>, sym: u32| {
        if let Some(x) = c.get_mut(sym as usize) {
            *x = true;
        }
    };
    b.mark_mentions(&mut cand_new, |f| !unchanged_new(f));
    a.mark_mentions(&mut cand_old, |f| ofile(f) != FileState::Unchanged);
    let layout_kind = |k: u8| {
        matches!(k, def_kind::ABSOLUTE | def_kind::COMMON | def_kind::INTERNAL | def_kind::SHARED)
    };
    for r in new.resolved.iter().filter(|r| r.kind <= def_kind::SHARED) {
        if layout_kind(r.kind) || (r.kind == def_kind::SECTION && section_moved(r.section)) {
            mark(&mut cand_new, r.symbol);
        }
    }
    for r in old.resolved.iter().filter(|r| r.kind <= def_kind::SHARED) {
        if layout_kind(r.kind)
            || (r.kind == def_kind::SECTION && section_moved(sec_new(false, r.section)))
        {
            mark(&mut cand_old, r.symbol);
        }
    }
    for (n, &f) in flipped.iter().enumerate() {
        if f {
            for m in a.globals_in(sec_n2o[n]) {
                mark(&mut cand_old, m.symbol);
            }
        }
    }
    // Pair the candidates across the links by identity: a map of the
    // candidates only, then one pass over each side's other keys to find
    // a candidate's row there (the state has no index by name).
    let mut pair: HashMap<KeyId<'_>, (u32, u32)> = HashMap::default();
    let mut in_pair_new = vec![false; new.resolved.len()];
    for (i, r) in new.resolved.iter().enumerate() {
        if r.kind <= def_kind::SHARED
            && cand_new.get(r.symbol as usize).copied().unwrap_or(false)
            && let Some(k) = key_id(new, r.symbol)
        {
            pair.entry(k).or_insert((NONE, NONE)).0 = i as u32;
            in_pair_new[i] = true;
        }
    }
    for (j, r) in old.resolved.iter().enumerate() {
        if r.kind <= def_kind::SHARED
            && cand_old.get(r.symbol as usize).copied().unwrap_or(false)
            && let Some(k) = key_id(old, r.symbol)
        {
            pair.entry(k).or_insert((NONE, NONE)).1 = j as u32;
        }
    }
    if !pair.is_empty() {
        for (j, r) in old.resolved.iter().enumerate() {
            if r.kind <= def_kind::SHARED
                && !cand_old.get(r.symbol as usize).copied().unwrap_or(false)
                && let Some(k) = key_id(old, r.symbol)
                && let Some(e) = pair.get_mut(&k)
            {
                e.1 = j as u32;
            }
        }
        for (i, r) in new.resolved.iter().enumerate() {
            if r.kind <= def_kind::SHARED
                && !in_pair_new[i]
                && let Some(k) = key_id(new, r.symbol)
                && let Some(e) = pair.get_mut(&k)
            {
                e.0 = i as u32;
                in_pair_new[i] = true;
            }
        }
    }
    let key_name = |k: &KeyId<'_>| {
        let mut n = String::from_utf8_lossy(k.1).into_owned();
        if k.0 == 1 {
            n.push('@');
            n.push_str(&String::from_utf8_lossy(k.2));
        }
        n
    };
    // In the report's order: the new link's keys in `RESOLVED` order (by
    // their `SYMBOLS` row), then the keys gone in the old one's.
    for (i, n) in new.resolved.iter().enumerate() {
        if !in_pair_new[i] {
            continue;
        }
        let Some(k) = key_id(new, n.symbol) else { continue };
        let nsym = n.symbol;
        let n = *n;
        let Some(&(_, j)) = pair.get(&k) else { continue };
        if j == NONE {
            d.symbols.new += 1;
            d.symbol_lines.push(("new", key_name(&k)));
            continue;
        }
        let o = old.resolved[j as usize];
        let osym = o.symbol;
        let defined = |r: &ResolvedRow| r.kind != def_kind::UNDEFINED;
        // Attributes (IN-26 v1): type, binding, st_other, or became
        // absolute or common, for a key an unchanged file mentions.
        let special = |r: &ResolvedRow| matches!(r.kind, def_kind::ABSOLUTE | def_kind::COMMON);
        let attrs =
            defined(&o) && defined(&n) && (o.typ, o.bind, o.other) != (n.typ, n.bind, n.other);
        let became = o.kind != n.kind && (special(&o) || special(&n));
        if attrs || became {
            d.symbols.attrs += 1;
            d.symbol_lines.push(("attrs", key_name(&k)));
            // An unchanged file's mentions are the same in both links.
            if files_mentioning.get(nsym).any(&unchanged_new) {
                fallback(&mut d, IncrReason::SymbolKindChanged(key_name(&k)));
            }
        }
        // The definition (IN-27 row 3).
        let definer = file_id(false, o.file) != file_id(true, n.file);
        let mut what = None;
        if o.kind != n.kind {
            d.symbols.kind += 1;
            what = Some("kind");
        } else if definer {
            d.symbols.definer += 1;
            what = Some("definer");
        } else if o.size != n.size {
            d.symbols.size += 1;
            what = Some("size");
        } else {
            let value = match n.kind {
                def_kind::SECTION => {
                    sec_new(false, o.section) != n.section
                        || o.value != n.value
                        || section_moved(n.section)
                }
                def_kind::ABSOLUTE => o.value != n.value,
                def_kind::COMMON => {
                    let changed = a.common_place(osym) != b.common_place(nsym);
                    if changed {
                        d.symbols.commons += 1;
                    }
                    changed
                }
                def_kind::INTERNAL => {
                    let (x, y) = (a.defined_value(osym), b.defined_value(nsym));
                    x.is_none() || x != y
                }
                def_kind::SHARED => true,
                _ => false,
            };
            if value {
                d.symbols.value += 1;
                what = Some("value");
            }
        }
        if let Some(w) = what {
            d.symbol_lines.push((w, key_name(&k)));
            referrers_of_key(&mut set, nsym, reason::GLOBAL);
        }
    }
    for r in old.resolved.iter().filter(|r| r.kind <= def_kind::SHARED) {
        if !cand_old.get(r.symbol as usize).copied().unwrap_or(false) {
            continue;
        }
        let Some(k) = key_id(old, r.symbol) else { continue };
        if pair.get(&k).is_some_and(|e| e.0 == NONE) {
            d.symbols.gone += 1;
            d.symbol_lines.push(("gone", key_name(&k)));
        }
    }
    // COMDAT keepers (IN-27 row 4). A group only unchanged files have
    // (in both links) keeps its keeper: the first of them in the same
    // order. So the candidates are the groups of the sections of the
    // other files, on either side, each looked up in both.
    let mut sigs: HashSet<&[u8]> = HashSet::default();
    for f in 0..new.files.len() as u32 {
        if !unchanged_new(f) {
            for x in b.by_file.get(f).map(|i| &new.sections[i as usize]) {
                if x.anchor_kind == 1 {
                    sigs.insert(new.str(x.anchor));
                }
            }
        }
    }
    for f in 0..old.files.len() as u32 {
        if ofile(f) != FileState::Unchanged {
            for x in a.by_file.get(f).map(|i| &old.sections[i as usize]) {
                if x.anchor_kind == 1 {
                    sigs.insert(old.str(x.anchor));
                }
            }
        }
    }
    // Changed: in one link only, or kept by another file.
    let changed_groups: HashSet<&[u8]> = sigs
        .into_iter()
        .filter(|sig| {
            a.keeper(sig).map(|f| file_id(false, f)) != b.keeper(sig).map(|f| file_id(true, f))
        })
        .collect();
    d.symbols.comdat = changed_groups.len();
    // Liveness flips of unchanged files' placed sections (rows 4 and 9).
    for n in 0..nsec {
        let x = &new.sections[n];
        if !flipped[n] || !unchanged_new(x.file) {
            continue;
        }
        let grouped = x.anchor_kind == 1 && changed_groups.contains(new.str(x.anchor));
        let (row, file_row) = if grouped {
            (reason::COMDAT, reason::COMDAT_FILE)
        } else {
            (reason::LIVE, reason::LIVE_FILE)
        };
        if b.placed(n as u32) {
            d.symbols.live += 1;
            set.add(n as u32, row);
        } else {
            d.symbols.dead += 1;
        }
        for u in eh_of.get(x.file).chain(nonalloc_of.get(x.file)) {
            set.add(u, file_row);
        }
        for m in b.globals_in(n as u32) {
            referrers_of_key(&mut set, m.symbol, row);
        }
        let o = sec_n2o[n];
        for m in a.globals_in(o) {
            if let Some(k) = key_id(old, m.symbol)
                && let Some(&(i, _)) = pair.get(&k)
                && i != NONE
            {
                referrers_of_key(&mut set, new.resolved[i as usize].symbol, row);
            }
        }
    }
    // A COMDAT group whose keeper moved to or from a changed file: the
    // other copy's file is unchanged and its copy flipped, handled above;
    // the new keeper's members are refilled (row 1 or the flip).
    // Moved sections of unchanged files (row 6) and merged ones.
    for n in 0..nsec {
        let x = &new.sections[n];
        if !unchanged_new(x.file) {
            continue;
        }
        if moved[n] {
            set.add(n as u32, reason::MOVED);
            for u in by_section.get(n as u32) {
                set.add(u, reason::MOVED_REF);
            }
            for u in nonalloc_of.get(x.file) {
                set.add(u, reason::MOVED_FILE);
            }
        }
        if merged_moved[n] {
            if present[n] {
                set.add(n as u32, reason::FRAG);
            }
            for u in by_section.get(n as u32) {
                set.add(u, reason::FRAG);
            }
            for u in nonalloc_of.get(x.file) {
                set.add(u, reason::FRAG);
            }
        }
        if eh_moved[n] {
            set.add(n as u32, reason::EH);
        }
    }
    // References into `.eh_frame` (row 10, S-50): a reference's value is
    // its piece's place, the same when every piece of the target section
    // is kept and none is between the prefix and the suffix. A removed
    // piece resolves to the next kept record (S-50 case 3), which may be
    // between them.
    if eh_changed {
        let mut kept = vec![0u32; nsec];
        for y in rn {
            if let Some(c) = kept.get_mut(y.section as usize) {
                *c += 1;
            }
        }
        for x in new.index.iter().filter(|x| x.kind == index_kind::EH_FRAME) {
            let (u, t) = (x.unit, x.target);
            // An empty section: every reference is S-50 case 1, the
            // output section's start (the chunk did not move: a prefix).
            let empty = new.sections.get(t as usize).is_some_and(|x| x.size == 0);
            let count = kept.get(t as usize).copied().unwrap_or(0);
            let whole = count > 0 && extra.eh_pieces(t) == Some(count);
            let same = (empty && !eh_chunk_moved)
                || (whole && !eh_moved.get(t as usize).copied().unwrap_or(true));
            if !same {
                set.add(u, reason::EH_REF);
            }
        }
    }
    // Slot tables (rows 5 and 7).
    let slot_names: [&[u8]; 6] =
        [b".got", b".got.plt", b".plt", b".iplt", b".rela.plt", b".rela.iplt"];
    let slot_chunks_moved = slot_names.iter().any(|n| {
        a.chunk_by_name(n).map(|c| (c.addr, c.size)) != b.chunk_by_name(n).map(|c| (c.addr, c.size))
    });
    let slots_changed = slot_chunks_moved
        || slot_list(old, &|f| file_id(false, f)) != slot_list(new, &|f| file_id(true, f));
    if slots_changed {
        for &u in &slot_users {
            set.add(u, reason::SLOTS);
        }
        for x in new.slots.iter().filter(|x| x.kind == 2) {
            referrers_of_key(&mut set, x.symbol, reason::SLOTS);
        }
    }
    // Patches and thunks (row 8).
    let redirect_view = |s: &'_ State, side: bool| -> Vec<(u32, u32, u64)> {
        s.redirects.iter().map(|r| (sec_new(side, r.section), r.patch, r.offset)).collect()
    };
    let patches_changed =
        patch_view(&a) != patch_view(&b) || redirect_view(old, false) != redirect_view(new, true);
    if patches_changed {
        for r in &new.redirects {
            set.add(r.section, reason::PATCH);
        }
    }
    // link-order-moved: the sh_link target of a live SHF_LINK_ORDER
    // section moved (or is new).
    for x in new.index.iter().filter(|x| x.kind == index_kind::LINK_ORDER) {
        if !b.placed(x.unit) {
            continue;
        }
        let t = x.target as usize;
        let o = sec_n2o.get(t).copied().unwrap_or(NONE);
        let same = o != NONE && a.pos(o).is_some() && a.pos(o) == b.pos(x.target);
        if !same {
            let name = String::from_utf8_lossy(new.str(new.sections[t].name)).into_owned();
            fallback(&mut d, IncrReason::LinkOrderMoved(name));
        }
    }
    // ---- holes -----------------------------------------------------------
    for (o, x) in old.sections.iter().enumerate() {
        let Some(p) = a.pos(o as u32) else { continue };
        if x.size == 0 {
            continue;
        }
        let n = sec_o2n[o];
        if n != NONE && b.pos(n) == Some(p) && new.sections[n as usize].size == x.size {
            continue;
        }
        let r = match ofile(x.file) {
            FileState::Gone | FileState::Unextracted => reason::GONE,
            FileState::Unchanged => {
                if n == NONE || flipped[n as usize] {
                    let nx = new.sections.get(n as usize);
                    if nx.is_some_and(|nx| {
                        nx.anchor_kind == 1 && changed_groups.contains(new.str(nx.anchor))
                    }) {
                        reason::COMDAT
                    } else {
                        reason::LIVE
                    }
                } else {
                    reason::MOVED
                }
            }
            _ => reason::FILE,
        };
        let r = r & !drop.reasons;
        if r != 0 {
            d.holes.push(Hole { old: o as u32, new: n, reasons: r });
        }
    }
    // ---- units -----------------------------------------------------------
    for (i, &r) in set.units.iter().enumerate() {
        if r != 0 {
            d.units.push(Unit { section: i as u32, reasons: r });
            let x = &new.sections[i];
            if x.sh_flags & SHF_ALLOC != 0 {
                d.unit_bytes += x.size;
            }
        }
    }
    // ---- chunks ----------------------------------------------------------
    let anything = !d.units.is_empty()
        || !d.holes.is_empty()
        || !d.symbol_lines.is_empty()
        || changed_groups.len() + d.fallbacks.len() > 0
        || frags_changed
        || eh_changed
        || slots_changed
        || patches_changed;
    let old_chunks: HashMap<(u8, &[u8], u32), &ChunkRow> =
        old.chunks.iter().map(|c| (a.chunk_key(c), c)).collect();
    // Body chunks holding a unit or a hole.
    let mut touched: HashSet<u32> = HashSet::default();
    for u in &d.units {
        let x = &new.sections[u.section as usize];
        if x.place_chunk != NONE {
            touched.insert(x.place_chunk);
        }
    }
    let mut old_touched: HashSet<(u8, &[u8], u32)> = HashSet::default();
    for h in &d.holes {
        let x = &old.sections[h.old as usize];
        if let Some(c) = a.chunk(x.place_chunk) {
            old_touched.insert(a.chunk_key(c));
        }
    }
    for (i, c) in new.chunks.iter().enumerate() {
        if c.sh_flags & SHF_ALLOC != 0 {
            d.alloc_bytes += c.size;
        }
        if !anything {
            continue;
        }
        let key = b.chunk_key(c);
        let before = old_chunks.get(&key);
        let shifted =
            before.is_none_or(|o| (o.addr, o.offset, o.size) != (c.addr, c.offset, c.size));
        let r = match c.tag {
            0..=2 => reason::HEADERS,
            TAG_SYNTHETIC | 7 | 8 => reason::SYNTH,
            TAG_MERGED if frags_changed || shifted => reason::SYNTH,
            TAG_PATCHES if patches_changed || shifted => reason::PATCH,
            TAG_BODY if shifted || touched.contains(&(i as u32)) || old_touched.contains(&key) => {
                reason::GAPS
            }
            _ => 0,
        };
        let r = r & !drop.reasons;
        if r != 0 {
            d.chunks.push(ChunkLine { chunk: i as u32, reasons: r });
        }
    }
    // ---- policy (IN-16) ---------------------------------------------------
    let live_inputs = changes.new_files.iter().filter(|f| f.1 != FileState::Dead).count() as u64;
    let changed_inputs = changes.new_files.iter().filter(|f| f.1.refills_all()).count() as u64;
    let (num, den) = TOO_MUCH;
    if d.unit_bytes * den > d.alloc_bytes * num {
        d.policy = Some(format!("refilled-bytes={} alloc-bytes={}", d.unit_bytes, d.alloc_bytes));
    } else if changed_inputs * den > live_inputs * num {
        d.policy = Some(format!("changed-inputs={changed_inputs} live-inputs={live_inputs}"));
    }
    d
}

// ---- the report (`docs/cli.md`, `--incremental-report`) -------------------

/// The most lines of one kind the report lists (`docs/cli.md` §3.13,
/// `spec/incr.md` IN-10, WP-106): the first `REPORT_BOUND` of each kind,
/// in the report's order, then one `delta elided` line with the kind's
/// total, so that nothing is left out unsaid. The count lines (`delta
/// inputs`, `delta symbols`, `delta refill`) are not bounded. 10,000
/// keeps every oracle-lane report whole (the largest kind there, the
/// units of `ovx`, is 2,137) and caps a report at nine kinds of 10,000
/// lines (WP-094's g10x fall-back wrote 12.9 million).
pub const REPORT_BOUND: usize = 10_000;

/// The report's bounded line kinds.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Kind {
    Path,
    StatMissed,
    File,
    Probe,
    Symbol,
    Fallback,
    Unit,
    Hole,
    Chunk,
}

impl Kind {
    const fn name(self) -> &'static str {
        match self {
            Self::Path => "path",
            Self::StatMissed => "stat-missed",
            Self::File => "file",
            Self::Probe => "probe",
            Self::Symbol => "symbol",
            Self::Fallback => "fallback",
            Self::Unit => "unit",
            Self::Hole => "hole",
            Self::Chunk => "chunk",
        }
    }
}

/// The lines of the kind being listed, and every line elided so far.
#[derive(Debug, Default)]
struct Bounded {
    count: usize,
    elided: usize,
}

impl Bounded {
    /// Whether the next line of the kind being listed is written: one of
    /// its first `REPORT_BOUND` (it is counted either way).
    fn take(&mut self) -> bool {
        self.count += 1;
        self.count <= REPORT_BOUND
    }

    /// The end of a kind's lines: `delta elided <kind> listed=N total=N`
    /// when some were left out.
    fn close(&mut self, s: &mut String, kind: Kind) {
        use std::fmt::Write as _;
        if self.count > REPORT_BOUND {
            let _ = writeln!(
                s,
                "delta elided {} listed={REPORT_BOUND} total={}",
                kind.name(),
                self.count
            );
            self.elided += self.count - REPORT_BOUND;
        }
        self.count = 0;
    }
}

/// A field of a report line: bytes other than printable ASCII, space and
/// `%` are written `%XX`; an empty field is `-`.
#[must_use]
pub fn field(b: &[u8]) -> String {
    use std::fmt::Write as _;
    if b.is_empty() {
        return "-".to_owned();
    }
    let mut out = String::with_capacity(b.len());
    for &c in b {
        if c > b' ' && c < 0x7f && c != b'%' {
            out.push(char::from(c));
        } else {
            let _ = write!(out, "%{c:02X}");
        }
    }
    out
}

const TAG_NAMES: [&str; 9] = [
    "file-header",
    "program-headers",
    "section-headers",
    "body",
    "merged",
    "patches",
    "synthetic",
    "group",
    "relocs",
];

/// The report's `delta` lines (after the `incremental:` line), in the
/// order `docs/cli.md` §3.13 gives.
#[allow(clippy::too_many_lines, clippy::many_single_char_names)] // one block per line kind, in order
#[must_use]
pub fn report(old: &State, new: &State, changes: &Changes, d: &Delta, extra: &Extra) -> String {
    use std::fmt::Write as _;
    let mut s = String::new();
    let mut bound = Bounded::default();
    let n = changes.counts();
    let _ = writeln!(
        s,
        "delta inputs clean={} hashed-same={} changed={} added={} removed={} files-changed={} appeared={} extracted={} gone={} unextracted={}",
        n.clean,
        n.hashed_same,
        n.changed,
        n.added,
        n.removed,
        n.files_changed,
        n.appeared,
        n.extracted,
        n.gone,
        n.unextracted
    );
    for (state, path) in &changes.paths {
        if *state != crate::detect::PathState::Clean && bound.take() {
            let _ = writeln!(s, "delta path {} {}", state.name(), field(path));
        }
    }
    bound.close(&mut s, Kind::Path);
    for path in &changes.stat_missed {
        if bound.take() {
            let _ = writeln!(s, "delta stat-missed {}", field(path));
        }
    }
    bound.close(&mut s, Kind::StatMissed);
    let file_fields = |st: &State, row: u32| -> String {
        st.files.get(row as usize).map_or_else(
            || "- -".to_owned(),
            |f| {
                let member =
                    if f.member != 0 { field(st.str(f.member_name)) } else { "-".to_owned() };
                format!("{} {member}", field(st.str(f.path)))
            },
        )
    };
    for (i, (_, state)) in changes.new_files.iter().enumerate() {
        if state.refills_all() && bound.take() {
            let _ = writeln!(s, "delta file {} {}", state.name(), file_fields(new, i as u32));
        }
    }
    for (i, (_, state)) in changes.old_files.iter().enumerate() {
        if matches!(state, FileState::Gone | FileState::Unextracted) && bound.take() {
            let _ = writeln!(s, "delta file {} {}", state.name(), file_fields(old, i as u32));
        }
    }
    bound.close(&mut s, Kind::File);
    let present = |p: Option<bool>| match p {
        Some(true) => "present",
        Some(false) => "absent",
        None => "-",
    };
    for (path, a, b) in &changes.probes {
        if bound.take() {
            let _ = writeln!(s, "delta probe {} {} {}", field(path), present(*a), present(*b));
        }
    }
    bound.close(&mut s, Kind::Probe);
    let y = &d.symbols;
    let _ = writeln!(
        s,
        "delta symbols new={} gone={} definer={} value={} size={} kind={} attrs={} comdat={} commons={} live={} dead={} tls={}",
        y.new,
        y.gone,
        y.definer,
        y.value,
        y.size,
        y.kind,
        y.attrs,
        y.comdat,
        y.commons,
        y.live,
        y.dead,
        y.tls
    );
    for (what, name) in &d.symbol_lines {
        if bound.take() {
            let _ = writeln!(s, "delta symbol {what} {}", field(name.as_bytes()));
        }
    }
    bound.close(&mut s, Kind::Symbol);
    for f in &d.fallbacks {
        if bound.take() {
            let (name, detail) = (f.reason.name(), f.reason.detail());
            let _ = writeln!(s, "delta fallback {name} {}", field(detail.as_bytes()));
        }
    }
    bound.close(&mut s, Kind::Fallback);
    if let Some(p) = &d.policy {
        let _ = writeln!(s, "delta policy too-much-changed {}", field(p.as_bytes()));
    }
    let _ = writeln!(
        s,
        "delta refill units={} holes={} chunks={} bytes={}",
        d.units.len(),
        d.holes.len(),
        d.chunks.len(),
        d.unit_bytes
    );
    // Past the bound a line is counted, not formatted.
    for u in &d.units {
        if !bound.take() {
            continue;
        }
        let x = &new.sections[u.section as usize];
        let shndx = extra.shndx.get(u.section as usize).copied().unwrap_or(0);
        let _ = writeln!(
            s,
            "delta unit {} {} {shndx} {}",
            reason::names(u.reasons),
            file_fields(new, x.file),
            field(new.str(x.name))
        );
    }
    bound.close(&mut s, Kind::Unit);
    let a = Side::new(old);
    for h in &d.holes {
        if !bound.take() {
            continue;
        }
        let x = &old.sections[h.old as usize];
        let shndx = if h.new == NONE {
            "-".to_owned()
        } else {
            extra.shndx.get(h.new as usize).map_or_else(|| "-".to_owned(), u32::to_string)
        };
        let addr = a.addr(h.old).map_or_else(|| "-".to_owned(), |v| format!("{v:x}"));
        let _ = writeln!(
            s,
            "delta hole {} {} {shndx} {addr} {:x} {}",
            reason::names(h.reasons),
            file_fields(old, x.file),
            x.size,
            field(old.str(x.name))
        );
    }
    bound.close(&mut s, Kind::Hole);
    for c in &d.chunks {
        if !bound.take() {
            continue;
        }
        let row = &new.chunks[c.chunk as usize];
        let out = new.outsecs.get(row.out as usize).map_or(&[][..], |o| new.str(o.name));
        let _ = writeln!(
            s,
            "delta chunk {} {} {} {}",
            reason::names(c.reasons),
            TAG_NAMES.get(row.tag as usize).copied().unwrap_or("?"),
            field(new.str(row.name)),
            field(out)
        );
    }
    bound.close(&mut s, Kind::Chunk);
    let _ = writeln!(s, "delta listed bound={REPORT_BOUND} elided={}", bound.elided);
    s.push_str("delta end\n");
    s
}

#[cfg(test)]
mod tests {
    use super::{Bounded, Kind, REPORT_BOUND};

    // spec: incr.md IN-10, docs/cli.md §3.13 (the report lists at most
    // REPORT_BOUND lines per kind and says what it left out), WP-106
    #[test]
    fn the_report_bound() {
        let mut s = String::new();
        let mut b = Bounded::default();
        // Under and at the bound: every line, no `elided` line.
        for n in [0, 1, REPORT_BOUND] {
            assert!((0..n).all(|_| b.take()), "{n}");
            b.close(&mut s, Kind::Unit);
        }
        assert_eq!((s.as_str(), b.elided), ("", 0));
        // Past it: the first REPORT_BOUND, then the total.
        let taken = (0..REPORT_BOUND + 5).filter(|_| b.take()).count();
        assert_eq!(taken, REPORT_BOUND);
        b.close(&mut s, Kind::Hole);
        let taken = (0..3 * REPORT_BOUND).filter(|_| b.take()).count();
        assert_eq!(taken, REPORT_BOUND);
        b.close(&mut s, Kind::Symbol);
        assert_eq!(
            s,
            format!(
                "delta elided hole listed={REPORT_BOUND} total={}\n\
                 delta elided symbol listed={REPORT_BOUND} total={}\n",
                REPORT_BOUND + 5,
                3 * REPORT_BOUND
            )
        );
        assert_eq!(b.elided, 5 + 2 * REPORT_BOUND);
    }
}
