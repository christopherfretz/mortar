//! The canonical form of a state (`spec/incr.md` IN-78: `canon(S') ==
//! canon(C(U))`), for H-1's judge and `incr_state canon`. WP-213 (U-1):
//! the types [`Canon`], [`CanonDiff`] and the entry points [`canon`],
//! [`compare`]; **filled by U-2**.

use mortar_core::incr::StoredLayout;

use crate::format::StateError;

/// A state in canonical form (IN-78: files by rank position, keys by
/// name, dead rows, slack and append areas dropped, `META`'s history
/// fields left out). Opaque: U-2 decides its contents; H-1 only compares
/// two.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Canon {
    /// The canonical rows, by table kind (U-2's encoding).
    pub tables: Vec<(u32, Vec<Vec<u8>>)>,
}

/// One difference between two canonical states: the table, the row (its
/// canonical key, printable), and what differs (H-1 prints it).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CanonDiff {
    pub kind: u32,
    pub row: String,
    pub what: String,
}

/// The canonical form of the state `bytes` (a whole read: tests and
/// `incr_state` only, IN-18c).
///
/// # Errors
/// The state's own `StateError`. Until U-2 lands: `StateError::Version`.
pub fn canon(bytes: &[u8]) -> Result<Canon, StateError> {
    let _ = bytes;
    Err(StateError::Version("format 5 canon: update-not-implemented (state5)".to_owned()))
}

/// The differences of `a` against `b`, `p_u` (the update's placement as
/// its map gives it) naming the layout rows (IN-78). Empty: equal.
///
/// Until U-2 lands: empty.
#[must_use]
pub fn compare(a: &Canon, b: &Canon, p_u: &StoredLayout<'_>) -> Vec<CanonDiff> {
    let _ = (a, b, p_u);
    Vec::new()
}
