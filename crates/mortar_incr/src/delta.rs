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

use std::collections::{HashMap, HashSet};

use crate::detect::{Changes, FileState};
use crate::format::{
    ChunkRow, Hash16, MentionRow, NONE, ResolvedRow, SectionRow, State, def_kind, index_kind,
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
    pub reason: &'static str,
    pub detail: String,
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

/// A section's identity: file (as a NEW `FILES` row), anchor, name,
/// ordinal.
type SecId<'s> = (u32, u8, &'s [u8], &'s [u8], u32);

/// Where a placed unit is: allocated by address; non-allocated by its
/// output section and offset in it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Pos<'s> {
    Alloc(u64),
    NonAlloc(&'s [u8], u32, u64),
}

struct Side<'s> {
    s: &'s State,
    /// Per `OUTSECS` row, the least file offset of its chunks.
    out_start: Vec<u64>,
}

impl<'s> Side<'s> {
    fn new(s: &'s State) -> Self {
        let mut out_start = vec![u64::MAX; s.outsecs.len()];
        for c in &s.chunks {
            if let Some(o) = out_start.get_mut(c.out as usize) {
                *o = (*o).min(c.offset);
            }
        }
        Self { s, out_start }
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
        let d = self.s.defined.iter().find(|d| d.symbol == symbol)?;
        if d.kind == 0 {
            return Some(d.value);
        }
        Some(self.chunk(d.chunk)?.addr.wrapping_add(d.value))
    }

