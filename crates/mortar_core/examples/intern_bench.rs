//! Hand-run benchmark of §2.3's numbering (`spec/landed/model/WP-020b.md`):
//! `cargo run --release -p mortar_core --example intern_bench [files]`.
//! Short (seconds), at most 8 threads: the host is shared.

#![allow(clippy::cast_precision_loss, clippy::too_many_lines)]

#[path = "../tests/intern_support/mod.rs"]
mod support;

use std::collections::HashMap;
use std::hash::BuildHasher;
use std::time::Instant;

use mortar_core::SymbolId;
use mortar_core::hash::FixedState;
use mortar_core::intern::{Interner, Mention, SymName, number};
use mortar_core::par::Pool;

const RUNS: usize = 7;

fn median(mut v: Vec<f64>) -> f64 {
    v.sort_by(f64::total_cmp);
    v[v.len() / 2]
}

/// The baseline: one thread, one `HashMap` from name to id.
fn baseline<S: BuildHasher + Default>(w: &support::Strtabs) -> (Vec<u32>, usize) {
    let mut map: HashMap<&[u8], u32, S> = HashMap::default();
    let mut sym_map = Vec::new();
    for f in 0..w.files.len() {
        for name in w.names(f) {
            let next = map.len() as u32;
            sym_map.push(*map.entry(name).or_insert(next));
        }
    }
    (sym_map, map.len())
}

fn parse(name: &[u8]) -> SymName<'_> {
    SymName::parse(name)
}

fn plain(name: &[u8]) -> SymName<'_> {
    SymName::plain(name)
}

fn main() {
    let files: usize = std::env::args().nth(1).map_or(2500, |a| a.parse().expect("a file count"));
    let w = support::workload(7, files, 600, 150_000, 25);
    let mentions = w.mentions();
    let tabs = support::Strtabs::new(&w);
    let counts = w.counts();
    let avg_len =
        w.files.iter().flatten().map(|&n| w.name(n).len()).sum::<usize>() as f64 / mentions as f64;
    println!("{files} files, {mentions} mentions, average name {avg_len:.0} bytes");

    let mut distinct = 0;
    for (label, run) in [
        ("HashMap+SipHash, 1 thread", baseline::<std::hash::RandomState> as fn(&_) -> _),
        ("HashMap+FixedState, 1 thread", baseline::<FixedState>),
    ] {
        let times: Vec<f64> = (0..RUNS)
            .map(|_| {
                let t = Instant::now();
                let (m, d) = run(&tabs);
                distinct = d;
                std::hint::black_box(m);
                t.elapsed().as_secs_f64()
            })
            .collect();
        let t = median(times);
        println!("{label:32} {:7.1} ms  {:6.1} M mentions/s", t * 1e3, mentions as f64 / t / 1e6);
    }
    println!("{distinct} distinct keys");
    let (reference, _) = baseline::<FixedState>(&tabs);
    for (threads, label, key) in
        [(1, "parse", parse as fn(&[u8]) -> SymName<'_>), (1, "plain", plain), (8, "parse", parse)]
    {
        let pool = Pool::sequential().with_threads(threads);
        let times: Vec<f64> = (0..RUNS)
            .map(|_| {
                let t = Instant::now();
                let interner = Interner::with_capacity(distinct);
                pool.for_each_init(
                    w.files.len(),
                    || interner.inserter(),
                    |ins, f| {
                        for (k, name) in tabs.names(f).enumerate() {
                            let m = Mention { unit: f as u32, ordinal: k as u32 };
                            std::hint::black_box(ins.insert(key(name), m).unwrap());
                        }
                    },
                );
                std::hint::black_box(interner.freeze());
                t.elapsed().as_secs_f64()
            })
            .collect();
        let t = median(times);
        println!("interner inserts only ({label}), {threads} thread(s) {:7.1} ms", t * 1e3);
    }
    {
        let times: Vec<f64> = (0..RUNS)
            .map(|_| {
                let t = Instant::now();
                let mut acc = 0u64;
                for f in 0..w.files.len() {
                    for name in tabs.names(f) {
                        acc = acc.wrapping_add(mortar_core::intern::InternKey::hash(
                            &SymName::parse(name),
                        ));
                    }
                }
                std::hint::black_box(acc);
                t.elapsed().as_secs_f64()
            })
            .collect();
        println!("parse and hash every mention, 1 thread {:7.1} ms", median(times) * 1e3);
    }
    if std::env::var_os("CONTROL").is_some() {
        let pool = Pool::sequential().with_threads(8);
        let times: Vec<f64> = (0..RUNS)
            .map(|_| {
                let t = Instant::now();
                let tables: Vec<Interner<SymName<'_>>> =
                    (0..8).map(|_| Interner::with_capacity(distinct)).collect();
                let next = std::sync::atomic::AtomicUsize::new(0);
                let take = || &tables[next.fetch_add(1, std::sync::atomic::Ordering::Relaxed)];
                pool.for_each_init(
                    w.files.len(),
                    || take().inserter(),
                    |ins, f| {
                        for (k, name) in tabs.names(f).enumerate() {
                            let m = Mention { unit: f as u32, ordinal: k as u32 };
                            std::hint::black_box(ins.insert(SymName::parse(name), m).unwrap());
                        }
                    },
                );
                t.elapsed().as_secs_f64()
            })
            .collect();
        println!("control: private interner per thread, 8 threads {:7.1} ms", median(times) * 1e3);
    }
    for threads in [1, 2, 4, 8] {
        let pool = Pool::sequential().with_threads(threads);
        let mut bytes = 0;
        let times: Vec<f64> = (0..RUNS)
            .map(|_| {
                let t = Instant::now();
                let r = number::<_, SymbolId, _, _>(&pool, &counts, |f| {
                    tabs.names(f).map(SymName::parse)
                })
                .expect("fits");
                let e = t.elapsed().as_secs_f64();
                assert_eq!(r.map.len(), reference.len());
                assert!(r.map.iter().zip(&reference).all(|(a, &b)| a.0 == b));
                bytes = r.names.heap_bytes();
                e
            })
            .collect();
        let t = median(times);
        println!(
            "mortar number, {threads} thread(s)     {:7.1} ms  {:6.1} M mentions/s  {:.1} bytes/key",
            t * 1e3,
            mentions as f64 / t / 1e6,
            bytes as f64 / distinct as f64
        );
    }
}
