//! A hash index from a row's key to the row: the conversion from a name
//! to its `SymbolId` or `ComdatId` without sorting the table (the sorted
//! `mortar_core::identity::Lookup` it replaces here cost a sort of every
//! key, and a binary search of byte comparisons per lookup: WP-032a).
//!
//! Only lookups read it (nothing iterates it), so its layout, which
//! depends on the hashes, decides nothing (`docs/ARCHITECTURE.md`,
//! principle 6). Its answer is `Lookup::find`'s: the row whose key is the
//! one asked for, if exactly one row has it.

/// An empty slot.
const EMPTY: u32 = u32::MAX;

/// Open addressing over row numbers, at most half full, probed linearly
/// from the hash's high bits.
#[derive(Debug, Clone, Default)]
pub struct HashIndex {
    slots: Box<[u32]>,
    shift: u32,
}

impl HashIndex {
    /// The index of rows `0..hashes.len()`, row `r`'s key having the hash
    /// `hashes[r]`.
    #[must_use]
    pub fn new(hashes: &[u64]) -> Self {
        let n = hashes.len().min(EMPTY as usize - 1);
        let bits = (2 * n).max(16).next_power_of_two().trailing_zeros();
        let mut slots = vec![EMPTY; 1 << bits].into_boxed_slice();
        let shift = 64 - bits;
        let mask = slots.len() - 1;
        for (row, &h) in hashes.iter().enumerate().take(n) {
            let mut i = home(h, shift);
            while slots[i] != EMPTY {
                i = (i + 1) & mask;
            }
            slots[i] = row as u32;
        }
        Self { slots, shift }
    }

    /// The row whose key has hash `hash` and for which `is(row)` holds, if
    /// exactly one row does.
    #[inline]
    pub fn find(&self, hash: u64, mut is: impl FnMut(u32) -> bool) -> Option<u32> {
        if self.slots.is_empty() {
            return None;
        }
        let mask = self.slots.len() - 1;
        let mut i = home(hash, self.shift);
        let mut found = None;
        loop {
            let row = self.slots[i];
            if row == EMPTY {
                return found;
            }
            if is(row) {
                if found.is_some() {
                    return None;
                }
                found = Some(row);
            }
            i = (i + 1) & mask;
        }
    }
}

#[inline]
fn home(hash: u64, shift: u32) -> usize {
    (hash.wrapping_mul(0x9E37_79B9_7F4A_7C15) >> shift) as usize
}

#[cfg(test)]
mod tests {
    use super::HashIndex;

    #[test]
    fn finds_each_row_and_refuses_twins() {
        let keys: Vec<u64> = (0..1000u64).map(|k| k % 700).collect();
        let index = HashIndex::new(&keys);
        for want in 0..700u64 {
            let got = index.find(want, |r| keys[r as usize] == want);
            if want < 300 {
                assert_eq!(got, None, "two rows have {want}");
            } else {
                assert_eq!(got, Some(want as u32));
            }
        }
        assert_eq!(index.find(5000, |r| keys[r as usize] == 5000), None);
        assert_eq!(HashIndex::new(&[]).find(1, |_| true), None);
    }
}
