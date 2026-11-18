//! WP-238 step 1's probe (`spec/incr.md` IN-29, IN-71a): behind the Cargo
//! feature `wp238-probe` of `mortar_live` (off by default: a default
//! build has no line of it). With the feature and `MORTAR_WP238=<dir>`, a
//! `--gc-sections` link, after its mark, runs the one edge enumerator
//! ([`Walk::edges`]) over every kept section, walks the rows from the
//! roots (a single-threaded worklist, as an update would), compares the
//! walk with the mark at section and fragment level, drops each row kind
//! in turn (IN-63b), and writes `<dir>/<pid>-<n>.sum` (the summary) and,
//! with `MORTAR_WP238_ROWS` set, `<dir>/<pid>-<n>.rows` (the rows named by
//! file, section index and key name, for `tests/incr-design/wp238/`).
//!
//! Experiment code: it is not the design's code and is deleted in step 2.
#![allow(
    clippy::many_single_char_names,
    clippy::bool_to_int_with_if,
    clippy::needless_range_loop,
    reason = "experiment code"
)]

use std::fmt::Write as _;
use std::sync::atomic::Ordering;
use std::time::Instant;

use mortar_core::inputs::Reader;
use mortar_core::{SectionId, SymbolId};

use super::{ALLOC, EDGES, Marks, ROOT, Target, To, Unit, Via, WHOLE, Walk, c_identifier};

/// IN-29's row kinds, as `mortar_incr::update::rows::edge_kind` numbers
/// them, plus the two kinds step 1 proposes (7, 8).
const KIND_NAMES: [&str; 9] = [
    "reloc",
    "startstop",
    "local",
    "fde_section",
    "fde_key",
    "nonalloc_key",
    "nonalloc_section",
    "linkorder",
    "group",
];

/// The row kind of an edge (IN-29's table): from how it was reached, what
/// it names, the source's facts and the key's name.
fn kind_of(w: &Walk<'_, '_, '_>, via: Via, to: To, src_bits: u8) -> u8 {
    let alloc = src_bits & ALLOC != 0;
    match (via, to) {
        (Via::Record, To::Key(k)) => {
            if !alloc {
                5
            } else if start_stop(w, k) {
                1
            } else {
                0
            }
        }
        (Via::Record, To::Section(..)) => {
            if alloc {
                2
            } else {
                6
            }
        }
        (Via::Frame, To::Section(..)) => 3,
        (Via::Frame, To::Key(_)) => 4,
        (Via::LinkOrder, _) => 7,
        (Via::Group, _) => 8,
    }
}

fn start_stop(w: &Walk<'_, '_, '_>, k: SymbolId) -> bool {
    let name = w.inputs.symbols.get(k).map_or(&[][..], |s| s.name);
    name.strip_prefix(b"__start_")
        .or_else(|| name.strip_prefix(b"__stop_"))
        .is_some_and(c_identifier)
}

#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
struct Row {
    src: u32,
    kind: u8,
    to: To,
}

