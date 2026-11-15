//! Columns: a `Vec<T>` indexed by an id (`spec/model.md` §9, §10), and the
//! split into disjoint mutable slices that parallel phases write (§6
//! rule 1).

use std::fmt;
use std::marker::PhantomData;
use std::ops::{Index, IndexMut};

use crate::id::{Id, IdOverflow, IdRange, row_count};

/// One cell per row of an id space, in id order.
#[derive(Clone, PartialEq, Eq)]
pub struct Column<I, T> {
    rows: Vec<T>,
    id: PhantomData<fn() -> I>,
}

/// Why [`Column::split_by`] refused its ranges.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SplitError {
    /// Range `index` starts before the end of the range before it: the
    /// ranges overlap or are out of order.
    NotAscending { index: usize },
    /// Range `index` ends past the last row.
    OutOfBounds { index: usize },
}

impl<I: Id, T> Column<I, T> {
    /// The empty column.
    #[must_use]
    pub fn new() -> Self {
        Self { rows: Vec::new(), id: PhantomData }
    }

    /// The column whose row `i` is `rows[i]`, or the overflow when there are
    /// more rows than an id space holds (§1).
    pub fn from_vec(rows: Vec<T>) -> Result<Self, IdOverflow> {
        row_count(rows.len())?;
        Ok(Self { rows, id: PhantomData })
    }

    /// `len` rows, each `value`.
    #[must_use]
    pub fn filled(len: u32, value: T) -> Self
    where
        T: Clone,
    {
        Self { rows: vec![value; len as usize], id: PhantomData }
    }

    /// Appends a row and returns its id.
    pub fn push(&mut self, row: T) -> Result<I, IdOverflow> {
        let index = row_count(self.rows.len() + 1)? - 1;
        self.rows.push(row);
        Ok(I::from_index(index))
    }

    /// The number of rows.
    #[must_use]
    pub fn len(&self) -> u32 {
        self.rows.len() as u32
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.rows.is_empty()
    }

    /// Every id of the column.
    #[must_use]
    pub fn ids(&self) -> IdRange<I> {
        IdRange::new(I::from_index(0), I::from_index(self.len()))
    }

    /// Whether `id` names a row.
    #[must_use]
    pub fn has(&self, id: I) -> bool {
        id.slot() < self.rows.len()
    }

    /// Row `id`, if there is one.
    #[must_use]
    pub fn get(&self, id: I) -> Option<&T> {
        self.rows.get(id.slot())
    }

    /// Row `id`, if there is one.
    pub fn get_mut(&mut self, id: I) -> Option<&mut T> {
        self.rows.get_mut(id.slot())
    }

    /// The rows of `range`, if it lies in the column.
    #[must_use]
    pub fn range(&self, range: IdRange<I>) -> Option<&[T]> {
        self.rows.get(range.slots())
    }

    /// The rows with their ids, in id order.
    pub fn iter(&self) -> impl DoubleEndedIterator<Item = (I, &T)> + ExactSizeIterator {
        self.rows.iter().enumerate().map(|(i, row)| (I::from_index(i as u32), row))
    }

    /// The rows, in id order.
    #[must_use]
    pub fn as_slice(&self) -> &[T] {
        &self.rows
    }

    /// The rows, in id order.
    pub fn as_mut_slice(&mut self) -> &mut [T] {
        &mut self.rows
    }

    /// The rows, in id order.
    #[must_use]
    pub fn into_vec(self) -> Vec<T> {
        self.rows
    }

    /// One mutable slice per range, in the order given: what a parallel
    /// phase hands each unit (§6 rule 1). The ranges must be ascending and
    /// disjoint (adjacent is fine, gaps are skipped) and lie in the column;
    /// otherwise nothing is split.
    pub fn split_by<'a>(
        &'a mut self,
        ranges: &[IdRange<I>],
    ) -> Result<Vec<&'a mut [T]>, SplitError> {
        let mut end = 0;
        for (index, range) in ranges.iter().enumerate() {
            if range.start.slot() < end {
                return Err(SplitError::NotAscending { index });
            }
            if range.end.slot() > self.rows.len() {
                return Err(SplitError::OutOfBounds { index });
            }
            end = range.end.slot().max(range.start.slot());
        }
        let mut out = Vec::with_capacity(ranges.len());
        let mut rest: &'a mut [T] = &mut self.rows;
        let mut at = 0;
        for range in ranges {
            let (_, tail) = rest.split_at_mut(range.start.slot() - at);
            let (piece, tail) = tail.split_at_mut(range.len() as usize);
            out.push(piece);
            rest = tail;
            at = range.start.slot() + range.len() as usize;
        }
        Ok(out)
    }
}

impl<I: Id, T> Default for Column<I, T> {
    fn default() -> Self {
        Self::new()
    }
}

impl<I: Id, T: fmt::Debug> fmt::Debug for Column<I, T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_list().entries(&self.rows).finish()
    }
}

impl<I: Id, T> Index<I> for Column<I, T> {
    type Output = T;
    fn index(&self, id: I) -> &T {
        &self.rows[id.slot()]
    }
}

impl<I: Id, T> IndexMut<I> for Column<I, T> {
    fn index_mut(&mut self, id: I) -> &mut T {
        &mut self.rows[id.slot()]
    }
}
