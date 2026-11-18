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
    root_keys: &[SymbolId],
    live: &[bool],
    frag: &[bool],
) {
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
        let _ =
            std::fs::write(dir.join(format!("{tag}.rows")), dump(w, &rows, root_keys, live, frag));
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