/// The probe; `make` gives a fresh `Marks`, `roots` marks round 0.
#[allow(clippy::too_many_lines, reason = "experiment code, deleted in step 2")]
pub(super) fn run<'t>(
    w: &Walk<'_, 't, '_>,
    make: &dyn Fn() -> Marks<'t>,
    roots: &dyn Fn(&Marks<'t>, &mut Vec<SectionId>),
    t: &crate::LiveInputs<'_, '_>,
    live: &[bool],
    frag: &[bool],
) {
    let root_keys = t.roots;
    let Some(dir) = std::env::var_os("MORTAR_WP238") else { return };
    let dir = std::path::PathBuf::from(dir);
    let n = w.r.sections.len() as usize;

    // 1. The enumerator over every kept section.
    let t0 = Instant::now();
    let mut rows: Vec<Row> = Vec::new();
    let mut raw = [0u64; 9];
    let mut failed = 0u32;
    for s in 0..n {
        let sid = SectionId(s as u32);
        if !w.r.sections[sid].is_kept() {
            continue;
        }
        let bits = w.facts.get(s).copied().unwrap_or(ROOT | EDGES);
        let hi = w.records_of(sid);
        let u = Unit { s: sid, lo: 0, hi };
        let r = w.edges(u, &mut |via, to| {
            let kind = kind_of(w, via, to, bits);
            raw[kind as usize] += 1;
            rows.push(Row { src: sid.0, kind, to });
        });
        if r.is_err() {
            failed += 1;
        }
    }
    let t_enum = t0.elapsed();
    rows.sort_unstable();
    rows.dedup();
    let t_sort = t0.elapsed();
    // Rows by source: `first[s]..first[s + 1]`.
    let mut first = vec![0u32; n + 1];
    for r in &rows {
        first[r.src as usize + 1] += 1;
    }
    for s in 0..n {
        first[s + 1] += first[s];
    }

    // 2. The walk from the roots over the rows (one thread, a worklist).
    let walk = |drop: Option<u8>| -> (Vec<bool>, Vec<bool>) {
        let marks = make();
        let mut stack = Vec::new();
        roots(&marks, &mut stack);
        while let Some(s) = stack.pop() {
            let span = first[s.0 as usize] as usize..first[s.0 as usize + 1] as usize;
            for r in &rows[span] {
                if Some(r.kind) == drop {
                    continue;
                }
                marks.target(w.resolve(r.to), &mut stack);
            }
        }
        let l = marks.live.iter().map(|b| b.load(Ordering::Relaxed)).collect();
        let f = marks.frag.iter().map(|b| b.load(Ordering::Relaxed)).collect();
        (l, f)
    };
    let mut best = u128::MAX;
    let mut got = (Vec::new(), Vec::new());
    for _ in 0..3 {
        let t = Instant::now();
        got = walk(None);
        best = best.min(t.elapsed().as_micros());
    }
    let (wl, wf) = got;
    // 2b. The update's path (step 1 item 3): the rows as 17-byte records
    // (IN-18e's `EdgeRow`: kind u8, source u32, target u32, offset u64),
    // then decoded, indexed by source and walked, timed together.
    let key_kind = |k: u8| matches!(k, 0 | 1 | 4 | 5);
    let mut bytes = Vec::with_capacity(rows.len() * 17);
    for r in &rows {
        let (t, off) = match r.to {
            To::Key(k) => (k.0, 0u64),
            To::Section(s, o) => (s.0, if o == WHOLE { u64::MAX } else { u64::from(o) }),
        };
        bytes.push(r.kind);
        bytes.extend_from_slice(&r.src.to_le_bytes());
        bytes.extend_from_slice(&t.to_le_bytes());
        bytes.extend_from_slice(&off.to_le_bytes());
    }
    let stored = || -> (Vec<bool>, u128) {
        let t = Instant::now();
        let u32_at = |b: &[u8], i: usize| u32::from_le_bytes([b[i], b[i + 1], b[i + 2], b[i + 3]]);
        let mut dec: Vec<(u32, To)> = Vec::with_capacity(bytes.len() / 17);
        let mut idx = vec![0u32; n + 1];
        for c in bytes.chunks_exact(17) {
            let src = u32_at(c, 1);
            let tgt = u32_at(c, 5);
            let off = u64::from_le_bytes([c[9], c[10], c[11], c[12], c[13], c[14], c[15], c[16]]);
            let to = if key_kind(c[0]) {
                To::Key(SymbolId(tgt))
            } else {
                To::Section(SectionId(tgt), if off == u64::MAX { WHOLE } else { off as u32 })
            };
            idx[src as usize + 1] += 1;
            dec.push((src, to));
        }
        for s in 0..n {
            idx[s + 1] += idx[s];
        }
        let marks = make();
        let mut stack = Vec::new();
        roots(&marks, &mut stack);
        while let Some(s) = stack.pop() {
            for (_, to) in &dec[idx[s.0 as usize] as usize..idx[s.0 as usize + 1] as usize] {
                marks.target(w.resolve(*to), &mut stack);
            }
        }
        let l: Vec<bool> = marks.live.iter().map(|b| b.load(Ordering::Relaxed)).collect();
        (l, t.elapsed().as_micros())
    };
    let mut best_stored = u128::MAX;
    let mut stored_diff = 0;
    for _ in 0..3 {
        let (l, us) = stored();
        best_stored = best_stored.min(us);
        stored_diff = l.iter().zip(live).filter(|(x, y)| x != y).count();
    }
    let diff = |a: &[bool], b: &[bool]| a.iter().zip(b).filter(|(x, y)| x != y).count();
    let lost = |a: &[bool], b: &[bool]| a.iter().zip(b).filter(|(x, y)| **x && !**y).count();
    let sec_diff = diff(live, &wl);
    let frag_diff = diff(frag, &wf);

    let mut sum = String::new();
    let out_name = w.inputs.files.iter().count();
    let _ = writeln!(sum, "files {out_name} sections {n} frags {}", frag.len());
    let kept = (0..n).filter(|&s| w.r.sections[SectionId(s as u32)].is_kept()).count();
    let nlive = live.iter().filter(|b| **b).count();
    let nroot = (0..n)
        .filter(|&s| {
            w.r.sections[SectionId(s as u32)].is_kept()
                && w.facts.get(s).is_none_or(|b| b & ROOT != 0)
        })
        .count();
    let _ =
        writeln!(sum, "kept {kept} live {nlive} root-sections {nroot} enumerate-failed {failed}");
    let _ = writeln!(
        sum,
        "enumerate_us {} sort_us {} walk_us(best of 3) {best}",
        t_enum.as_micros(),
        t_sort.as_micros()
    );
    let _ = writeln!(sum, "replay: sections differ {sec_diff} fragments differ {frag_diff}");
    let _ = writeln!(
        sum,
        "stored-form: {} bytes at 17 B a row; decode + index + walk (best of 3) {best_stored} us, sections differ {stored_diff}",
        bytes.len()
    );
    let mut distinct = [0u64; 9];
    let mut live_src = [0u64; 9];
    for r in &rows {
        distinct[r.kind as usize] += 1;
        if live.get(r.src as usize).copied().unwrap_or(false) {
            live_src[r.kind as usize] += 1;
        }
    }
    for k in 0..9 {
        let _ = writeln!(
            sum,
            "kind {:<17} raw {:>9} distinct {:>9} live-source {:>9}",
            KIND_NAMES[k], raw[k], distinct[k], live_src[k]
        );
    }
    let _ = writeln!(
        sum,
        "total distinct {} live-source {}",
        distinct.iter().sum::<u64>(),
        live_src.iter().sum::<u64>()
    );
    // The stored form's other counts (step 1 item 2): group rows as a ring
    // (one per member of a group of 2 or more), root sections with
    // outgoing edges or allocated (the ones a walk must start from),
    // C-identifier named kept sections (`__start_`/`__stop_` targets).
    let ring: std::collections::BTreeSet<u32> =
        rows.iter().filter(|r| r.kind == 8).map(|r| r.src).collect();
    let walk_roots = (0..n)
        .filter(|&s| {
            w.r.sections[SectionId(s as u32)].is_kept()
                && w.facts.get(s).is_none_or(|b| b & ROOT != 0 && b & (ALLOC | EDGES) != 0)
        })
        .count();
    let srcs: std::collections::BTreeSet<u32> = rows.iter().map(|r| r.src).collect();
    let _ = writeln!(
        sum,
        "group-ring rows {} walk-roots {walk_roots} named-c-ident-sections {} sources-with-rows {}",
        ring.len(),
        w.marks.named.len(),
        srcs.len()
    );
    // Keys defined in a split section that a row names (fragment-level
    // liveness through a key, step 1 item 2).
    let mut split_keys = 0u64;
    let mut split_key_rows = 0u64;
    let mut seen = std::collections::BTreeSet::new();
    for r in &rows {
        if let To::Key(k) = r.to
            && let Target::At(s, _) = w.resolve(r.to)
            && w.split.merge_of.binary_search_by_key(&s, |x| x.section).is_ok()
        {
            split_key_rows += 1;
            if seen.insert(k) {
                split_keys += 1;
            }
        }
    }
    let _ = writeln!(
        sum,
        "keys defined in split sections named by rows {split_keys} rows {split_key_rows}"
    );
    // Named targets (__start_/__stop_ resolved to a conditional PROVIDE).
    let named = rows.iter().filter(|r| matches!(w.resolve(r.to), Target::Named(..))).count();
    let _ = writeln!(sum, "rows resolving to __start_/__stop_ sets {named}");
    // 3. Each kind dropped (IN-63b).
    for k in 0..9u8 {
        if distinct[k as usize] == 0 {
            let _ = writeln!(sum, "drop {:<17} no rows", KIND_NAMES[k as usize]);
            continue;
        }
        let (l, f) = walk(Some(k));
        let ls = lost(live, &l);
        let lf = lost(frag, &f);
        let mut example = String::new();
        if let Some(s) = (0..n).find(|&s| live[s] && !l[s])
            && let Some((file, index)) = w.r.sh_index(SectionId(s as u32))
        {
            let name = match w.inputs.files[file].reader {
                Reader::Object(o) => o
                    .section(index)
                    .ok()
                    .and_then(|x| x.name().ok())
                    .map(|x| String::from_utf8_lossy(x).into_owned())
                    .unwrap_or_default(),
                _ => String::new(),
            };
            let _ = write!(example, " e.g. {:?} {name}", w.inputs.files[file].display());
        }
        let _ = writeln!(
            sum,
            "drop {:<17} sections lost {ls} fragments lost {lf}{example}",
            KIND_NAMES[k as usize]
        );
    }
    let tag = format!(
        "{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |d| d.as_nanos())
    );
    let _ = std::fs::write(dir.join(format!("{tag}.sum")), sum);
    if std::env::var_os("MORTAR_WP238_ROWS").is_some() {
        let mut text = dump(w, &rows, root_keys, live, frag);
        stored_dump(w, t, &mut text);
        let _ = std::fs::write(dir.join(format!("{tag}.rows")), text);
    }
}

