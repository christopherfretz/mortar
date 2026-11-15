//! L0: the helpers of `spec/model.md` §6 (`mortar_core::par`): each one's
//! result equals its sequential meaning under the thread counts 1, 2, 7 and
//! 64 and several scheduling seeds (§11). Workloads are tiny: the host is
//! shared, and 64 threads mostly park.

use std::panic::{AssertUnwindSafe, catch_unwind};
use std::sync::Mutex;
use std::sync::atomic::{AtomicU32, AtomicU64, Ordering};

use mortar_core::par::{
    BadSeed, Collected, MERGE_BLOCK, Pool, SCAN_BLOCK, SORT_RUN, TEST_SEEDS, TEST_THREADS,
    block_count, blocks, check_schedules, parse_seed, split_offsets, split_ranges,
};

/// splitmix64, for generated inputs.
fn rng(seed: u64) -> impl FnMut() -> u64 {
    let mut state = seed;
    move || {
        state = state.wrapping_add(0x9e37_79b9_7f4a_7c15);
        let mut z = state;
        z = (z ^ (z >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
        z ^ (z >> 31)
    }
}

/// Every pool the tests run under, the sequential one first.
fn pools() -> Vec<Pool> {
    let mut all = vec![Pool::sequential()];
    for threads in TEST_THREADS {
        for seed in std::iter::once(None).chain(TEST_SEEDS.map(Some)) {
            let pool = Pool::sequential().with_threads(threads).with_seed(seed);
            if !pool.is_inline() {
                all.push(pool);
            }
        }
    }
    all
}

// ---- §6: the pool and the seed --------------------------------------------

/// §6: a pool's thread count is what it is given; one thread without a
/// seed is inline; a seed is set without the environment.
#[test]
fn s6_pool_threads_and_seed() {
    let pool = Pool::sequential();
    assert_eq!((pool.threads(), pool.seed(), pool.is_inline()), (1, None, true));
    let seeded = pool.with_seed(Some(9));
    assert_eq!((seeded.seed(), seeded.is_inline()), (Some(9), false));
    assert_eq!(pool.with_threads(7).threads(), 7);
    assert!(!pool.with_threads(2).is_inline());
    assert!(Pool::available() >= 1);
}

/// §6: `MORTAR_SCHED_SEED` is a decimal `u64`; anything else is named.
#[test]
fn s6_sched_seed_parses_decimal_u64() {
    assert_eq!(parse_seed("0"), Ok(0));
    assert_eq!(parse_seed("18446744073709551615"), Ok(u64::MAX));
    assert_eq!(parse_seed(" 42\n"), Ok(42));
    for bad in ["", "-1", "0x10", "18446744073709551616", "seed"] {
        let e: BadSeed = parse_seed(bad).unwrap_err();
        assert!(e.to_string().contains("MORTAR_SCHED_SEED"), "{e}");
    }
}

/// §6: with a seed, units are handed out in a permuted order, a different
/// one for different seeds; with one thread and no seed, in unit order.
#[test]
fn s6_sched_seed_permutes_the_unit_order() {
    let order = |pool: Pool| {
        let seen = Mutex::new(Vec::new());
        pool.for_each(50, |u| seen.lock().unwrap().push(u));
        seen.into_inner().unwrap()
    };
    let identity: Vec<usize> = (0..50).collect();
    assert_eq!(order(Pool::sequential()), identity);
    let a = order(Pool::sequential().with_seed(Some(1)));
    let b = order(Pool::sequential().with_seed(Some(2)));
    for perm in [&a, &b] {
        let mut sorted = perm.clone();
        sorted.sort_unstable();
        assert_eq!(sorted, identity, "a permutation");
        assert_ne!(*perm, identity);
    }
    assert_ne!(a, b);
    assert_eq!(a, order(Pool::sequential().with_seed(Some(1))), "a seed is a schedule");
}

/// §6, test support: `check_schedules` returns the sequential result and
/// fails a computation whose result follows the schedule.
#[test]
fn s6_check_schedules_catches_a_schedule_dependent_result() {
    assert_eq!(
        check_schedules(|pool| pool.map(10, |u| u * u)),
        (0..10).map(|u| u * u).collect::<Vec<_>>()
    );
    let caught = catch_unwind(|| {
        check_schedules(|pool| {
            // "First writer wins": rule 2 forbids it.
            let order = Mutex::new(Vec::new());
            pool.for_each(20, |u| order.lock().unwrap().push(u));
            order.into_inner().unwrap()
        })
    });
    let message = caught.unwrap_err();
    let text = message.downcast_ref::<String>().cloned().unwrap_or_default();
    assert!(text.contains("depends on the schedule"), "{text}");
}

// ---- §6 rule 1: disjoint slices -------------------------------------------

/// §6 rule 1: `split_ranges` gives the slices of ascending, disjoint ranges
/// (gaps allowed); `split_offsets` those of a prefix sum.
#[test]
fn s6_rule1_split_ranges_and_offsets() {
    let mut v: Vec<u32> = (0..10).collect();
    let parts = split_ranges(&mut v, &[0..2, 2..2, 5..9]);
    assert_eq!(
        parts.iter().map(|p| p.to_vec()).collect::<Vec<_>>(),
        [vec![0, 1], vec![], vec![5, 6, 7, 8]]
    );
    let mut v: Vec<u32> = (0..6).collect();
    for (i, part) in split_offsets(&mut v, &[0, 1, 1, 4, 6]).into_iter().enumerate() {
        for x in part {
            *x = i as u32;
        }
    }
    assert_eq!(v, [0, 2, 2, 2, 3, 3]);
    assert!(split_offsets(&mut v, &[0]).is_empty());
}

/// §6 rule 1: an overlapping, reversed or out-of-bounds range is a bug of
/// the caller's, and stops.
#[test]
#[allow(clippy::single_range_in_vec_init, clippy::reversed_empty_ranges)] // bad ranges are the inputs
fn s6_rule1_split_ranges_rejects_overlap_and_overrun() {
    let bad: [&[std::ops::Range<usize>]; 4] = [&[0..3, 2..4], &[3..2], &[0..11], &[4..6, 1..2]];
    for ranges in bad {
        let mut v = vec![0u8; 10];
        assert!(
            catch_unwind(AssertUnwindSafe(|| split_ranges(&mut v, ranges).len())).is_err(),
            "{ranges:?}"
        );
    }
}

/// §6 rule 1: `for_each_part` and `for_each_mut` hand each part to exactly
/// one worker, once, with its index.
#[test]
fn s6_rule1_disjoint_slices_written_once_each() {
    let offsets = [0, 3, 3, 10, 11, 40, 100];
    let got = check_schedules(|pool| {
        let mut column = vec![0u32; 100];
        pool.for_each_part(split_offsets(&mut column, &offsets), |unit, part| {
            for (i, x) in part.iter_mut().enumerate() {
                *x += (unit * 1000 + i) as u32 + 1;
            }
        });
        let mut flat = vec![0u64; 300];
        pool.for_each_mut(&mut flat, |i, x| *x += i as u64 * 3 + 1);
        (column, flat)
    });
    for unit in 0..offsets.len() - 1 {
        for (i, &x) in got.0[offsets[unit]..offsets[unit + 1]].iter().enumerate() {
            assert_eq!(x, (unit * 1000 + i) as u32 + 1);
        }
    }
    assert!(got.1.iter().enumerate().all(|(i, &x)| x == i as u64 * 3 + 1));
}

/// §6: `for_each` and `for_each_init` visit every unit exactly once; each
/// worker's state is made once and reused across its units.
#[test]
fn s6_for_each_visits_every_unit_once() {
    for pool in pools() {
        for units in [0, 1, 7, 64, 333] {
            let hits: Vec<AtomicU32> = (0..units).map(|_| AtomicU32::new(0)).collect();
            pool.for_each(units, |u| {
                hits[u].fetch_add(1, Ordering::Relaxed);
            });
            assert!(hits.iter().all(|h| h.load(Ordering::Relaxed) == 1), "{pool:?} {units}");
            let states = AtomicU32::new(0);
            pool.for_each_init(
                units,
                || {
                    states.fetch_add(1, Ordering::Relaxed);
                    Vec::<usize>::with_capacity(4)
                },
                |scratch, u| {
                    scratch.clear();
                    scratch.push(u);
                    hits[u].fetch_add(1, Ordering::Relaxed);
                },
            );
            assert!(hits.iter().all(|h| h.load(Ordering::Relaxed) == 2));
            assert!(
                states.load(Ordering::Relaxed) as usize <= pool.threads().min(units.max(1)),
                "{pool:?}"
            );
        }
    }
}

// ---- §6 rule 2: commutative updates ---------------------------------------

/// §6 rule 2: an atomic minimum, maximum and or over shared cells give the
/// sequential values whatever the schedule (the helpers need nothing more
/// than `std`'s atomics for it).
#[test]
fn s6_rule2_commutative_updates_are_schedule_independent() {
    check_schedules(|pool| {
        let min: Vec<AtomicU64> = (0..8).map(|_| AtomicU64::new(u64::MAX)).collect();
        let max: Vec<AtomicU64> = (0..8).map(|_| AtomicU64::new(0)).collect();
        let or: Vec<AtomicU64> = (0..8).map(|_| AtomicU64::new(0)).collect();
        pool.for_each(200, |u| {
            let cell = u * 7 % 8;
            min[cell].fetch_min(u as u64 * 31 % 97, Ordering::Relaxed);
            max[cell].fetch_max(u as u64 * 31 % 97, Ordering::Relaxed);
            or[cell].fetch_or(1 << (u % 64), Ordering::Relaxed);
        });
        let read = |v: Vec<AtomicU64>| v.into_iter().map(AtomicU64::into_inner).collect::<Vec<_>>();
        (read(min), read(max), read(or))
    });
}

// ---- §6 rule 3: ordered collection ----------------------------------------

/// §6 rule 3: `map` is `(0..units).map(f)`, in unit order.
#[test]
fn s6_rule3_ordered_collection_map() {
    for units in [0, 1, 5, 1000] {
        let got = check_schedules(|pool| pool.map(units, |u| (u, u.wrapping_mul(0x9e37))));
        assert_eq!(got, (0..units).map(|u| (u, u.wrapping_mul(0x9e37))).collect::<Vec<_>>());
    }
}

/// §6 rule 3: `map_collect` concatenates each unit's rows in unit order,
/// with the offsets of each unit's rows; units with no rows included.
#[test]
fn s6_rule3_ordered_collection_flat_with_offsets() {
    let rows = |u: usize, out: &mut Vec<(usize, usize)>| {
        for i in 0..(u * 7919) % 5 {
            out.push((u, i));
        }
    };
    for units in [0, 1, 3, 500] {
        let got: Collected<(usize, usize)> = check_schedules(|pool| pool.map_collect(units, rows));
        let mut items = Vec::new();
        let mut offsets = vec![0];
        for u in 0..units {
            rows(u, &mut items);
            offsets.push(items.len());
        }
        assert_eq!(got.items, items);
        assert_eq!(got.offsets, offsets);
        assert_eq!(got.units(), units);
        for u in 0..units {
            assert!(got.unit(u).iter().all(|&(owner, _)| owner == u));
        }
    }
}

/// §6 rule 3: `map_collect_vecs` gives one vector per unit, in unit order.
#[test]
fn s6_rule3_ordered_collection_per_unit_vectors() {
    let got = check_schedules(|pool| {
        pool.map_collect_vecs(40, |u, out| {
            out.extend((0..u % 4).map(|i| u * 10 + i));
        })
    });
    assert_eq!(got.len(), 40);
    for (u, v) in got.iter().enumerate() {
        assert_eq!(*v, (0..u % 4).map(|i| u * 10 + i).collect::<Vec<_>>());
    }
}

/// §6 rule 3: the exclusive prefix sum, across the block size of its
/// parallel form.
#[test]
fn s6_rule3_prefix_sum_equals_sequential() {
    let mut next = rng(3);
    for len in [0, 1, 17, SCAN_BLOCK, SCAN_BLOCK + 1, 3 * SCAN_BLOCK + 5] {
        let values: Vec<u64> = (0..len).map(|_| next() % 1000).collect();
        let mut expect = values.clone();
        let mut acc = 0;
        for x in &mut expect {
            let v = *x;
            *x = acc;
            acc += v;
        }
        // Few schedules for the long inputs: each costs a pass.
        for pool in [
            Pool::sequential(),
            Pool::sequential().with_threads(7),
            Pool::sequential().with_threads(2).with_seed(Some(5)),
        ] {
            let mut got = values.clone();
            assert_eq!(pool.prefix_sum(&mut got), acc, "{pool:?} {len}");
            assert_eq!(got, expect, "{pool:?} {len}");
        }
    }
}

// ---- §6 rule 4: sorting ----------------------------------------------------

/// §6 rule 4: the sort is `slice::sort_by_key`'s, stable: rows of equal
/// keys keep their input order. Inputs span the sequential threshold, one
/// run, many runs, and many merge blocks; keys with many ties.
#[test]
fn s6_rule4_sort_equals_the_sequential_stable_sort() {
    let mut next = rng(11);
    for (len, distinct) in [
        (0, 1),
        (1, 1),
        (100, 3),
        (SORT_RUN + 1, 10),
        (3 * MERGE_BLOCK + 17, 50),
        (64 * SORT_RUN + 999, 1 << 20),
        (100_000, 1),
    ] {
        // (key, input position): the position shows stability.
        let rows: Vec<(u64, u32)> = (0..len).map(|i| (next() % distinct, i as u32)).collect();
        let mut expect = rows.clone();
        expect.sort_by_key(|r| r.0);
        let pools: Vec<Pool> = if len > 10_000 {
            vec![
                Pool::sequential().with_threads(7),
                Pool::sequential().with_threads(64).with_seed(Some(2)),
            ]
        } else {
            pools()
        };
        for pool in pools {
            let mut got = rows.clone();
            pool.sort_by_key(&mut got, |r| r.0);
            assert!(got == expect, "{pool:?} {len}");
        }
    }
}

/// §6 rule 4: `sort_by` with a reversed order, already sorted and reversed
/// inputs.
#[test]
fn s6_rule4_sort_by_presorted_inputs() {
    let n = 5 * SORT_RUN + 3;
    for rows in [(0..n as u32).collect::<Vec<_>>(), (0..n as u32).rev().collect()] {
        for pool in [Pool::sequential(), Pool::sequential().with_threads(7)] {
            let mut got = rows.clone();
            pool.sort_by(&mut got, |a, b| b.cmp(a));
            assert!(got.windows(2).all(|w| w[0] > w[1]), "{pool:?}");
        }
    }
}

// ---- §6 rule 6: fixed-size blocks -----------------------------------------

/// §6 rule 6: blocks are a function of the length and the block size,
/// whatever the pool; `for_each_block` visits each once.
#[test]
fn s6_rule6_blocks_are_a_function_of_the_length() {
    assert_eq!(blocks(10, 4).collect::<Vec<_>>(), [0..4, 4..8, 8..10]);
    assert_eq!(blocks(8, 4).collect::<Vec<_>>(), [0..4, 4..8]);
    assert_eq!(blocks(0, 4).len(), 0);
    assert_eq!(block_count(10, 4), 3);
    let got = check_schedules(|pool| {
        let seen = Mutex::new(Vec::new());
        pool.for_each_block(1001, 64, |b, r| seen.lock().unwrap().push((b, r)));
        let mut seen = seen.into_inner().unwrap();
        seen.sort_by_key(|s| s.0);
        seen
    });
    assert_eq!(got, blocks(1001, 64).enumerate().collect::<Vec<_>>());
}

// ---- §6: panics --------------------------------------------------------------

/// §6: a panic in a worker reaches the caller with its message, every
/// thread is joined, and the pool works afterwards.
#[test]
fn s6_a_worker_panic_propagates_without_deadlock() {
    for pool in pools() {
        let caught = catch_unwind(AssertUnwindSafe(|| {
            pool.for_each(100, |u| assert!(u != 13, "unit 13 fails"));
        }));
        let payload = caught.expect_err("the panic propagates");
        let text = payload
            .downcast_ref::<String>()
            .cloned()
            .or_else(|| payload.downcast_ref::<&str>().map(|s| (*s).to_owned()))
            .unwrap_or_default();
        assert!(text.contains("unit 13 fails"), "{pool:?}: {text}");
        let caught = catch_unwind(AssertUnwindSafe(|| {
            pool.map(50, |u| if u == 49 { panic!("last") } else { u })
        }));
        assert!(caught.is_err());
        assert_eq!(pool.map(5, |u| u), [0, 1, 2, 3, 4], "{pool:?} works after a panic");
    }
}

// ---- §6: how many workers (WP-032g) ----------------------------------------

/// §6: a call site's cost estimate changes how many workers run, never a
/// result: every helper under every pool, with estimates from "nothing"
/// to "a second per unit", gives its sequential meaning.
#[test]
fn s6_unit_cost_changes_no_result() {
    let input: Vec<u64> = {
        let mut next = rng(7);
        (0..3000).map(|_| next() % 1000).collect()
    };
    for ns in [1, 200, 100_000, u32::MAX] {
        let got = check_schedules(|pool| {
            let pool = pool.unit_cost(ns);
            let squares = pool.map(input.len(), |u| input[u] * input[u]);
            let rows = pool.map_collect(input.len(), |u, out| {
                out.extend(std::iter::repeat_n(u as u64, (input[u] % 3) as usize));
            });
            let mut sums = input.clone();
            let total = pool.prefix_sum(&mut sums);
            let mut sorted = input.clone();
            pool.sort_by_key(&mut sorted, |&x| x / 7);
            let hits = AtomicU64::new(0);
            pool.for_each(input.len(), |u| {
                hits.fetch_add(input[u], Ordering::Relaxed);
            });
            let mut cells = vec![0u64; input.len()];
            pool.for_each_mut(&mut cells, |u, c| *c = input[u] + 1);
            (squares, rows, sums, total, sorted, hits.into_inner(), cells)
        });
        assert_eq!(got.4, {
            let mut s = input.clone();
            s.sort_by_key(|&x| x / 7);
            s
        });
    }
}

/// §6: without a seed, a region whose estimated work is below one
/// worker's share runs on the caller alone: nothing is spawned.
#[test]
fn s6_a_small_region_runs_on_the_caller() {
    let caller = std::thread::current().id();
    let pool = Pool::sequential().with_threads(8).unit_cost(10);
    let others = AtomicU32::new(0);
    pool.for_each(1000, |_| {
        if std::thread::current().id() != caller {
            others.fetch_add(1, Ordering::Relaxed);
        }
    });
    let ids = pool.map(1000, |_| std::thread::current().id());
    assert_eq!(others.into_inner(), 0);
    assert!(ids.iter().all(|&id| id == caller));
}

/// §6: a parallel call made from inside a worker of another (a nested
/// region) completes and gives its sequential meaning.
#[test]
fn s6_nested_regions() {
    let got = check_schedules(|pool| {
        pool.map(20, |u| pool.map(30, |v| (u * 30 + v) as u64).iter().sum::<u64>())
    });
    assert_eq!(got.len(), 20);
    assert_eq!(got[1], (30..60).sum::<u64>());
}
