//! Anchoring (`meta.md` MT-12, `spec/odr.md` O-4): an anchored row (kind
//! 2 or 5, `name` 0) is named by the one symbol of its file whose FNV-1a 64
//! is the row's key. [`Anchors`] is built from a file's names (for kind 2
//! the symbols it defines, for kind 5 those it mentions; the caller picks)
//! and answers per key: one name, none (unanchored: renamed, stripped, or
//! inlined away), or several (two names of one key in one file: ambiguous,
//! never compared). A named row's key is an index too: whoever groups rows
//! confirms the name (MT-13).

use crate::fnv1a64;

/// What an anchored row's key names in its file.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Anchor<'a> {
    /// Exactly one name of the file has the key.
    Name(&'a [u8]),
    /// No name of the file has it (O-4: unanchored, not compared unless
    /// `--odr=strict`).
    Missing,
    /// Two or more different names of the file have it (O-4: ambiguous,
    /// not compared; `--odr-stats` counts it).
    Ambiguous,
}

/// The keys of one file's names, sorted by `(key, name)`, a name once.
#[derive(Debug, Clone, Default)]
pub struct Anchors<'a> {
    keys: Vec<(u64, &'a [u8])>,
}

impl<'a> Anchors<'a> {
    /// Hash every name (O-4's cost: once per file with a unit, only when a
    /// record-based check is on).
    pub fn new(names: impl IntoIterator<Item = &'a [u8]>) -> Self {
        let mut keys: Vec<(u64, &[u8])> = names.into_iter().map(|n| (fnv1a64(n), n)).collect();
        keys.sort_unstable();
        keys.dedup();
        Self { keys }
    }

    /// The name of `key` in this file.
    #[must_use]
    pub fn resolve(&self, key: u64) -> Anchor<'a> {
        let lo = self.keys.partition_point(|&(k, _)| k < key);
        match self.keys.get(lo..) {
            Some([(k, n), rest @ ..]) if *k == key => {
                if rest.first().is_some_and(|&(k2, _)| k2 == key) {
                    Anchor::Ambiguous
                } else {
                    Anchor::Name(n)
                }
            }
            _ => Anchor::Missing,
        }
    }

    /// The number of distinct names.
    #[must_use]
    pub fn len(&self) -> usize {
        self.keys.len()
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.keys.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // spec: odr.md O-4 (anchoring, ambiguity)
    #[test]
    fn resolve_one_none_two() {
        let names: [&[u8]; 3] = [b"_Z1fv", b"_Z1gv", b"_Z1fv"];
        let a = Anchors::new(names);
        assert_eq!(a.len(), 2);
        assert_eq!(a.resolve(fnv1a64(b"_Z1fv")), Anchor::Name(b"_Z1fv"));
        assert_eq!(a.resolve(fnv1a64(b"_Z1hv")), Anchor::Missing);
        // Two names with one key: construct by hand (no FNV-1a 64 collision
        // among short names is known to the test).
        let b = Anchors { keys: vec![(7, b"a".as_slice()), (7, b"b".as_slice()), (9, b"c")] };
        assert_eq!(b.resolve(7), Anchor::Ambiguous);
        assert_eq!(b.resolve(9), Anchor::Name(b"c"));
        assert_eq!(b.resolve(8), Anchor::Missing);
    }
}