/// The rows by name: `F` files, `S` sections, `K` keys with their
/// definition, `R` rooted keys, `E` rows, `P` pieces of split sections,
/// `L` live fragments (tab-separated).
fn dump(
    w: &Walk<'_, '_, '_>,
    rows: &[Row],
    root_keys: &[SymbolId],
    live: &[bool],
    frag: &[bool],
) -> String {
    let mut o = String::new();
    for k in root_keys {
        let _ = writeln!(o, "R\t{}", k.0);
    }
    for (f, file) in w.inputs.files.iter() {
        let member = file.member.map_or_else(
            || "-".to_owned(),
            |m| format!("{}#{}", String::from_utf8_lossy(m.name), m.ordinal),
        );
        let hash = match file.reader {
            Reader::Object(obj) => {
                let d = obj.data();
                d.get(0, d.len()).map_or(0, mortar_core::hash::hash_bytes)
            }
            _ => 0,
        };
        let _ = writeln!(
            o,
            "F\t{}\t{}\t{member}\t{hash:016x}",
            f.0,
            String::from_utf8_lossy(file.path)
        );
    }
    for s in 0..w.r.sections.len() as usize {
        let sid = SectionId(s as u32);
        let Some((file, index)) = w.r.sh_index(sid) else { continue };
        let Reader::Object(obj) = w.inputs.files[file].reader else { continue };
        let name = obj
            .section(index)
            .ok()
            .and_then(|x| x.name().ok())
            .map(|x| String::from_utf8_lossy(x).into_owned())
            .unwrap_or_default();
        let split = w.split.merge_of.binary_search_by_key(&sid, |x| x.section).is_ok();
        let _ = writeln!(
            o,
            "S\t{s}\t{}\t{}\t{name}\t{}\t{}\t{}\t{}",
            file.0,
            index.0,
            u8::from(w.r.sections[sid].is_kept()),
            u8::from(live[s]),
            w.facts.get(s).copied().unwrap_or(ROOT | EDGES),
            u8::from(split)
        );
    }
    for (k, key) in w.inputs.symbols.iter() {
        let name = String::from_utf8_lossy(key.name);
        let v = key.version.map(|v| format!("@{}", String::from_utf8_lossy(v))).unwrap_or_default();
        let (t, rooted) = w.defs.get(k.0 as usize).copied().unwrap_or((Target::None, false));
        let t = match t {
            Target::None => "-".to_owned(),
            Target::At(s, at) => {
                format!("S {} {}", s.0, if at == WHOLE { -1 } else { i64::from(at) })
            }
            Target::Named(lo, hi) => {
                let names: Vec<String> = w
                    .marks
                    .named
                    .get(lo as usize..hi as usize)
                    .unwrap_or_default()
                    .iter()
                    .map(|(_, s)| s.0.to_string())
                    .collect();
                format!("N {}", names.join(","))
            }
        };
        let _ = writeln!(o, "K\t{}\t{name}{v}\t{t}\t{}", k.0, u8::from(rooted));
    }
    for r in rows {
        let t = match r.to {
            To::Key(k) => format!("K\t{}", k.0),
            To::Section(s, at) => {
                format!("S\t{}\t{}", s.0, if at == WHOLE { -1 } else { i64::from(at) })
            }
        };
        let _ = writeln!(o, "E\t{}\t{}\t{t}", r.src, r.kind);
    }
    for row in &w.split.merge_of {
        let pieces = row.pieces;
        for p in pieces.start..pieces.start + pieces.len {
            let off = w.split.piece_off.get(p as usize).copied().unwrap_or(0);
            let f = w.split.piece_frag.get(p as usize).map_or(u32::MAX, |f| f.0);
            let _ = writeln!(o, "P\t{}\t{off}\t{f}", row.section.0);
        }
    }
    for (f, l) in frag.iter().enumerate() {
        if *l {
            let _ = writeln!(o, "L\t{f}");
        }
    }
    o
}

