//! The one hash of the linker (`spec/model.md` §6 rule 5): fixed, seedless,
//! 64 bits, the same on every host.
//!
//! [`hash_bytes`] follows the construction of wyhash (final version 4, by
//! Wang Yi, public domain): 16 bytes or fewer are read as two overlapping
//! pairs of 4-byte words; longer strings are folded 16 bytes at a time
//! (three independent lanes of 16 while 48 or more remain) with the
//! 128-bit multiply-fold `mix(a, b) = lo(a * b) ^ hi(a * b)`, and the last
//! 16 bytes, read overlapping, finish it. It is written from scratch here,
//! with a fixed seed of zero and wyhash's published default secret; it was
//! not checked bit for bit against the reference implementation, so the
//! frozen test vectors (`tests/hash.rs`) are this implementation's own.
//!
//! Words are read with `from_le_bytes`, and [`FixedHasher`] hashes integers
//! by value, never by their in-memory bytes: results do not depend on the
//! host's endianness or pointer width.
//!
//! **The values may never change silently**: shard assignments, and later
//! link states, depend on them. A change is a change of `spec/model.md`.

use std::hash::{BuildHasher, Hasher};

/// wyhash's default secret.
const P0: u64 = 0x2d35_8dcc_aa6c_78a5;
const P1: u64 = 0x8bb8_4b93_962e_acc9;
const P2: u64 = 0x4b33_a62e_d433_d4a3;
const P3: u64 = 0x4d5a_2da5_1de1_aa47;

/// The fixed seed, after wyhash's seed mixing (`seed ^= mix(seed ^ P0, P1)`
/// with seed 0).
const SEED: u64 = mix(P0, P1);

/// The full 128-bit product of `a` and `b`, as (low, high).
#[inline]
const fn mum(a: u64, b: u64) -> (u64, u64) {
    let r = (a as u128) * (b as u128);
    (r as u64, (r >> 64) as u64)
}

/// The multiply-fold: low and high halves of the product, xored.
#[inline]
const fn mix(a: u64, b: u64) -> u64 {
    let (lo, hi) = mum(a, b);
    lo ^ hi
}

#[inline]
fn r8(s: &[u8], at: usize) -> u64 {
    let mut w = [0; 8];
    w.copy_from_slice(&s[at..at + 8]);
    u64::from_le_bytes(w)
}

#[inline]
fn r4(s: &[u8], at: usize) -> u64 {
    let mut w = [0; 4];
    w.copy_from_slice(&s[at..at + 4]);
    u64::from(u32::from_le_bytes(w))
}

/// The hash of a byte string (a name).
#[inline]
#[allow(clippy::many_single_char_names)] // the names of the construction
pub fn hash_bytes(s: &[u8]) -> u64 {
    let len = s.len();
    let mut seed = SEED;
    let (a, b);
    if len <= 16 {
        if len >= 4 {
            let q = (len >> 3) << 2;
            a = (r4(s, 0) << 32) | r4(s, q);
            b = (r4(s, len - 4) << 32) | r4(s, len - 4 - q);
        } else if len > 0 {
            a = (u64::from(s[0]) << 16) | (u64::from(s[len >> 1]) << 8) | u64::from(s[len - 1]);
            b = 0;
        } else {
            a = 0;
            b = 0;
        }
    } else {
        let mut p = 0;
        let mut i = len;
        if i >= 48 {
            let (mut lane1, mut lane2) = (seed, seed);
            while i >= 48 {
                seed = mix(r8(s, p) ^ P1, r8(s, p + 8) ^ seed);
                lane1 = mix(r8(s, p + 16) ^ P2, r8(s, p + 24) ^ lane1);
                lane2 = mix(r8(s, p + 32) ^ P3, r8(s, p + 40) ^ lane2);
                p += 48;
                i -= 48;
            }
            seed ^= lane1 ^ lane2;
        }
        while i > 16 {
            seed = mix(r8(s, p) ^ P1, r8(s, p + 8) ^ seed);
            p += 16;
            i -= 16;
        }
        a = r8(s, len - 16);
        b = r8(s, len - 8);
    }
    let (lo, hi) = mum(a ^ P1, b ^ seed);
    mix(lo ^ P0 ^ len as u64, hi ^ P1)
}

/// The hash of an integer.
#[inline]
pub const fn hash_u64(x: u64) -> u64 {
    let (lo, hi) = mum(x ^ P1, SEED);
    mix(lo ^ P0 ^ 8, hi ^ P1)
}

/// Combines two hashes, order-sensitively: the hash of a pair.
#[inline]
pub const fn combine(a: u64, b: u64) -> u64 {
    mix(a ^ P2, b ^ P3)
}

/// A [`Hasher`] for ordinary `HashMap`s of the linker: fixed, seedless.
/// Bytes are folded through [`hash_bytes`], integers by value.
#[derive(Clone, Copy, Debug)]
pub struct FixedHasher {
    state: u64,
}

impl Default for FixedHasher {
    fn default() -> Self {
        Self { state: SEED }
    }
}

impl FixedHasher {
    #[inline]
    fn fold(&mut self, x: u64) {
        self.state = mix(self.state ^ x, P1);
    }
}

impl Hasher for FixedHasher {
    #[inline]
    fn finish(&self) -> u64 {
        self.state
    }
    #[inline]
    fn write(&mut self, bytes: &[u8]) {
        self.fold(hash_bytes(bytes));
    }
    #[inline]
    fn write_u8(&mut self, i: u8) {
        self.fold(u64::from(i));
    }
    #[inline]
    fn write_u16(&mut self, i: u16) {
        self.fold(u64::from(i));
    }
    #[inline]
    fn write_u32(&mut self, i: u32) {
        self.fold(u64::from(i));
    }
    #[inline]
    fn write_u64(&mut self, i: u64) {
        self.fold(i);
    }
    #[inline]
    fn write_u128(&mut self, i: u128) {
        self.fold(i as u64);
        self.fold((i >> 64) as u64);
    }
    #[inline]
    fn write_usize(&mut self, i: usize) {
        self.fold(i as u64);
    }
}

/// The [`BuildHasher`] of [`FixedHasher`]: every hasher it builds starts in
/// the same state.
#[derive(Clone, Copy, Debug, Default)]
pub struct FixedState;

impl BuildHasher for FixedState {
    type Hasher = FixedHasher;
    fn build_hasher(&self) -> FixedHasher {
        FixedHasher::default()
    }
}

/// A `HashMap` with the fixed hash. For lookup only: nothing observable may
/// follow its iteration order (§6 rule 5).
pub type HashMap<K, V> = std::collections::HashMap<K, V, FixedState>;

/// A `HashSet` with the fixed hash; as [`HashMap`].
pub type HashSet<K> = std::collections::HashSet<K, FixedState>;
