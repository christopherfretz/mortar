//! What a table's own `check` reports: the invariant of the spec a table
//! breaks, found from the table alone. A broken derived table is a bug of
//! the phase that wrote it (tests and `debug_assert`s call `check`); a
//! layout has its own, richer report (`crate::layout::Violation`).

use std::fmt;

/// A table breaks an invariant of `spec/model.md`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Broken {
    /// The table: `"Inputs"`, `"Resolution"`, ...
    pub table: &'static str,
    /// Which row and how, in words.
    pub what: String,
}

impl fmt::Display for Broken {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}: {}", self.table, self.what)
    }
}

impl std::error::Error for Broken {}

/// `Ok` when `holds`, else the breach `what` describes.
pub(crate) fn ensure(
    holds: bool,
    table: &'static str,
    what: impl FnOnce() -> String,
) -> Result<(), Broken> {
    if holds { Ok(()) } else { Err(Broken { table, what: what() }) }
}

/// The first row of `rows` that breaks a strictly ascending order by `key`
/// (sorted, no key twice), if one does.
pub(crate) fn unsorted_at<T, K: Ord>(rows: &[T], key: impl Fn(&T) -> K) -> Option<usize> {
    rows.windows(2).position(|pair| key(&pair[0]) >= key(&pair[1])).map(|i| i + 1)
}