// ---- WP-238 step 2, Part 0: the stored form after the review -------------

/// The stored row kinds of Part 0 (`spec/landed/incr/WP-238.md` "Step 2"):
/// `mortar_incr::update::rows::edge_kind`'s 0-6, then the seam-9 kinds.
#[allow(dead_code)]
pub(super) mod kind {
    pub const RELOC: u8 = 0;
    pub const START_STOP: u8 = 1;
    pub const LOCAL: u8 = 2;
    pub const FDE_SECTION: u8 = 3;
    pub const FDE_KEY: u8 = 4;
    pub const NONALLOC_KEY: u8 = 5;
    pub const NONALLOC_SECTION: u8 = 6;
    pub const LINK_ORDER: u8 = 7;
    pub const GROUP: u8 = 8;
    pub const ROOT: u8 = 9;
    pub const NAMED: u8 = 10;
    pub const SPLIT: u8 = 11;
    pub const REFUSE: u8 = 12;
}

/// `REFUSE` rows' `target`: why every update of the link falls back.
pub(super) mod refuse {
    /// An FDE whose initial location is named by a non-local symbol
    /// (review F3): its code section is the resolution's, so its edges
    /// belong to whichever file defines the symbol.
    pub const FDE_GLOBAL: u32 = 1;
    /// A non-local symbol defined in a section dropped other than by COMDAT
    /// election (`SHF_EXCLUDE`, consumed, stripped; review F6): its key's
    /// `RESOLVED` row names a section the mark never marks.
    pub const DEF_DROPPED: u32 = 2;
    /// The section's relocations could not be enumerated: its rows are
    /// unknown.
    pub const UNREADABLE: u32 = 3;
}

