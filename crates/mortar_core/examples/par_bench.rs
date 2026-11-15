//! Hand-run benchmark of the parallel helpers (`spec/landed/model/WP-020b.md`):
//! `cargo run --release -p mortar_core --example par_bench`. Seconds, at
//! most 8 threads.

#![allow(clippy::cast_precision_loss)]

use std::time::Instant;

use mortar_core::par::Pool;

fn median(mut v: Vec<f64>) -> f64 {
    v.sort_by(f64::total_cmp);
    v[v.len() / 2]
}

fn time(runs: usize, mut f: impl FnMut()) -> f64 {
    median(
        (0..runs)
            .map(|_| {
                let t = Instant::now();
                f();
                t.elapsed().as_secs_f64()
            })
            .collect(),
    )
}

fn main() {
    for threads in [2, 4, 8] {
        let pool = Pool::sequential().with_threads(threads);
        let t = time(201, || {
            pool.for_each(threads, |u| {
                std::hint::black_box(u);
            });
        });
        println!(
            "for_each of {threads} empty units on {threads} threads: {:6.1} us per call",
            t * 1e6
        );
    }
    let mut state = 1u64;
    let rows: Vec<(u64, u32)> = (0..4_000_000u32)
        .map(|i| {
            state = state
                .wrapping_mul(6_364_136_223_846_793_005)
                .wrapping_add(1_442_695_040_888_963_407);
            (state >> 40, i)
        })
        .collect();
    let t_std = time(5, || {
        let mut v = rows.clone();
        v.sort_by_key(|r| r.0);
        std::hint::black_box(v);
    });
    println!("4M rows: slice::sort_by_key {:6.1} ms (clone included)", t_std * 1e3);
    for threads in [1, 2, 4, 8] {
        let pool = Pool::sequential().with_threads(threads);
        let t = time(5, || {
            let mut v = rows.clone();
            pool.sort_by_key(&mut v, |r| r.0);
            std::hint::black_box(v);
        });
        println!("4M rows: Pool::sort_by_key, {threads} thread(s) {:6.1} ms", t * 1e3);
    }
    for threads in [1, 8] {
        let pool = Pool::sequential().with_threads(threads);
        let t = time(5, || {
            let c = pool.map_collect(10_000, |u, out: &mut Vec<u32>| {
                out.extend((0..400).map(|i| (u * i) as u32));
            });
            std::hint::black_box(c);
        });
        println!("map_collect, 10k units of 400 rows, {threads} thread(s) {:6.1} ms", t * 1e3);
    }
}
