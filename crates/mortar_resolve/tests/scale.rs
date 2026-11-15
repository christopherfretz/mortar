//! Determinism (`spec/resolve.md` §12, §15.1 `s12_same_tables_under_seeds_and_threads`)
//! over generated corpora, and hostile inputs: many groups of one
//! signature in a file, empty groups, 10^6 keys, a chain of 10^5 members
//! in either archive order. The hostile tests print their timings.

mod support;

use std::time::Instant;

use mortar_core::par::{Pool, TEST_THREADS};
use mortar_core::{FileId, OptId, SymbolId};
use mortar_elf::Machine;
use mortar_elf::consts::GRP_COMDAT;
use mortar_resolve::options::{Entry, Options};
use mortar_resolve::view::View;
use mortar_resolve::{Resolved, gnu, live, resolve};
use support::Model;

fn opts() -> Options {
    Options { entry: Entry::None, warn_backrefs: true, warn_common: true, ..Options::default() }
}

/// Archives in `--start-group` regions and not, with chains, cycles,
/// commons, weak definitions and COMDAT groups.
fn corpus(seed: u64, members: usize) -> Model {
    let mut s = seed | 1;
    let mut rnd = move |n: u64| {
        s ^= s << 13;
        s ^= s >> 7;
        s ^= s << 17;
        s % n
    };
    let mut m = Model::new(Machine::X86_64);
    let main = m.obj("main.o");
    m.undef(main, "f0");
    m.undef(main, "f1");
    let mut ar = m.archive();
    let mut group = None;
    let mut groups = 0;
    for i in 0..members {
        if rnd(20) == 0 {
            ar = m.archive();
            group = if rnd(2) == 0 {
                groups += 1;
                Some(groups)
            } else {
                None
            };
            if rnd(3) == 0 {
                let o = m.obj(&format!("o{i}.o"));
                m.undef(o, &format!("f{}", rnd(members as u64)));
                m.weak(o, &format!("f{}", rnd(members as u64)));
                if let Some(g) = group {
                    m.set_group(o, g);
                }
            }
        }
        let f = m.member(ar, &format!("m{i}.o"));
        if let Some(g) = group {
            m.set_group(f, g);
        }
        match rnd(5) {
            0 => {
                m.weak(f, &format!("f{i}"));
            }
            1 => {
                m.common(f, &format!("f{i}"), 4, 4);
            }
            2 => {
                m.comdat_def(f, &format!("g{}", rnd(8)), &format!("t{}", rnd(8)));
                m.def(f, &format!("f{i}"));
            }
            _ => {
                m.def(f, &format!("f{i}"));
            }
        }
        if rnd(3) == 0 {
            m.data(f, &format!("d{}", rnd(members as u64)), 4);
        }
        for _ in 0..rnd(4) {
            let t = rnd(members as u64);
            match rnd(6) {
                0 => {
                    m.weak_undef(f, &format!("f{t}"));
                }
                1 => {
                    m.common(f, &format!("d{t}"), 8, 8);
                }
                2 => {
                    m.undef(f, &format!("t{}", rnd(8)));
                }
                _ => {
                    m.undef(f, &format!("f{t}"));
                }
            }
        }
    }
    m
}

fn check(m: &Model, o: &Options) -> Resolved {
    let reference = resolve(m, o, &Pool::sequential());
    for threads in TEST_THREADS {
        for seed in
            (0..10).map(|i| Some(0x9e37_79b9_7f4a_7c15_u64.wrapping_mul(i + 1))).chain([None])
        {
            let pool = Pool::sequential().with_threads(threads).with_seed(seed);
            assert_eq!(resolve(m, o, &pool), reference, "{threads} threads, seed {seed:?}");
        }
    }
    let roots: Vec<(SymbolId, u32)> = o.roots().iter().map(|r| (r.key, r.arg)).collect();
    assert_eq!(
        live::oracle(m, &reference.targets, &reference.redirect, &roots),
        reference.file_live
    );
    assert_eq!(gnu::naive(m, o, &reference), *reference.gnu.as_ref().unwrap());
    reference
}

#[test]
fn s12_same_tables_under_seeds_and_threads() {
    for seed in 1..9 {
        let m = corpus(seed * 0x2545_f491, 400);
        let r = check(&m, &opts());
        assert!(r.file_live.iter().filter(|l| l.is_live()).count() > 2);
    }
}

