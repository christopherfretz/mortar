//! Hand-run benchmark of the fixed hash against `std`'s `SipHash`
//! (`spec/landed/model/WP-020b.md`): `cargo run --release -p mortar_core
//! --example hash_bench [--vectors]`. One thread, a few seconds.

#![allow(clippy::cast_precision_loss)]

#[path = "../tests/intern_support/mod.rs"]
mod support;

use std::hash::{BuildHasher, RandomState};
use std::time::Instant;

use mortar_core::hash::{FixedState, hash_bytes, hash_u64};

/// The inputs of the frozen vectors of `tests/hash.rs`.
const VECTOR_INPUTS: [&[u8]; 14] = [
    b"",
    b"a",
    b"ab",
    b"abc",
    b"main",
    b"_start",
    b"12345678",
    b"0123456789abcdef",
    b"0123456789abcdefg",
    b"_ZNSt3__16vectorIiNS_9allocatorIiEEE9push_backEOi",
    b"_ZNSt3__112basic_stringIcNS_11char_traitsIcEENS_9allocatorIcEEE6appendEPKcm",
    b"_ZNSt3__112basic_stringIcNS_11char_traitsIcEENS_9allocatorIcEEE6appendEPKcmXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXX",
    b"foo@V1",
    b"\0\xff\x80\x7f",
];

fn median(mut v: Vec<f64>) -> f64 {
    v.sort_by(f64::total_cmp);
    v[v.len() / 2]
}

fn main() {
    if std::env::args().any(|a| a == "--vectors") {
        for input in VECTOR_INPUTS {
            println!("    (b{:?}, 0x{:016x}),", String::from_utf8_lossy(input), hash_bytes(input));
        }
        for x in [0u64, 1, 0xffff_ffff, u64::MAX] {
            println!("    ({x:#x}, 0x{:016x}),", hash_u64(x));
        }
        return;
    }
    let w = support::workload(3, 400, 600, 50_000, 25);
    let tabs = support::Strtabs::new(&w);
    let names: Vec<&[u8]> = (0..w.files.len()).flat_map(|f| tabs.names(f)).collect();
    let bytes: usize = names.iter().map(|n| n.len()).sum();
    println!("{} names, {:.0} bytes on average", names.len(), bytes as f64 / names.len() as f64);
    let sip = RandomState::new();
    let runs = |f: &dyn Fn() -> u64| {
        median(
            (0..9)
                .map(|_| {
                    let t = Instant::now();
                    std::hint::black_box(f());
                    t.elapsed().as_secs_f64()
                })
                .collect(),
        )
    };
    let fixed = runs(&|| names.iter().fold(0u64, |a, n| a ^ hash_bytes(n)));
    let fixed_hasher = runs(&|| names.iter().fold(0u64, |a, n| a ^ FixedState.hash_one(n)));
    let siphash = runs(&|| names.iter().fold(0u64, |a, n| a ^ sip.hash_one(n)));
    for (label, t) in [
        ("hash_bytes", fixed),
        ("FixedState::hash_one(&[u8])", fixed_hasher),
        ("SipHash-1-3 (std)", siphash),
    ] {
        println!(
            "{label:30} {:6.2} ns/name  {:6.2} GB/s",
            t * 1e9 / names.len() as f64,
            bytes as f64 / t / 1e9
        );
    }
    let ints: Vec<u64> = (0..1_000_000u64).collect();
    let fixed = runs(&|| ints.iter().fold(0u64, |a, &x| a ^ FixedState.hash_one(x)));
    let siphash = runs(&|| ints.iter().fold(0u64, |a, &x| a ^ sip.hash_one(x)));
    println!("u64 FixedState {:.2} ns, SipHash {:.2} ns", fixed * 1e3, siphash * 1e3);
}
