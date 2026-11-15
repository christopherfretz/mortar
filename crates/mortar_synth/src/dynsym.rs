//! `.dynsym`'s plan (`spec/dyn.md` D-21, D-22, D-26): which keys, in which
//! order, and the `.dynsym` index of each. Owned by WP-M2-DYNSYM; WP-041
//! fixed the signatures and left [`dynsym_plan`] a stub.
//!
//! `mortar_resolve` produces only the sets (`Resolution::{imported,
//! exported}`, D-21 items 1 and 2), the scan the `WeakDyn` keys
//! (`Demands::weak_dyn`, item 4) and `Slots` the copy allocations (item
//! 3); this plan is the one place that orders them. The relocation
//! writers take `r_sym` and D-40's sort key from [`DynsymPlan::index`].

use mortar_core::id::SymbolId;
use mortar_core::{Demands, Resolution, Slots};

/// The `.dynsym` of an output (D-21, D-22, D-26): the entries after the
/// null one, the unhashed ones first. Empty (no `.dynsym`) in a static
/// link.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct DynsymPlan {
    order: Box<[SymbolId]>,
    unhashed: u32,
    nbuckets: u32,
    /// `(key, index)` sorted by key.
    index: Box<[(SymbolId, u32)]>,
}

impl DynsymPlan {
    /// The plan of `order` (the entries after the null one, D-22's order),
    /// whose first `unhashed` entries are unhashed, with `nbuckets`
    /// `.gnu.hash` buckets (D-26). `None` when a key is given twice or
    /// `unhashed` exceeds the entries. Also the fixture constructor.
    #[must_use]
    pub fn new(order: Box<[SymbolId]>, unhashed: u32, nbuckets: u32) -> Option<Self> {
        if unhashed as usize > order.len() {
            return None;
        }
        let mut index: Vec<(SymbolId, u32)> =
            order.iter().enumerate().map(|(i, &k)| (k, i as u32 + 1)).collect();
        index.sort_unstable();
        if index.windows(2).any(|w| w[0].0 == w[1].0) {
            return None;
        }
        Some(Self { order, unhashed, nbuckets, index: index.into() })
    }

    /// The `.dynsym` index of `key` (1 for the first entry after the null
    /// one), if it is an entry.
    #[must_use]
    pub fn index(&self, key: SymbolId) -> Option<u32> {
        self.index.binary_search_by_key(&key, |row| row.0).ok().map(|at| self.index[at].1)
    }

    /// The entries after the null one, in `.dynsym` order (`Slots.dynsym`).
    #[must_use]
    pub fn order(&self) -> &[SymbolId] {
        &self.order
    }

    /// How many of [`Self::order`]'s entries are unhashed (D-22).
    #[must_use]
    pub fn unhashed(&self) -> u32 {
        self.unhashed
    }

    /// `.gnu.hash`'s `nbuckets` (D-26).
    #[must_use]
    pub fn nbuckets(&self) -> u32 {
        self.nbuckets
    }

    /// Whether the plan has no entry besides the null one.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.order.is_empty()
    }

    /// The keys with an index, in key order (for the checks and the map).
    pub fn keys(&self) -> impl Iterator<Item = SymbolId> + '_ {
        self.index.iter().map(|row| row.0)
    }
}

/// D-21's set in D-22's order with D-26's `nbuckets`, from the resolution's
/// sets, the scan's `WeakDyn` keys and the copy allocations of `slots`.
///
/// **Stub** (WP-041): returns the empty plan; WP-M2-DYNSYM fills it. It is
/// never called while dynamic output is refused (`mortar`'s
/// `dynamic_guard`).
#[must_use]
pub fn dynsym_plan(resolution: &Resolution, demands: &Demands, slots: &Slots) -> DynsymPlan {
    let _ = (resolution, demands, slots);
    DynsymPlan::default()
}

#[cfg(test)]
mod tests {
    use super::*;
    use mortar_core::id::Id;

    // dyn.md D-22: the index of each entry counts from 1 (0 is the null
    // entry); a fixture plan rejects a key given twice.
    #[test]
    fn a_fixture_plan_and_its_index() {
        let p = DynsymPlan::new(vec![SymbolId(7), SymbolId(2), SymbolId(5)].into(), 1, 1).unwrap();
        assert_eq!(
            (p.index(SymbolId(7)), p.index(SymbolId(5)), p.index(SymbolId(3))),
            (Some(1), Some(3), None)
        );
        assert_eq!(p.keys().map(Id::index).collect::<Vec<_>>(), [2, 5, 7]);
        assert!(DynsymPlan::new(vec![SymbolId(1), SymbolId(1)].into(), 0, 1).is_none());
        assert!(DynsymPlan::new(vec![SymbolId(1)].into(), 2, 1).is_none());
        let empty = dynsym_plan(&Resolution::empty(0, 0, 0), &Demands::empty(0), &Slots::default());
        assert!(empty.is_empty() && empty.unhashed() == 0 && empty.nbuckets() == 0);
    }
}
