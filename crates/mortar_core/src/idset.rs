//! A set of ids as a bit set (`spec/model.md` §3.3: the dynamic columns of
//! `Resolution`, `spec/dyn.md` D-11, D-12, D-21).
//!
//! An empty set costs nothing: a static link keeps every such column
//! empty ([`IdSet::new`]), and `contains` answers `false` for any id
//! beyond the set's length, so a reader never needs to know whether the
//! column was filled.

use std::fmt;
use std::marker::PhantomData;

use crate::id::Id;

/// A set of ids of one space, as bits.
#[derive(Clone, PartialEq, Eq, Hash)]
pub struct IdSet<I> {
    words: Box<[u64]>,
    /// The number of ids the set can hold: ids `0..len`.
    len: u32,
    _id: PhantomData<fn() -> I>,
}

impl<I: Id> IdSet<I> {
    /// The empty set with no room: what a static link keeps.
    #[must_use]
    pub fn new() -> Self {
        Self { words: Box::new([]), len: 0, _id: PhantomData }
    }

    /// The empty set with room for ids `0..len`.
    #[must_use]
    pub fn with_len(len: u32) -> Self {
        Self { words: vec![0; (len as usize).div_ceil(64)].into(), len, _id: PhantomData }
    }

    /// The set of `ids`, with room for ids `0..len`; `None` when an id is
    /// not below `len`.
    #[must_use]
    pub fn from_ids(len: u32, ids: impl IntoIterator<Item = I>) -> Option<Self> {
        let mut set = Self::with_len(len);
        for id in ids {
            if !set.insert(id) && id.index() >= len {
                return None;
            }
        }
        Some(set)
    }

    /// The number of ids the set has room for.
    #[must_use]
    pub fn len(&self) -> u32 {
        self.len
    }

    /// Whether the set has no room at all (not merely no members:
    /// [`IdSet::count`]).
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.len == 0
    }

    /// Whether `id` is a member; `false` beyond the set's room.
    #[must_use]
    pub fn contains(&self, id: I) -> bool {
        let i = id.slot();
        self.words.get(i / 64).is_some_and(|w| w >> (i % 64) & 1 == 1)
    }

    /// Adds `id`; returns whether it was added (`false` when it already
    /// was a member, or is beyond the set's room).
    pub fn insert(&mut self, id: I) -> bool {
        let i = id.slot();
        if id.index() >= self.len {
            return false;
        }
        let bit = 1u64 << (i % 64);
        let word = &mut self.words[i / 64];
        let added = *word & bit == 0;
        *word |= bit;
        added
    }

    /// Removes `id`; returns whether it was a member.
    pub fn remove(&mut self, id: I) -> bool {
        let i = id.slot();
        let Some(word) = self.words.get_mut(i / 64) else { return false };
        let bit = 1u64 << (i % 64);
        let was = *word & bit != 0;
        *word &= !bit;
        was
    }

    /// The number of members.
    #[must_use]
    pub fn count(&self) -> u32 {
        self.words.iter().map(|w| w.count_ones()).sum()
    }

    /// The members, in increasing id order.
    pub fn iter(&self) -> impl Iterator<Item = I> + '_ {
        self.words.iter().enumerate().flat_map(|(n, &w)| {
            let base = n as u32 * 64;
            (0..64u32).filter(move |b| w >> b & 1 == 1).map(move |b| I::from_index(base + b))
        })
    }
}

impl<I: Id> Default for IdSet<I> {
    fn default() -> Self {
        Self::new()
    }
}

impl<I: Id> fmt::Debug for IdSet<I> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_set().entries(self.iter()).finish()
    }
}

#[cfg(test)]
mod tests {
    use super::IdSet;
    use crate::id::SymbolId;

    #[test]
    fn members_room_and_order() {
        let empty = IdSet::<SymbolId>::new();
        assert!(empty.is_empty() && !empty.contains(SymbolId(0)) && empty.count() == 0);
        let mut s = IdSet::with_len(130);
        assert!(s.insert(SymbolId(129)) && s.insert(SymbolId(3)) && !s.insert(SymbolId(3)));
        assert!(!s.insert(SymbolId(130)), "beyond the room");
        assert!(s.contains(SymbolId(129)) && !s.contains(SymbolId(64)));
        assert_eq!(s.iter().collect::<Vec<_>>(), [SymbolId(3), SymbolId(129)]);
        assert!(s.remove(SymbolId(3)) && !s.remove(SymbolId(3)) && s.count() == 1);
        assert_eq!(IdSet::from_ids(4, [SymbolId(1), SymbolId(1)]).map(|s| s.count()), Some(1));
        assert!(IdSet::from_ids(4, [SymbolId(4)]).is_none());
    }
}
