//! The compaction (`spec/incr.md` IN-18f; IN-77's W5/C4 when the patch does
//! not fit, or `S` is hard-linked or a symbolic link). WP-213 (U-1): the
//! entry point [`compact`]; **filled by U-2**. Its read of every block
//! (IN-18f step 1) is `super::plan_patch`'s, before W1 (WP-213 follow-up,
//! Q-1).

use std::path::Path;

use mortar_core::incr::{IncrReason, Unimplemented};

use super::write::PatchPlan;
use crate::format::StatRow;
use crate::ops::{WriteFailure, WriteOps};
use crate::update::view::StateView;

/// Write the whole new state the plan holds (IN-18f steps 2-5, read and
/// renumbered by `plan_patch`), with the binding `output` (as
/// `super::patch`), to `dir(state_path)/.<base(state_path)>.mortar-compact`
/// (created and `flock`ed through `ops`), `fdatasync`, rename over
/// `state_path`, `fsync` the directory. Writes only. Called by U-7's
/// `write` at W5/C4 when `plan.compacts()`.
///
/// # Errors
/// [`WriteFailure::Failed`] for an I/O error (IN-77e: the temporary is
/// removed, the flag stays set). Until U-2 lands:
/// [`WriteFailure::Fallback`] with `update-not-implemented (state5)`.
pub fn compact(
    view: &StateView<'_>,
    plan: &PatchPlan,
    output: &StatRow,
    ops: &mut dyn WriteOps,
    state_path: &Path,
) -> Result<(), WriteFailure> {
    let _ = (view, plan, output, ops, state_path);
    Err(WriteFailure::Fallback(IncrReason::UpdateNotImplemented(Some(Unimplemented::State5))))
}
