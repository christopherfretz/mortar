//! Scale (WP-024 extra test (iv)): a synthetic link of 10^6 input sections
//! with 10^5 distinct names, every stage timed at 1, 2, 4 and 8 threads,
//! and the same link at a tenth of the size to show that no stage grows
//! faster than linearly.
//!
//! Ignored by default (it is a measurement): run it with
//! `cargo test --release -p mortar_layout --test scale -- --ignored --nocapture`.
#![allow(clippy::cast_possible_truncation, clippy::cast_precision_loss)]

mod common;

use std::time::{Duration, Instant};

use common::{Sec, World, sec};
use mortar_core::par::Pool;
use mortar_elf::Machine;
use mortar_elf::consts::{SHF_ALLOC, SHF_EXECINSTR, SHF_WRITE, SHT_NOBITS, SHT_PROGBITS};
use mortar_layout::assign::{assign, finish};
use mortar_layout::chunk::{ChunkInputs, chunk, start_stop_outputs};
use mortar_layout::fixed::fixed_point;
use mortar_layout::order::order;
use mortar_layout::run::out_ranks;
use mortar_layout::stack::executable_stack;
use mortar_layout::symbols::{SymbolInputs, symbols};

/// `files` files of `per` sections; the names cycle through `names`
/// distinct ones (`.text.fN`, `.rodata.rN`, `.data.dN`, `.bss.bN`, and
/// one orphan name in a thousand).
fn world(files: usize, per: usize, names: usize) -> World {
    let leaked: Vec<&'static [u8]> = (0..names)
        .map(|k| {
            let s = match k % 1000 {
                0 => format!("orphan_{k}"),
                r if r % 4 == 0 => format!(".rodata.r{k}"),
                r if r % 4 == 1 => format!(".data.d{k}"),
                r if r % 4 == 2 => format!(".bss.b{k}"),
                _ => format!(".text.f{k}"),
            };
            &*Box::leak(s.into_bytes().into_boxed_slice())
        })
        .collect();
    let mut all: Vec<Vec<Sec>> = Vec::with_capacity(files);
    let mut k = 0usize;
    for _ in 0..files {
        let mut f = Vec::with_capacity(per);
        for _ in 0..per {
            let name = leaked[k % names];
            let (ty, flags) = if name.starts_with(b".text") {
                (SHT_PROGBITS, SHF_ALLOC | SHF_EXECINSTR)
            } else if name.starts_with(b".bss") {
                (SHT_NOBITS, SHF_ALLOC | SHF_WRITE)
            } else if name.starts_with(b".rodata") {
                (SHT_PROGBITS, SHF_ALLOC)
            } else {
                (SHT_PROGBITS, SHF_ALLOC | SHF_WRITE)
            };
            f.push(sec(name, ty, flags, 16 + (k as u64 % 97), (k % 5) as u8));
            k += 1;
        }
        all.push(f);
    }
    World::new(Machine::AArch64, &all, &[])
}

/// Each stage's wall time for one run.
fn stages(w: &World, pool: &Pool) -> [(&'static str, Duration); 7] {
    let t = Instant::now();
    let mut a = assign(&w.headers, &w.resolution, &w.symbols, &w.target, &w.options, pool)
        .unwrap()
        .assignment;
    let ta = t.elapsed();
    let t = Instant::now();
    let info = finish(&mut a, &w.headers, &w.liveness, &w.target).unwrap();
    let tfin = t.elapsed();
    let t = Instant::now();
    let ranks = out_ranks(&a);
    let ordered =
        order(&a, &w.headers, &w.resolution, &w.liveness, &w.folds, &ranks, pool).unwrap();
    let tb = t.elapsed();
    let t = Instant::now();
    let keep_empty = start_stop_outputs(&a, &w.resolution, &w.symbols);
    let (exec, _) = executable_stack(&w.stack_notes, w.options.execstack);
    let rules = w.options.segment_rules(&w.target, exec);
    let inputs = ChunkInputs {
        assignment: &a,
        info: &info,
        headers: &w.headers,
        ordered: &ordered,
        folds: &w.folds,
        split: &w.split,
        merge: &w.merge,
        liveness: &w.liveness,
        commons: &w.commons,
        synthetic: &w.synthetic,
        keep_empty: &keep_empty,
        relro: w.options.relro,
        now: w.options.now,
    };
    let staged = chunk(&inputs, &w.target, &rules, pool).unwrap();
    let tc = t.elapsed();
    let t = Instant::now();
    let d = mortar_layout::address::address(
        &staged,
        &vec![0; staged.islands.len()],
        &w.target,
        &w.options,
        &rules,
    )
    .unwrap();
    let td = t.elapsed();
    let t = Instant::now();
    let fixed = fixed_point(&staged, &w.target, &w.options, &rules, None, pool).unwrap();
    let te = t.elapsed();
    assert_eq!(fixed.addressed.chunks, d.chunks);
    let t = Instant::now();
    let cx = SymbolInputs {
        resolution: &w.resolution,
        assignment: &a,
        symbols: &w.symbols,
        chunks: &fixed.addressed.chunks,
        order: &staged.order,
        ranks: &staged.ranks,
        out_of: &staged.out_of,
        place: &staged.place,
        sym_place: &staged.sym_place,
    };
    let _ = symbols(cx, &w.target, &w.options, |_| None).unwrap();
    let tf = t.elapsed();
    [
        ("A assign", ta),
        ("A finish", tfin),
        ("B order", tb),
        ("C chunk", tc),
        ("D address", td),
        ("E fixed point", te),
        ("F symbols", tf),
    ]
}

/// The best of three runs, per stage.
fn best(w: &World, pool: &Pool) -> Vec<(&'static str, Duration)> {
    let mut out: Vec<(&'static str, Duration)> = stages(w, pool).to_vec();
    for _ in 0..2 {
        for (o, n) in out.iter_mut().zip(stages(w, pool)) {
            o.1 = o.1.min(n.1);
        }
    }
    out
}

fn ms(d: Duration) -> f64 {
    d.as_secs_f64() * 1e3
}

// spec: layout.md §19 L-140..L-145 (cost: no stage is quadratic)
#[test]
#[ignore = "a measurement; run with --release --ignored"]
fn scale_1e6_sections() {
    let big = world(10_000, 100, 100_000);
    let small = world(1_000, 100, 10_000);
    eprintln!(
        "{} input sections, {} names; small: {}",
        big.headers.len(),
        100_000,
        small.headers.len()
    );
    eprintln!(
        "stage          | 1 thread | 2 threads | 4 threads | 8 threads | 1/10 size (1 thread) | ratio"
    );
    let runs: Vec<Vec<(&str, Duration)>> =
        [1, 2, 4, 8].iter().map(|&t| best(&big, &Pool::sequential().with_threads(t))).collect();
    let tenth = best(&small, &Pool::sequential());
    for i in 0..runs[0].len() {
        let ratio = ms(runs[0][i].1) / ms(tenth[i].1).max(1e-3);
        eprintln!(
            "{:<14} | {:>8.2} | {:>9.2} | {:>9.2} | {:>9.2} | {:>20.3} | {ratio:.1}",
            runs[0][i].0,
            ms(runs[0][i].1),
            ms(runs[1][i].1),
            ms(runs[2][i].1),
            ms(runs[3][i].1),
            ms(tenth[i].1),
        );
        // 10x the input: a linear stage costs ~10x; a quadratic one 100x.
        assert!(
            ratio < 30.0 || ms(runs[0][i].1) < 5.0,
            "{} grows {ratio:.1}x for 10x the input",
            runs[0][i].0
        );
    }
}