/// One stored row in the link's id space: `target` is a `SymbolId` for the
/// key kinds (0, 1, 4, 5), a `SectionId` for the section kinds (2, 3, 6,
/// 7, 8), 0 for `ROOT`, `NAMED`, `SPLIT`, a [`refuse`] code for `REFUSE`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub(super) struct Stored {
    pub src: u32,
    pub kind: u8,
    pub target: u32,
    pub offset: u32,
}

fn is_key_kind(k: u8) -> bool {
    matches!(k, kind::RELOC | kind::START_STOP | kind::FDE_KEY | kind::NONALLOC_KEY)
}

/// IN-29's rows of every kept section in their stored form (Part 0): the
/// enumerator's edges normalised (F8: a section target's offset is kept
/// only when the target is split, a non-split self-edge is dropped; F6: a
/// row to a section that is not kept is dropped), the group as a ring,
/// `ROOT`, `NAMED` (target 0, F4), `SPLIT`, and `REFUSE` (F3, F6).
#[allow(clippy::too_many_lines)] // the probe's prototype (R-7: deleted in Part 4)
pub(super) fn stored_rows(w: &Walk<'_, '_, '_>) -> Vec<Stored> {
    let r = w.r;
    let n = r.sections.len() as usize;
    let kept = |s: u32| {
        r.sections.get(SectionId(s)).is_some_and(mortar_core::resolution::SectionRow::is_kept)
    };
    let split = |s: u32| w.facts.get(s as usize).is_some_and(|b| b & super::MERGE != 0);
    let mut out: Vec<Stored> = Vec::new();
    let named: std::collections::BTreeSet<u32> = w.marks.named.iter().map(|(_, s)| s.0).collect();
    for s in 0..n {
        let sid = SectionId(s as u32);
        if !kept(sid.0) {
            continue;
        }
        let bits = w.facts.get(s).copied().unwrap_or(ROOT | EDGES);
        let src = sid.0;
        let mut push = |kind: u8, target: u32, offset: u32| {
            out.push(Stored { src, kind, target, offset });
        };
        if bits & ROOT != 0 {
            push(kind::ROOT, 0, 0);
        }
        if named.contains(&src) {
            push(kind::NAMED, 0, 0);
        }
        if bits & super::MERGE != 0 {
            push(kind::SPLIT, 0, 0);
        }
        let hi = w.records_of(sid);
        let res = w.edges(Unit { s: sid, lo: 0, hi }, &mut |via, to| {
            if via == Via::Group {
                return;
            }
            let k = kind_of(w, via, to, bits);
            match to {
                To::Key(id) => push(k, id.0, 0),
                To::Section(t, at) => {
                    if !kept(t.0) {
                        return;
                    }
                    if split(t.0) {
                        push(k, t.0, at);
                    } else if t.0 != src || k == kind::LINK_ORDER {
                        push(k, t.0, 0);
                    }
                }
            }
        });
        if res.is_err() {
            push(kind::REFUSE, refuse::UNREADABLE, 0);
        }
    }
    // The ring: each member names the next of its group (members in file
    // order, the last naming the first).
    let mut seen = std::collections::BTreeSet::new();
    for &(_, start, len) in w.group_of {
        if !seen.insert(start) {
            continue;
        }
        let span = w.group_members.get(start as usize..(start + len) as usize).unwrap_or_default();
        for (i, &m) in span.iter().enumerate() {
            let next = span[(i + 1) % span.len()];
            if next != m {
                out.push(Stored { src: m.0, kind: kind::GROUP, target: next.0, offset: 0 });
            }
        }
    }
    // F3: an FDE whose initial location (the record's relocation at byte 8,
    // S-37) is named by a non-local, non-section symbol.
    for sr in &w.split.eh_of {
        let s = sr.section;
        if !kept(s.0) || r.sections[s].reloc.0 == 0 {
            continue;
        }
        let Some((file, _)) = r.sh_index(s) else { continue };
        let Some(f) = w.tables.get(file.0 as usize) else { continue };
        let (Some(obj), Some(table)) = (f.obj.as_ref(), f.table.as_ref()) else { continue };
        let mut buf = Vec::new();
        let Ok(recs) = super::read::records(w.inputs, file, obj, r.sections[s].reloc, &mut buf)
        else {
            out.push(Stored {
                src: s.0,
                kind: kind::REFUSE,
                target: refuse::UNREADABLE,
                offset: 0,
            });
            continue;
        };
        let pieces = w.split.eh_piece.get(sr.pieces.slots()).unwrap_or_default();
        let first = table.first_global().0;
        for i in 0..recs.len() {
            let Some(raw) = recs.get(i) else { continue };
            let Ok(off) = u32::try_from(raw.r_offset) else { continue };
            let Some(p) = w.split.eh_piece_at(s, off) else { continue };
            if p.within != 8 || pieces.get(p.ordinal as usize).is_none_or(|x| x.is_cie()) {
                continue;
            }
            let global = raw.r_sym >= first
                && table
                    .get(mortar_elf::SymbolIndex(raw.r_sym))
                    .is_ok_and(|y| y.sym_type() != mortar_elf::consts::STT_SECTION);
            if global {
                out.push(Stored {
                    src: s.0,
                    kind: kind::REFUSE,
                    target: refuse::FDE_GLOBAL,
                    offset: 0,
                });
            }
        }
    }
    // F6: a non-local symbol defined in a section dropped other than by
    // COMDAT election.
    for (file, f) in w.tables.iter().enumerate() {
        let Some(table) = f.table.as_ref() else { continue };
        let file = mortar_core::FileId(file as u32);
        for (index, sym) in table.globals() {
            let Ok(mortar_elf::SymbolSection::Section(i)) = table.section_of(index, &sym) else {
                continue;
            };
            let Some(sid) = r.section_id(file, i) else { continue };
            if let mortar_core::resolution::SectionState::Dropped(why) = r.sections[sid].state
                && why != mortar_core::resolution::DropReason::Comdat
            {
                out.push(Stored {
                    src: sid.0,
                    kind: kind::REFUSE,
                    target: refuse::DEF_DROPPED,
                    offset: 0,
                });
            }
        }
    }
    out.sort_unstable();
    out.dedup();
    out
}

