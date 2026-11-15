//! The record index (`spec/image.md` I-29; model §4.5, a phase cache):
//! per relocated section, whether its relocation records are in `r_offset`
//! order, the permutation of those that are not, and whether one of its
//! records leads a sequence (`spec/arch.md` §3.5, `RelocAt.leads`).
//!
//! The scan (phase 70) writes it while it reads every record; fill
//! (phase 100) reads it to find a clip's records by binary search. It is a
//! `mortar_core` type so that the image need not depend on the scan
//! (`docs/ARCHITECTURE.md`, the crate table).

use crate::SectionId;

/// The order of a relocation section's records by `r_offset` (I-29).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Order<'a> {
    /// `r_offset` never decreases.
    Sorted,
    /// The record indexes stably sorted by `r_offset`.
    Permuted(&'a [u32]),
}

/// The record index (I-29), keyed by the *relocated* section. Kept sparse:
/// a section not listed is `Sorted`, and leads nothing.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct RecIndex {
    /// Sorted by section.
    permuted: Box<[(SectionId, Box<[u32]>)]>,
    /// The sections with a record whose type is in `Facts.seq_leads`,
    /// sorted (`spec/arch.md` §3.5: fill's `RelocAt.leads`).
    leads: Box<[SectionId]>,
    /// The sections whose records the scan did not read (`spec/image.md`
    /// I-14, WP-032c): fill checks them as it applies them, and finds
    /// their order itself. Sorted.
    deferred: Box<[SectionId]>,
}

impl RecIndex {
    /// The index of `permuted` (each section's record indexes stably
    /// sorted by `r_offset`) and `leads` (the sections with a lead record),
    /// in any order. A section given twice keeps its first permutation.
    #[must_use]
    pub fn new(mut permuted: Vec<(SectionId, Box<[u32]>)>, mut leads: Vec<SectionId>) -> Self {
        permuted.sort_by_key(|p| p.0);
        permuted.dedup_by_key(|p| p.0);
        leads.sort_unstable();
        leads.dedup();
        Self {
            permuted: permuted.into_boxed_slice(),
            leads: leads.into_boxed_slice(),
            deferred: Box::new([]),
        }
    }

    /// The index with `deferred` (in any order) as the sections the scan
    /// left to fill (I-14).
    #[must_use]
    pub fn with_deferred(mut self, mut deferred: Vec<SectionId>) -> Self {
        deferred.sort_unstable();
        deferred.dedup();
        self.deferred = deferred.into_boxed_slice();
        self
    }

    /// Whether the scan left `section`'s records to fill (I-14): their
    /// order and `leads` bit are then not in the index.
    #[must_use]
    pub fn is_deferred(&self, section: SectionId) -> bool {
        self.deferred.binary_search(&section).is_ok()
    }

    /// The sections the scan left to fill, in id order.
    #[must_use]
    pub fn deferred(&self) -> &[SectionId] {
        &self.deferred
    }

    /// The order of the records of `section`'s relocation section.
    #[must_use]
    pub fn get(&self, section: SectionId) -> Order<'_> {
        match self.permuted.binary_search_by_key(&section, |p| p.0) {
            Ok(at) => Order::Permuted(&self.permuted[at].1),
            Err(_) => Order::Sorted,
        }
    }

    /// `RelocAt.leads` for `section`'s records (`spec/arch.md` §3.5):
    /// whether one has a type in `Facts.seq_leads`. Exact for every
    /// section the scan read: the live ones, which are all fill reads.
    #[must_use]
    pub fn leads(&self, section: SectionId) -> bool {
        self.leads.binary_search(&section).is_ok()
    }

    /// The sections whose records are not sorted, in id order.
    pub fn unsorted(&self) -> impl ExactSizeIterator<Item = SectionId> + '_ {
        self.permuted.iter().map(|p| p.0)
    }

    /// The bytes the index holds (model §9): 4 per record of an unsorted
    /// section, plus the rows.
    #[must_use]
    pub fn heap_bytes(&self) -> usize {
        self.permuted
            .iter()
            .map(|p| 4 * p.1.len() + size_of::<(SectionId, Box<[u32]>)>())
            .sum::<usize>()
            + (self.leads.len() + self.deferred.len()) * size_of::<SectionId>()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn i29_index_is_sparse_and_sorted() {
        let ix = RecIndex::new(
            vec![(SectionId(9), vec![1, 0].into()), (SectionId(3), vec![2, 0, 1].into())],
            vec![SectionId(9), SectionId(2), SectionId(9)],
        );
        assert_eq!(ix.get(SectionId(3)), Order::Permuted(&[2, 0, 1]));
        assert_eq!(ix.get(SectionId(9)), Order::Permuted(&[1, 0]));
        assert_eq!(ix.get(SectionId(4)), Order::Sorted);
        assert!(ix.leads(SectionId(2)) && ix.leads(SectionId(9)) && !ix.leads(SectionId(3)));
        assert_eq!(ix.unsorted().collect::<Vec<_>>(), [SectionId(3), SectionId(9)]);
        assert_eq!(RecIndex::default().get(SectionId(0)), Order::Sorted);
    }
}
