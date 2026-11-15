//! The verifier's hash maps (WP-035): FNV-1a 64 instead of the standard
//! library's SipHash, whose per-process random keys buy nothing here (the
//! keys are the link's own names) and cost ~11% of a run on string keys
//! (WP-028d, "Item 4"). Deterministic; no map's iteration order decides
//! anything the verifier reports (`docs/AGENTS.md` rule 11: findings are
//! sorted, §7.1).

use std::hash::{BuildHasherDefault, Hasher};

/// FNV-1a, 64 bits.
#[derive(Clone, Copy, Debug)]
pub(crate) struct Fnv(u64);

impl Default for Fnv {
    fn default() -> Self {
        Self(0xcbf2_9ce4_8422_2325)
    }
}

impl Hasher for Fnv {
    fn write(&mut self, bytes: &[u8]) {
        for b in bytes {
            self.0 = (self.0 ^ u64::from(*b)).wrapping_mul(0x0100_0000_01b3);
        }
    }

    fn finish(&self) -> u64 {
        self.0
    }
}

pub(crate) type HashMap<K, V> = std::collections::HashMap<K, V, BuildHasherDefault<Fnv>>;
pub(crate) type HashSet<K> = std::collections::HashSet<K, BuildHasherDefault<Fnv>>;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fnv1a64_known_values() {
        let h = |s: &[u8]| {
            let mut f = Fnv::default();
            f.write(s);
            f.finish()
        };
        assert_eq!(h(b""), 0xcbf2_9ce4_8422_2325);
        assert_eq!(h(b"a"), 0xaf63_dc4c_8601_ec8c);
        let mut m: HashMap<String, u32> = HashMap::default();
        m.insert("x".into(), 1);
        assert_eq!(m.get("x"), Some(&1));
    }
}