/// Part 0's dump lines (tab-separated): `A` the process's arguments, `Q`
/// per key its `RESOLVED` row as the capture writes it (`def_kind`,
/// section or `-`, value), `W` the stored rows (source, kind, `K` key or
/// `S` section or `-` with the target number, offset; `u32::MAX` is -1).
fn stored_dump(w: &Walk<'_, '_, '_>, t: &crate::LiveInputs<'_, '_>, o: &mut String) {
    use mortar_core::resolution::Def;
    if let Ok(d) = std::env::current_dir() {
        let _ = writeln!(o, "C\t{}", d.display());
    }
    for a in std::env::args_os().skip(1) {
        let _ = writeln!(o, "A\t{}", a.to_string_lossy());
    }
    for (k, _) in w.inputs.symbols.iter() {
        // `mortar_incr::tables::resolved_fields` over `Assignment::def`.
        let def = t.assignment.def(w.r, k);
        let (kind, sec, value) = match def {
            Some(Def::Undefined { .. }) | None => (0, None, 0),
            Some(Def::Defined(e)) => {
                let tab = w.tables.get(e.file.0 as usize).and_then(|f| f.table.as_ref());
                match tab.and_then(|tb| tb.get(e.index).ok().map(|y| (tb, y))) {
                    Some((tb, y)) => match tb.section_of(e.index, &y) {
                        Ok(mortar_elf::SymbolSection::Section(i)) => {
                            match w.r.section_id(e.file, i) {
                                Some(s) => (1, Some(s.0), y.st_value),
                                None => (2, None, y.st_value),
                            }
                        }
                        Ok(mortar_elf::SymbolSection::Common) => (3, None, y.st_value),
                        _ => (2, None, y.st_value),
                    },
                    None => (2, None, 0),
                }
            }
            Some(Def::Common(_)) => (3, None, 0),
            Some(Def::Internal(_)) => (4, None, 0),
            Some(Def::Shared(_)) => (5, None, 0),
        };
        let sec = sec.map_or_else(|| "-".to_owned(), |s| s.to_string());
        let _ = writeln!(o, "Q\t{}\t{kind}\t{sec}\t{value}", k.0);
    }
    for row in stored_rows(w) {
        let tk = if is_key_kind(row.kind) {
            "K"
        } else if matches!(row.kind, kind::ROOT | kind::NAMED | kind::SPLIT | kind::REFUSE) {
            "-"
        } else {
            "S"
        };
        let off = if row.offset == WHOLE { -1 } else { i64::from(row.offset) };
        let _ = writeln!(o, "W\t{}\t{}\t{tk}\t{}\t{off}", row.src, row.kind, row.target);
    }
}
