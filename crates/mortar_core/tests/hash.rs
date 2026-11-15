//! L0: the fixed hash of `spec/model.md` §6 rule 5 (`mortar_core::hash`):
//! frozen values, independence from the host, and spread over realistic
//! symbol names.

#![allow(clippy::cast_precision_loss)]

mod intern_support;

use std::hash::{BuildHasher, Hash, Hasher};

use mortar_core::hash::{FixedHasher, FixedState, HashMap, combine, hash_bytes, hash_u64};

/// §6 rule 5: the hash is fixed. These values were produced by this
/// implementation when it landed (WP-020b, `examples/hash_bench.rs
/// --vectors`) and may never change silently: shard assignments and link
/// states depend on them. Changing one is a change of `spec/model.md`.
#[test]
fn s6_rule5_frozen_byte_string_vectors() {
    let vectors: [(&[u8], u64); 14] = [
        (b"", 0x9322_8a4d_e0ee_c5a2),
        (b"a", 0xaced_1252_7fe5_bff8),
        (b"ab", 0xe9c2_8c29_6825_8c7d),
        (b"abc", 0x989b_4a20_9c10_11c9),
        (b"main", 0xe94f_7e8f_65d0_a5a2),
        (b"_start", 0xa905_8601_da6b_bdae),
        (b"12345678", 0xea51_b9e7_7dfd_55d1),
        (b"0123456789abcdef", 0x88de_385a_856c_fb95),
        (b"0123456789abcdefg", 0x14f3_7288_a5f8_073a),
        (b"_ZNSt3__16vectorIiNS_9allocatorIiEEE9push_backEOi", 0xe775_272a_ce13_1bfe),
        (b"_ZNSt3__112basic_stringIcNS_11char_traitsIcEENS_9allocatorIcEEE6appendEPKcm", 0x5100_bd65_1791_ff03),
        (
            b"_ZNSt3__112basic_stringIcNS_11char_traitsIcEENS_9allocatorIcEEE6appendEPKcmXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXX",
            0xd592_3a01_2db1_fc8a,
        ),
        (b"foo@V1", 0x7aa7_f3b3_6d9f_77e3),
        (b"\x00\xff\x80\x7f", 0x07d2_f360_dee8_7cf0),
    ];
    for (input, expect) in vectors {
        assert_eq!(hash_bytes(input), expect, "{:?}", String::from_utf8_lossy(input));
    }
}

/// §6 rule 5: the integer hash is fixed too.
#[test]
fn s6_rule5_frozen_integer_vectors() {
    let vectors = [
        (0u64, 0x4adc_8d1e_72de_28b1),
        (1, 0x2f7f_a5f1_fef7_0d30),
        (0xffff_ffff, 0x10fd_b152_d2f7_63f2),
        (u64::MAX, 0xc3a4_0af7_b980_09ed),
    ];
    for (x, expect) in vectors {
        assert_eq!(hash_u64(x), expect, "{x:#x}");
    }
}

/// §6 rule 5: a `HashMap` of the linker hashes the same on every run and
/// host: no per-process seed, integers by value (so `usize` and `u64` agree
/// on 32- and 64-bit hosts), every built hasher starting alike.
#[test]
#[allow(clippy::manual_hash_one)] // `Default` and `build_hasher` are what is tested
fn s6_rule5_fixed_state_has_no_seed() {
    let one = FixedState.hash_one(b"main".as_slice());
    assert_eq!(one, FixedState.hash_one(b"main".as_slice()));
    assert_eq!(FixedState.hash_one(5usize), FixedState.hash_one(5u64));
    assert_eq!(FixedState.hash_one(-1i32), FixedState.hash_one(u32::MAX));
    let mut a = FixedHasher::default();
    let mut b = FixedState.build_hasher();
    (1u8, 2u16, b"x".as_slice()).hash(&mut a);
    (1u8, 2u16, b"x".as_slice()).hash(&mut b);
    assert_eq!(a.finish(), b.finish());
    assert_ne!(FixedState.hash_one((1u32, 2u32)), FixedState.hash_one((2u32, 1u32)));
    let mut map: HashMap<&[u8], u32> = HashMap::default();
    map.insert(b"a", 1);
    assert_eq!(map.get(b"a".as_slice()), Some(&1));
}

/// §6 rule 5: every length's code path reads every byte: a change of any
/// byte changes the hash.
#[test]
fn s6_rule5_every_byte_counts() {
    for len in 0..=130usize {
        let base: Vec<u8> = (0..len).map(|i| (i * 31 + 7) as u8).collect();
        let h = hash_bytes(&base);
        for at in 0..len {
            let mut flipped = base.clone();
            flipped[at] ^= 1;
            assert_ne!(hash_bytes(&flipped), h, "len {len}, byte {at}");
        }
        let mut longer = base.clone();
        longer.push(0);
        assert_ne!(hash_bytes(&longer), h, "len {len} and a zero byte more");
    }
    assert_ne!(combine(1, 2), combine(2, 1));
}

/// χ² of `hashes` over 1024 buckets taken from bits `shift..shift + 10`.
fn chi2(hashes: &[u64], shift: u32) -> f64 {
    let mut buckets = [0u32; 1024];
    for h in hashes {
        buckets[((h >> shift) & 1023) as usize] += 1;
    }
    let expect = hashes.len() as f64 / 1024.0;
    buckets.iter().map(|&n| (f64::from(n) - expect).powi(2) / expect).sum()
}

/// §6 rule 5: the hash spreads realistic symbol names (mangled C++ names
/// with long common prefixes, differing in a few bytes) evenly over
/// buckets in its low bits (the tag), high bits (the interner's home cell,
/// the `HashMap`'s control byte) and middle, with no collision among 200k
/// distinct names. A χ² over 1023 degrees of freedom has mean 1023 and
/// standard deviation 45: the bound is seven deviations.
#[test]
fn s6_rule5_spread_over_mangled_names() {
    let w = intern_support::workload(1, 0, 0, 200_000, 0);
    let mut hashes: Vec<u64> = (0..w.names.len() as u32).map(|n| hash_bytes(w.name(n))).collect();
    for shift in [0, 10, 27, 32, 44, 54] {
        let x = chi2(&hashes, shift);
        assert!(x < 1340.0, "bits {shift}..{}: chi2 {x:.0}", shift + 10);
    }
    hashes.sort_unstable();
    hashes.dedup();
    assert_eq!(hashes.len(), w.names.len(), "no 64-bit collision");
}
