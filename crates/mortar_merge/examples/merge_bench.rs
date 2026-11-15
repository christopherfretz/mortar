//! Hand-run benchmark of the split and merge (`spec/synth.md` S-28, S-30;
//! `spec/landed/synth/WP-025m.md`): `cargo run --release -p mortar_merge
//! --example merge_bench [workload-dir]`. Times `merge` (cut, hash,
//! partition, intern, number, resolve) and `order::offsets` at 1, 2, 4 and
//! 8 threads on a synthetic input of 10^7 string pieces and on the
//! `.debug_str` sections of a real link's inputs (the `cxx-medium-g`
//! workload: its main object and every member of `libprotoc.a` and
//! `libprotobuf.a`). Prints the peak resident set beyond the input
//! (`VmHWM` after resetting it through `/proc/self/clear_refs`) and the
//! load average: the host is shared, so times are upper bounds.

#![allow(clippy::cast_precision_loss, clippy::cast_possible_truncation)]

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use mortar_core::par::Pool;
use mortar_core::{Column, FileId, OutSecId, SectionId};
use mortar_elf::Object;
use mortar_merge::order::offsets;
use mortar_merge::{MergeIn, merge};

const THREADS: [usize; 4] = [1, 2, 4, 8];
const REPEATS: usize = 5;

fn load_average() -> String {
    std::fs::read_to_string("/proc/loadavg").map_or_else(
        |_| "unknown".into(),
        |s| s.split_whitespace().take(3).collect::<Vec<_>>().join(" "),
    )
}

/// A `/proc/self/status` field in kB.
fn status_kb(field: &str) -> u64 {
    std::fs::read_to_string("/proc/self/status")
        .ok()
        .and_then(|s| {
            s.lines()
                .find(|l| l.starts_with(field))
                .and_then(|l| l.split_whitespace().nth(1)?.parse().ok())
        })
        .unwrap_or(0)
}

/// Resets the peak resident set to the current one.
fn reset_peak() {
    let _ = std::fs::write("/proc/self/clear_refs", "5");
}

fn splitmix(mut x: u64) -> u64 {
    x = x.wrapping_add(0x9e37_79b9_7f4a_7c15);
    x = (x ^ (x >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
    x = (x ^ (x >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
    x ^ (x >> 31)
}

/// 10^7 strings in 10^4 sections of 10^3: about 2.45 × 10^6 distinct,
/// 9 to 98 bytes long (S-30's synthetic input).
fn synthetic() -> Vec<Vec<u8>> {
    const DISTINCT: u64 = 2_450_000;
    let mut sections = Vec::with_capacity(10_000);
    let mut i = 0u64;
    for _ in 0..10_000 {
        let mut b = Vec::with_capacity(56_000);
        for _ in 0..1000 {
            let k = splitmix(i) % DISTINCT;
            i += 1;
            b.extend_from_slice(format!("s{k:07x}").as_bytes());
            let pad = (splitmix(k) % 90) as usize;
            b.extend(std::iter::repeat_n(b'a' + (k % 26) as u8, pad));
            b.push(0);
        }
        sections.push(b);
    }
    sections
}

/// The `.debug_str` of the workload's main object and archive members.
fn debug_str(dir: &Path) -> Result<Vec<Vec<u8>>, String> {
    let build = dir.join("build");
    let text = std::fs::read_to_string(dir.join("response.txt")).map_err(|e| e.to_string())?;
    let main = text
        .split_whitespace()
        .find(|w| w.ends_with("main.cc.o"))
        .ok_or("no main object in response.txt")?;
    let mut files: Vec<Vec<u8>> =
        vec![std::fs::read(build.join(main)).map_err(|e| format!("{main}: {e}"))?];
    for a in ["libprotoc.a", "libprotobuf.a"] {
        let bytes = std::fs::read(build.join(a)).map_err(|e| format!("{a}: {e}"))?;
        let ar = mortar_elf::archive::Archive::parse(&bytes).map_err(|e| e.to_string())?;
        for m in ar.members() {
            let m = m.map_err(|e| e.to_string())?;
            if let Some(d) = m.data().filter(|d| d.starts_with(b"\x7fELF")) {
                files.push(d.to_vec());
            }
        }
    }
    let mut out = Vec::new();
    for f in &files {
        let o = Object::parse(f).map_err(|e| e.to_string())?;
        for s in o.sections() {
            if s.name().is_ok_and(|n| n == b".debug_str") {
                out.push(s.data().map_err(|e| e.to_string())?.to_vec());
            }
        }
    }
    Ok(out)
}

fn ms(d: Duration) -> f64 {
    d.as_secs_f64() * 1e3
}

fn bench(what: &str, sections: &[Vec<u8>]) {
    let ins: Vec<MergeIn<'_>> = sections
        .iter()
        .enumerate()
        .map(|(i, b)| MergeIn {
            section: SectionId(i as u32),
            file: FileId(i as u32),
            index: 1,
            out: OutSecId(0),
            strings: true,
            entsize: 1,
            align: 0,
            bytes: b,
        })
        .collect();
    let bytes: usize = sections.iter().map(Vec::len).sum();
    let reference = merge(&Pool::sequential(), &ins).expect("no refusal");
    let pieces = reference.piece_frag.len();
    let frags = reference.frags.len();
    let meta = reference.meta.clone();
    let split = reference.into_split((Box::new([]), Box::new([]), Box::new([])));
    let live = Column::filled(frags, true);
    let out = offsets(&Pool::sequential(), &split, &meta, &live);
    println!(
        "{what}: {} sections, {pieces} pieces, {:.1} MB, {frags} fragments, {:.2} MB out",
        sections.len(),
        bytes as f64 / 1e6,
        out.chunks[0].size as f64 / 1e6
    );
    drop(out);
    for threads in THREADS {
        let pool = Pool::new(threads);
        let mut merges = Vec::with_capacity(REPEATS);
        let mut orders = Vec::with_capacity(REPEATS);
        let mut peak = 0u64;
        for _ in 0..REPEATS {
            let base = status_kb("VmRSS:");
            reset_peak();
            let t = Instant::now();
            let m = merge(&pool, &ins).expect("no refusal");
            merges.push(t.elapsed());
            assert_eq!(m.piece_frag, split.piece_frag, "{threads} threads");
            let meta = m.meta.clone();
            let s = m.into_split((Box::new([]), Box::new([]), Box::new([])));
            let t = Instant::now();
            let o = offsets(&pool, &s, &meta, &live);
            orders.push(t.elapsed());
            peak = peak.max(status_kb("VmHWM:").saturating_sub(base));
            drop((o, s));
        }
        merges.sort();
        orders.sort();
        println!(
            "  {threads} threads: merge median {:.0} ms (min {:.0}), offsets median {:.0} ms, peak +{:.0} MB, load {}",
            ms(merges[REPEATS / 2]),
            ms(merges[0]),
            ms(orders[REPEATS / 2]),
            peak as f64 / 1e3,
            load_average()
        );
    }
}

fn main() {
    let dir = std::env::args().nth(1).map_or_else(
        || {
            PathBuf::from(env!("CARGO_MANIFEST_DIR"))
                .join("../../../../../.build/bench-work/workloads/cxx-medium-g")
        },
        PathBuf::from,
    );
    println!("load average at start: {}", load_average());
    match debug_str(&dir) {
        Ok(s) => bench(&format!("{} .debug_str", dir.display()), &s),
        Err(e) => println!("{}: not run: {e}", dir.display()),
    }
    let s = synthetic();
    bench("synthetic 10^7 strings", &s);
    println!("load average at end: {}", load_average());
}
