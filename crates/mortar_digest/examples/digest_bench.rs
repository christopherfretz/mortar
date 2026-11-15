//! Throughput of `mortar_digest`'s functions (`spec/image.md` I-74).
//!
//! `cargo run --release -p mortar_digest --example digest_bench [MiB] [threads…]`
//!
//! Generates `MiB` (default 1024) of splitmix64 bytes, then for each tree
//! function hashes them as `IMAGE_BLOCK` leaves on each thread count
//! (default 1 2 4 8), the workers taking disjoint leaves from an atomic
//! counter as the image's jobs do, and computes the root. Prints the time,
//! the aggregate and per-thread rates, and the load average; checks that
//! the id is the same at every thread count.

#![allow(clippy::cast_precision_loss)] // rates for printing

use std::sync::Mutex;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Instant;

use mortar_digest::{IMAGE_BLOCK, LeafColumn, LeafHasher, Root, TreeFn, leaf_count, leaf_range};

fn generate(n: usize) -> Vec<u8> {
    let mut state = 0x0123_4567_89AB_CDEFu64;
    let mut out = Vec::with_capacity(n);
    while out.len() < n {
        state = state.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut x = state;
        x = (x ^ (x >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        x = (x ^ (x >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        out.extend_from_slice(&(x ^ (x >> 31)).to_le_bytes());
    }
    out.truncate(n);
    out
}

fn load() -> String {
    std::fs::read_to_string("/proc/loadavg").map_or_else(
        |_| "?".to_owned(),
        |s| s.split_whitespace().take(3).collect::<Vec<_>>().join(" "),
    )
}

fn tree(f: TreeFn, data: &[u8], threads: usize) -> Root {
    let n = data.len() as u64;
    let column = Mutex::new(LeafColumn::new(f, n).expect("column"));
    let next = AtomicU64::new(0);
    std::thread::scope(|scope| {
        for _ in 0..threads {
            scope.spawn(|| {
                loop {
                    let i = next.fetch_add(1, Ordering::Relaxed);
                    if i >= leaf_count(n) {
                        break;
                    }
                    let range = leaf_range(n, i);
                    let mut hasher = LeafHasher::new(f);
                    hasher.update(&data[range.start as usize..range.end as usize]);
                    let leaf = hasher.finish();
                    column.lock().expect("column lock").set(i, &leaf).expect("leaf");
                }
            });
        }
    });
    column.into_inner().expect("column").root().expect("root")
}

fn main() {
    let mut args = std::env::args().skip(1);
    let mib: usize = args.next().map_or(1024, |a| a.parse().expect("MiB"));
    let mut counts: Vec<usize> = args.map(|a| a.parse().expect("threads")).collect();
    if counts.is_empty() {
        counts = vec![1, 2, 4, 8];
    }
    let data = generate(mib << 20);
    println!("digest_bench: {mib} MiB, leaves of {} KiB, load {}", IMAGE_BLOCK >> 10, load());
    println!("| function | threads | time | aggregate | per thread | load |");
    println!("|---|---|---|---|---|---|");
    for (name, f) in [
        ("L128 tree", TreeFn::L128),
        ("SHA-1 tree", TreeFn::Sha1),
        ("SHA-256 tree", TreeFn::Sha256),
    ] {
        let mut first: Option<Root> = None;
        for &t in &counts {
            // Best of three for the fast function; one run for the slow ones.
            let runs = if f == TreeFn::L128 { 3 } else { 1 };
            let mut best = f64::INFINITY;
            for _ in 0..runs {
                let start = Instant::now();
                let root = tree(f, &data, t);
                best = best.min(start.elapsed().as_secs_f64());
                match first {
                    None => first = Some(root),
                    Some(r) => assert_eq!(r, root, "{name}: the id differs at {t} threads"),
                }
            }
            let mbs = data.len() as f64 / best / 1e6;
            println!(
                "| {name} | {t} | {:.1} ms | {mbs:.0} MB/s | {:.0} MB/s | {} |",
                best * 1e3,
                mbs / t as f64,
                load()
            );
        }
    }
}