    /// A common's allocation (`SYM_PLACE`): address and size.
    fn common_place(&self, symbol: u32) -> Option<(u64, u64)> {
        let p = self.s.sym_place.iter().find(|p| p.symbol == symbol)?;
        Some((self.chunk(p.chunk)?.addr.wrapping_add(p.offset), p.size))
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

fn resolved_map(s: &State) -> HashMap<KeyId<'_>, (u32, ResolvedRow)> {
    s.resolved
        .iter()
        .filter(|r| r.kind <= def_kind::SHARED)
        .filter_map(|r| Some((key_id(s, r.symbol)?, (r.symbol, *r))))
        .collect()
}

fn globals_in(s: &State, sec: u32) -> Vec<&MentionRow> {
    s.mentions.iter().filter(|m| m.section == sec && m.kind == def_kind::SECTION).collect()
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
    let (a, b) = (Side::new(old), Side::new(new));
    let nfile = |row: u32| changes.new_files.get(row as usize).map_or(FileState::Dead, |f| f.1);
    let unchanged_new = |row: u32| nfile(row) == FileState::Unchanged;
    let ofile = |row: u32| changes.old_files.get(row as usize).map_or(FileState::Dead, |f| f.1);
    let old_to_new_file = |row: u32| changes.old_files.get(row as usize).and_then(|f| f.0);
    let fallback = |d: &mut Delta, reason: &'static str, detail: String| {
        if !drop.fallbacks.contains(&reason) && !d.fallbacks.iter().any(|f| f.reason == reason) {
            d.fallbacks.push(Fallback { reason, detail });
        }
    };
    // ---- sections by identity -------------------------------------------
    let new_by_id: HashMap<SecId<'_>, u32> = new
        .sections
        .iter()
        .enumerate()
        .map(|(i, x)| {
            ((x.file, x.anchor_kind, new.str(x.anchor), new.str(x.name), x.ordinal), i as u32)
        })
        .collect();
    let mut sec_o2n: Vec<u32> = vec![NONE; old.sections.len()];
    let mut sec_n2o: Vec<u32> = vec![NONE; new.sections.len()];
    for (i, x) in old.sections.iter().enumerate() {
        let Some(f) = old_to_new_file(x.file) else { continue };
        let id = (f, x.anchor_kind, old.str(x.anchor), old.str(x.name), x.ordinal);
        if let Some(&n) = new_by_id.get(&id) {
            sec_o2n[i] = n;
            sec_n2o[n as usize] = i as u32;
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
        fallback(&mut d, "new-output-section", detail);
    }
    let prop = |s: &Side<'_>| s.chunk_by_name(b".note.gnu.property").map(|c| c.size);
    if prop(&a) != prop(&b) {
        fallback(&mut d, "property-changed", format!("{:?} -> {:?} bytes", prop(&a), prop(&b)));
    }
    if let Some(p) = &changes.script_changed {
        fallback(
            &mut d,
            "options-changed",
            format!("{} changed and is not an object or archive", String::from_utf8_lossy(p)),
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
                "inputs-reordered",
                format!("{} moved before {}", name(old, o), name(new, n as usize)),
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
                    "tls-layout-changed",
                    format!("{file}:{name} {tp_a:#x} -> {tp_b:#x}"),
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
    // link only, or a hash with several offsets (ordinal rows). A FRAGS
    // offset is relative to its output section's merged chunk, so the
    // place is that chunk's address (allocated) or its offset in the
    // output section (non-allocated) plus the offset: a merged chunk that
    // moves moves every fragment in it (the oracle's `os` scenario).
    let frag_map = |side: &Side<'_>| -> HashMap<(Vec<u8>, u32, Hash16), Option<u64>> {
        let s = side.s;
        let mut base: HashMap<u32, Option<u64>> = HashMap::new();
        for c in &s.chunks {
            if c.tag == TAG_MERGED {
                let at = if c.sh_flags & SHF_ALLOC != 0 {
                    c.addr
                } else {
                    c.offset.wrapping_sub(
                        side.out_start.get(c.out as usize).copied().unwrap_or(c.offset),
                    )
                };
                // Several merged chunks of one output section: no one base.
                base.entry(c.out).and_modify(|e| *e = None).or_insert(Some(at));
            }
        }
        let mut m: HashMap<(Vec<u8>, u32, Hash16), Option<u64>> = HashMap::new();
        for f in &s.frags {
            let Some(o) = s.outsecs.get(f.out as usize) else { continue };
            let k = (s.str(o.name).to_vec(), o.ordinal, f.hash);
            let at = base.get(&f.out).copied().flatten();
            let v = if f.ordinal == NONE { at.map(|b| b.wrapping_add(f.offset)) } else { None };
            m.entry(k).and_modify(|e| *e = None).or_insert(v);
        }
        m
    };
    let (fa, fb) = (frag_map(&a), frag_map(&b));
    let mut merged_moved = vec![false; nsec];
    let mut frags_changed = fa.len() != fb.len();
    for (sec, out, h) in &extra.frags {
        let Some(o) = new.outsecs.get(*out as usize) else { continue };
        let k = (new.str(o.name).to_vec(), o.ordinal, *h);
        let (x, y) = (fa.get(&k).copied().flatten(), fb.get(&k).copied().flatten());
        if x.is_none() || x != y {
            if let Some(m) = merged_moved.get_mut(*sec as usize) {
                *m = true;
            }
            frags_changed = true;
        }
    }
    if !frags_changed {
        frags_changed = fa.iter().any(|(k, v)| fb.get(k) != Some(v));
    }
    // .eh_frame: the prefix rule over the records in output order.
    let eh_chunk_moved = a.chunk_by_name(b".eh_frame").map(|c| c.addr)
        != b.chunk_by_name(b".eh_frame").map(|c| c.addr);
    let mut k = 0;
    if !eh_chunk_moved {
        while k < old.eh_records.len() && k < new.eh_records.len() {
            let (x, y) = (&old.eh_records[k], &new.eh_records[k]);
            let file = new.sections.get(y.section as usize).map_or(NONE, |s| s.file);
            if sec_o2n.get(x.section as usize) != Some(&y.section)
                || x.piece != y.piece
                || !unchanged_new(file)
            {
                break;
            }
            k += 1;
        }
    }
    let eh_changed = k < old.eh_records.len().max(new.eh_records.len());
    let mut eh_moved = vec![false; nsec];
    for y in new.eh_records.iter().skip(k) {
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
    let mut eh_of: HashMap<u32, Vec<u32>> = HashMap::new();
    let mut nonalloc_of: HashMap<u32, Vec<u32>> = HashMap::new();
    for (i, x) in new.sections.iter().enumerate() {
        if !present[i] {
            continue;
        }
        if is_eh(new, x) {
            eh_of.entry(x.file).or_default().push(i as u32);
        } else if x.sh_flags & SHF_ALLOC == 0 && x.place_chunk != NONE {
            nonalloc_of.entry(x.file).or_default().push(i as u32);
        }
    }
    // The index backwards: target -> units.
    let mut by_global: HashMap<u32, Vec<u32>> = HashMap::new();
    let mut by_section: HashMap<u32, Vec<u32>> = HashMap::new();
    let mut slot_users: Vec<u32> = Vec::new();
    let mut eh_refs: Vec<u32> = Vec::new();
    for x in &new.index {
        match x.kind {
            index_kind::GLOBAL => by_global.entry(x.target).or_default().push(x.unit),
            index_kind::SECTION => by_section.entry(x.target).or_default().push(x.unit),
            index_kind::SLOT | index_kind::LOCAL_SLOT => slot_users.push(x.unit),
            index_kind::EH_FRAME => eh_refs.push(x.unit),
            _ => {}
        }
    }
    // Mentions by key (new SYMBOLS row), for the non-alloc referrers.
    let mut files_mentioning: HashMap<u32, Vec<u32>> = HashMap::new();
    for m in &new.mentions {
        files_mentioning.entry(m.symbol).or_default().push(m.file);
    }
    let mut set = Set { units: vec![0; nsec], drop: drop.reasons };
    let referrers_of_key = |set: &mut Set, key: u32, r: u32| {
        for &u in by_global.get(&key).into_iter().flatten() {
            set.add(u, r);
        }
        for f in files_mentioning.get(&key).into_iter().flatten() {
            for &u in nonalloc_of.get(f).into_iter().flatten() {
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
    let (ra, rb) = (resolved_map(old), resolved_map(new));
    // Keys an unchanged file mentions (in the old state).
    let mut unchanged_mentions: HashSet<u32> = HashSet::new();
    for m in &old.mentions {
        if ofile(m.file) == FileState::Unchanged {
            unchanged_mentions.insert(m.symbol);
        }
    }
    let file_id = |s: &State, side: bool, f: u32| -> Option<u32> {
        // As a NEW files row (the old side translated).
        if f == NONE {
            return None;
        }
        let _ = s;
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
    let mut keys: Vec<KeyId<'_>> = ra.keys().chain(rb.keys()).copied().collect();
    keys.sort_unstable();
    keys.dedup();
    let key_name = |k: &KeyId<'_>| {
        let mut n = String::from_utf8_lossy(k.1).into_owned();
        if k.0 == 1 {
            n.push('@');
            n.push_str(&String::from_utf8_lossy(k.2));
        }
        n
    };
    for k in &keys {
        let (Some(&(osym, o)), Some(&(nsym, n))) = (ra.get(k), rb.get(k)) else {
            if rb.contains_key(k) {
                d.symbols.new += 1;
                d.symbol_lines.push(("new", key_name(k)));
            } else {
                d.symbols.gone += 1;
                d.symbol_lines.push(("gone", key_name(k)));
            }
            continue;
        };
        let defined = |r: &ResolvedRow| r.kind != def_kind::UNDEFINED;
        // Attributes (IN-26 v1): type, binding, st_other, or became
        // absolute or common, for a key an unchanged file mentions.
        let special = |r: &ResolvedRow| matches!(r.kind, def_kind::ABSOLUTE | def_kind::COMMON);
        let attrs =
            defined(&o) && defined(&n) && (o.typ, o.bind, o.other) != (n.typ, n.bind, n.other);
        let became = o.kind != n.kind && (special(&o) || special(&n));
        if attrs || became {
            d.symbols.attrs += 1;
            d.symbol_lines.push(("attrs", key_name(k)));
            if unchanged_mentions.contains(&osym) {
                fallback(&mut d, "symbol-kind-changed", key_name(k));
            }
        }
        // The definition (IN-27 row 3).
        let definer = file_id(old, false, o.file) != file_id(new, true, n.file);
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
            d.symbol_lines.push((w, key_name(k)));
            referrers_of_key(&mut set, nsym, reason::GLOBAL);
        }
    }
    // COMDAT keepers (IN-27 row 4).
    let keepers = |s: &'_ State, side: bool| -> HashMap<Vec<u8>, Option<u32>> {
        s.resolved
            .iter()
            .filter(|r| r.kind == def_kind::COMDAT)
            .map(|r| (s.str(r.name).to_vec(), file_id(s, side, r.file)))
            .collect()
    };
    let (ka, kb) = (keepers(old, false), keepers(new, true));
    let mut changed_groups: HashSet<Vec<u8>> = HashSet::new();
    for (sig, keeper) in &ka {
        if kb.get(sig) != Some(keeper) {
            changed_groups.insert(sig.clone());
        }
    }
    for sig in kb.keys() {
        if !ka.contains_key(sig) {
            changed_groups.insert(sig.clone());
        }
    }
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
        for &u in eh_of
            .get(&x.file)
            .into_iter()
            .flatten()
            .chain(nonalloc_of.get(&x.file).into_iter().flatten())
        {
            set.add(u, file_row);
        }
        for m in globals_in(new, n as u32) {
            referrers_of_key(&mut set, m.symbol, row);
        }
        let o = sec_n2o[n];
        for m in globals_in(old, o) {
            if let Some(k) = key_id(old, m.symbol)
                && let Some(&(nsym, _)) = rb.get(&k)
            {
                referrers_of_key(&mut set, nsym, row);
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
            for &u in by_section.get(&(n as u32)).into_iter().flatten() {
                set.add(u, reason::MOVED_REF);
            }
            for &u in nonalloc_of.get(&x.file).into_iter().flatten() {
                set.add(u, reason::MOVED_FILE);
            }
        }
        if merged_moved[n] {
            if present[n] {
                set.add(n as u32, reason::FRAG);
            }
            for &u in by_section.get(&(n as u32)).into_iter().flatten() {
                set.add(u, reason::FRAG);
            }
            for &u in nonalloc_of.get(&x.file).into_iter().flatten() {
                set.add(u, reason::FRAG);
            }
        }
        if eh_moved[n] {
            set.add(n as u32, reason::EH);
        }
    }
    // References into `.eh_frame` (row 10, S-50).
    if eh_changed {
        for &u in &eh_refs {
            set.add(u, reason::EH_REF);
        }
    }
    // Slot tables (rows 5 and 7).
    let slot_names: [&[u8]; 6] =
        [b".got", b".got.plt", b".plt", b".iplt", b".rela.plt", b".rela.iplt"];
    let slot_chunks_moved = slot_names.iter().any(|n| {
        a.chunk_by_name(n).map(|c| (c.addr, c.size)) != b.chunk_by_name(n).map(|c| (c.addr, c.size))
    });
    let slots_changed = slot_chunks_moved
        || slot_list(old, &|f| file_id(old, false, f))
            != slot_list(new, &|f| file_id(new, true, f));
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
            fallback(&mut d, "link-order-moved", name);
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
    let mut touched: HashSet<u32> = HashSet::new();
    for u in &d.units {
        let x = &new.sections[u.section as usize];
        if x.place_chunk != NONE {
            touched.insert(x.place_chunk);
        }
    }
    let mut old_touched: HashSet<(u8, &[u8], u32)> = HashSet::new();
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
        if *state != crate::detect::PathState::Clean {
            let _ = writeln!(s, "delta path {} {}", state.name(), field(path));
        }
    }
    for path in &changes.stat_missed {
        let _ = writeln!(s, "delta stat-missed {}", field(path));
    }
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
        if state.refills_all() {
            let _ = writeln!(s, "delta file {} {}", state.name(), file_fields(new, i as u32));
        }
    }
    for (i, (_, state)) in changes.old_files.iter().enumerate() {
        if matches!(state, FileState::Gone | FileState::Unextracted) {
            let _ = writeln!(s, "delta file {} {}", state.name(), file_fields(old, i as u32));
        }
    }
    let present = |p: Option<bool>| match p {
        Some(true) => "present",
        Some(false) => "absent",
        None => "-",
    };
    for (path, a, b) in &changes.probes {
        let _ = writeln!(s, "delta probe {} {} {}", field(path), present(*a), present(*b));
    }
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
        let _ = writeln!(s, "delta symbol {what} {}", field(name.as_bytes()));
    }
    for f in &d.fallbacks {
        let _ = writeln!(s, "delta fallback {} {}", f.reason, field(f.detail.as_bytes()));
    }
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
    for u in &d.units {
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
    let a = Side::new(old);
    for h in &d.holes {
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
    for c in &d.chunks {
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
    s.push_str("delta end\n");
    s
}