#[test]
fn hostile_many_groups_of_one_signature_in_a_file() {
    let mut m = Model::new(Machine::AArch64);
    let a = m.obj("a.o");
    for i in 0..2000 {
        m.comdat_def(a, "G", &format!("x{i}"));
    }
    let b = m.obj("b.o");
    m.comdat_def(b, "G", "x0");
    let t = Instant::now();
    let r = check(&m, &opts());
    println!("2000 groups of one signature: {:?} (17 schedules)", t.elapsed());
    let kept = r.sections.iter().filter(|s| s.comdat.is_some() && s.is_kept()).count();
    assert_eq!(kept, 1);
    assert_eq!(r.keepers.keeper[0], OptId::some(a));
}

#[test]
fn hostile_empty_group() {
    let mut m = Model::new(Machine::AArch64);
    let a = m.obj("a.o");
    m.group(a, "E", GRP_COMDAT, &[]);
    let b = m.obj("b.o");
    m.group(b, "E", GRP_COMDAT, &[]);
    m.group(b, "F", GRP_COMDAT, &[999]);
    let r = check(&m, &opts());
    assert!(r.problems.is_empty(), "{:?}", r.problems);
    assert_eq!(r.keepers.copies.len(), 3);
}

#[test]
fn hostile_million_keys() {
    // 10^6 keys, one definer each, all referenced from one object.
    let mut m = Model::new(Machine::X86_64);
    let user = m.obj("user.o");
    let names: Vec<String> = (0..1_000_000).map(|i| format!("k{i}")).collect();
    for n in &names {
        m.undef(user, n);
    }
    let ar = m.archive();
    for (c, chunk) in names.chunks(1000).enumerate() {
        let f = m.member(ar, &format!("m{c}.o"));
        for n in chunk {
            m.def(f, n);
        }
    }
    let o = opts();
    for threads in [1, 4] {
        let pool = Pool::new(threads);
        let t = Instant::now();
        let r = resolve(&m, &o, &pool);
        println!("10^6 keys, {threads} threads: {:?}", t.elapsed());
        assert_eq!(r.file_live.iter().filter(|l| l.is_live()).count(), 1002);
        assert!(r.def.iter().filter(|d| d.entry().is_some()).count() == 1_000_000);
        assert!(r.problems.is_empty());
    }
}

/// A chain of `n` members, each referencing the next, in ascending or
/// descending archive order.
fn chain(n: usize, descending: bool) -> Model {
    let mut m = Model::new(Machine::AArch64);
    let main = m.obj("main.o");
    m.undef(main, "c0");
    let ar = m.archive();
    let order: Vec<usize> = if descending { (0..n).rev().collect() } else { (0..n).collect() };
    for i in order {
        let f = m.member(ar, &format!("c{i}.o"));
        m.def(f, &format!("c{i}"));
        if i + 1 < n {
            m.undef(f, &format!("c{}", i + 1));
        }
    }
    m
}

#[test]
fn hostile_chain_depth() {
    let n = 100_000;
    for descending in [false, true] {
        let m = chain(n, descending);
        let o = opts();
        let t = Instant::now();
        let r = resolve(&m, &o, &Pool::new(4));
        println!(
            "chain of {n} members ({}): {:?}, {} rounds",
            if descending { "descending" } else { "ascending" },
            t.elapsed(),
            r.rounds
        );
        assert_eq!(r.rounds as usize, n);
        assert_eq!(r.file_live.iter().filter(|l| l.is_live()).count(), n + 2);
        let sim = r.gnu.as_ref().unwrap();
        assert_eq!(sim.loaded.len(), n);
        // Descending: GNU ld loads one member per pass, but loads them all.
        assert!(r.problems.is_empty(), "{:?}", &r.problems[..r.problems.len().min(3)]);
        let t = Instant::now();
        let oracle = live::oracle(&m, &r.targets, &r.redirect, &[]);
        println!("  sequential worklist: {:?}", t.elapsed());
        assert_eq!(oracle, r.file_live);
        let _ = FileId(0);
        let _ = m.files();
    }
    // A small descending chain against the full visit of every pass.
    let m = chain(300, true);
    check(&m, &opts());
}
