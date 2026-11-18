//! The state patched in place (`spec/incr.md` IN-46b, IN-18e; IN-77's
//! W5/C4). WP-213 (U-1): the entry point [`patch`]; **filled by U-2**.

use mortar_core::incr::{IncrReason, Unimplemented};

use crate::ops::{WriteFailure, WriteOps};
use crate::update::view::{StatePatch, StateView};

/// Write `patch` into the state `view` reads (WP-211 §C; IN-46b): the
/// changed blocks (each verified against its block-list entry first), their
/// block-list entries, the directory entries, and the header but its flags
/// word; then `fdatasync`. Every write through `ops` on the view's
/// descriptor. Called by U-7's `write` at W5/C4.
///
/// # Errors
/// [`WriteFailure::Fallback`] with `state-corrupt` (a block that fails its
/// hash) or when the patch does not fit (the caller compacts instead);
/// [`WriteFailure::Failed`] for an I/O error (IN-77e). Until U-2 lands:
/// `update-not-implemented (state5)`.
pub fn patch(
    view: &StateView<'_>,
    patch: &StatePatch,
    ops: &mut dyn WriteOps,
) -> Result<(), WriteFailure> {
    let _ = (view, patch, ops);
    Err(WriteFailure::Fallback(IncrReason::UpdateNotImplemented(Some(Unimplemented::State5))))
}
