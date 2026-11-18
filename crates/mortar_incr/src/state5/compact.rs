//! The compaction (`spec/incr.md` IN-18f; IN-77's W5/C4 when the patch does
//! not fit, or `S` is hard-linked or a symbolic link). WP-213 (U-1): the
//! entry point [`compact`]; **filled by U-2**.

use std::path::Path;

use mortar_core::incr::{IncrReason, Unimplemented};

use crate::ops::{WriteFailure, WriteOps};
use crate::update::view::{StatePatch, StateView};

/// Write the whole new state (IN-18f steps 1-5): read and verify every
/// block of `view`, apply `patch`, drop the dead rows, renumber, write to
/// `dir(state_path)/.<base(state_path)>.mortar-compact` (created and
/// `flock`ed through `ops`), `fdatasync`, rename over `state_path`, `fsync`
/// the directory. Called by U-7's `write` at W5/C4.
///
/// # Errors
/// [`WriteFailure::Fallback`] with `state-corrupt` (step 1, before W1 only:
/// U-7 calls it after W4, see WP-213's question Q-1);
/// [`WriteFailure::Failed`] for an I/O error. Until U-2 lands:
/// `update-not-implemented (state5)`.
pub fn compact(
    view: &StateView<'_>,
    patch: &StatePatch,
    ops: &mut dyn WriteOps,
    state_path: &Path,
) -> Result<(), WriteFailure> {
    let _ = (view, patch, ops, state_path);
    Err(WriteFailure::Fallback(IncrReason::UpdateNotImplemented(Some(Unimplemented::State5))))
}
