//! `L128`'s avalanche and collision sanity tests (`spec/image.md` I-114,
//! WP-026's E23). The quick variants run in the gate; the full ones are
//! `#[ignore]`d (`cargo test --release -p mortar_digest --test
//! l128_quality -- --ignored`).

#![allow(clippy::cast_precision_loss)] // counts far below 2^52

mod common;

use std::collections::HashSet;

use common::Rng;
use mortar_digest::l128;

/// For each length, `seeds` random inputs, and up to `max_bits` input bits
/// flipped one at a time (all of them when there are fewer, else a
/// random sample): per output bit, the fraction of flips that flipped it.
/// Returns (mean over the output bits, worst |p − 0.5|).
fn avalanche(lengths: &[usize], seeds: u64, max_bits: usize) -> (f64, f64) {
    let mut flips = [0u64; 128];
    let mut trials = 0u64;
    let mut rng = Rng(0xA7A1_5EED);
    for &n in lengths {
        for _ in 0..seeds {
            let mut d = rng.bytes(n);
            let base = u128::from_le_bytes(l128(&d));
            let bits: Vec<usize> = if 8 * n <= max_bits {
                (0..8 * n).collect()
            } else {
                (0..max_bits).map(|_| rng.below(8 * n as u64) as usize).collect()
            };
            for b in bits {
                d[b / 8] ^= 1 << (b % 8);
                let diff = base ^ u128::from_le_bytes(l128(&d));
                d[b / 8] ^= 1 << (b % 8);
                for (k, f) in flips.iter_mut().enumerate() {
                    *f += ((diff >> k) & 1) as u64;
                }
                trials += 1;
            }
        }
    }
    let p: Vec<f64> = flips.iter().map(|&f| f as f64 / trials as f64).collect();
    let mean = p.iter().sum::<f64>() / 128.0;
    let worst = p.iter().map(|x| (x - 0.5).abs()).fold(0.0, f64::max);
    eprintln!("avalanche: {trials} flips, mean {mean:.4}, worst |p - 0.5| {worst:.4}");
    (mean, worst)
}

/// The number of distinct values among `digests`, whole and truncated to
/// the low 32 bits.
fn distinct(digests: &[[u8; 16]]) -> (usize, usize) {
    let full: HashSet<[u8; 16]> = digests.iter().copied().collect();
    let low: HashSet<[u8; 4]> = digests.iter().map(|d| [d[0], d[1], d[2], d[3]]).collect();
    (full.len(), low.len())
}

/// Leaves of `size` zero bytes but a little-endian 4-byte counter at
/// `at`, for counters `0..count`.
fn counter_leaves(count: u32, size: usize, at: usize) -> Vec<[u8; 16]> {
    let mut d = vec![0u8; size];
    (0..count)
        .map(|c| {
            d[at..at + 4].copy_from_slice(&c.to_le_bytes());
            l128(&d)
        })
        .collect()
}

/// All zeros of each length `0..=max_len`, and every single nonzero byte
/// value at every position of a zero leaf of 64 bytes.
fn sparse_leaves(max_len: usize) -> Vec<[u8; 16]> {
    let zeros = vec![0u8; max_len];
    let mut out: Vec<[u8; 16]> = (0..=max_len).map(|n| l128(&zeros[..n])).collect();
    let mut d = [0u8; 64];
    for pos in 0..64 {
        for v in 1..=255u8 {
            d[pos] = v;
            out.push(l128(&d));
        }
        d[pos] = 0;
    }
    out
}

#[test]
fn avalanche_quick() {
    let (mean, worst) = avalanche(&[8, 31, 64, 100], 32, 128);
    assert!((mean - 0.5).abs() < 0.005, "mean {mean}");
    assert!(worst < 0.03, "worst {worst}");
}

#[test]
fn collisions_quick() {
    let c = counter_leaves(1 << 14, 1024, 512);
    assert_eq!(distinct(&c).0, c.len(), "counter leaves collide");
    let s = sparse_leaves(4096);
    assert_eq!(distinct(&s).0, s.len(), "sparse leaves collide");
}

/// E23's sizes: 64 seeds, lengths 8 to 4096, up to 512 bits each.
#[test]
#[ignore = "long: run with --release -- --ignored"]
fn avalanche_full() {
    let (mean, worst) = avalanche(&[8, 31, 64, 100, 4096], 64, 512);
    assert!((mean - 0.5).abs() < 0.002, "mean {mean}");
    assert!(worst < 0.01, "worst {worst}");
}

/// E23's sizes: 2^20 structured 4 KiB leaves (low 32 bits collide about
/// 128 ± 11 times for random values), and the sparse leaves up to 64 KiB.
#[test]
#[ignore = "long: run with --release -- --ignored"]
fn collisions_full() {
    let c = counter_leaves(1 << 20, 4096, 2048);
    let (full, low) = distinct(&c);
    let low_collisions = c.len() - low;
    eprintln!("counter leaves: {} low-32 collisions, {} full", low_collisions, c.len() - full);
    assert_eq!(full, c.len(), "counter leaves collide");
    assert!((60..=200).contains(&low_collisions), "{low_collisions} low-32-bit collisions");
    let s = sparse_leaves(65_536);
    eprintln!("sparse leaves: {}", s.len());
    assert_eq!(distinct(&s).0, s.len(), "sparse leaves collide");
}
