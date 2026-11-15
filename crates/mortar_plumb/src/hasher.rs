//! The verifier's hash maps (WP-035): a fixed multiply-rotate hasher over
//! eight-byte words instead of the standard library's `SipHash`, whose
//! per-process random keys buy nothing here (the keys are the link's own
//! names). Measured on static protoc (`spec/landed/harness/WP-035.md`):
//! user time 0.20 → 0.18 s; byte-at-a-time FNV-1a was slower than
//! `SipHash` on long C++ names (0.23 s). Deterministic; no map's iteration
//! order decides anything the verifier reports (`docs/AGENTS.md` rule 11:
//! findings are sorted, §7.1).

use std::hash::{BuildHasherDefault, Hasher};

const K: u64 = 0x517c_c1b7_2722_0a95;

/// The hasher: per word, rotate left 5, xor, multiply by [`K`].
#[derive(Clone, Copy, Debug, Default)]
pub(crate) struct Words(u64);

impl Hasher for Words {
    fn write(&mut self, bytes: &[u8]) {
        let mut chunks = bytes.chunks_exact(8);
        for c in &mut chunks {
            let w = u64::from_le_bytes([c[0], c[1], c[2], c[3], c[4], c[5], c[6], c[7]]);
            self.0 = (self.0.rotate_left(5) ^ w).wrapping_mul(K);
        }
        for b in chunks.remainder() {
            self.0 = (self.0.rotate_left(5) ^ u64::from(*b)).wrapping_mul(K);
        }
    }

    fn finish(&self) -> u64 {
        self.0
    }
}

pub(crate) type HashMap<K, V> = std::collections::HashMap<K, V, BuildHasherDefault<Words>>;
pub(crate) type HashSet<K> = std::collections::HashSet<K, BuildHasherDefault<Words>>;

#[cfg(test)]
mod tests {
    use super::*;

    // Deterministic (no per-process key), and every byte counts.
    #[test]
    fn the_hasher_is_fixed() {
        let h = |s: &[u8]| {
            let mut f = Words::default();
            f.write(s);
            f.finish()
        };
        assert_eq!(h(b""), 0);
        assert_eq!(h(b"a"), 0x61u64.wrapping_mul(K));
        assert_ne!(h(b"_ZN6google8protobuf1aE"), h(b"_ZN6google8protobuf1bE"));
        assert_ne!(h(b"abcdefgh1"), h(b"abcdefgh2"));
        let mut m: HashMap<String, u32> = HashMap::default();
        m.insert("x".into(), 1);
        assert_eq!(m.get("x"), Some(&1));
        let s: HashSet<&str> = ["a", "b", "a"].into_iter().collect();
        assert_eq!(s.len(), 2);
    }
}
