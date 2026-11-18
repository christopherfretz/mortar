//! The state patched in place (`spec/incr.md` IN-46b, IN-18e; IN-77's
//! W5/C4), in two calls (WP-213 follow-up, Q-1): [`plan_patch`] before W1
//! (every read, every check, every decision) and [`patch`] at W5 (writes
//! only). WP-213 (U-1): the entry points and [`PatchPlan`]'s one public
//! accessor; **filled by U-2** (`PatchPlan`'s contents are U-2's).

use mortar_core::incr::{IncrReason, Unimplemented};

use crate::format::StatRow;
use crate::ops::{WriteFailure, WriteOps};
use crate::update::view::{StatePatch, StateView};

/// What W5 (or the compaction) will write, decided before W1 by
/// [`plan_patch`]: the verified blocks with the patch's rows in them, or
/// the whole compacted state. **Opaque** to every package but U-2; the one
/// public question is [`PatchPlan::compacts`].
///
/// `Default` is the empty plan (writes nothing, patches in place): for the
/// packages' tests until U-2 lands, since [`plan_patch`] gives none before
/// then.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct PatchPlan {
    /// W5 is the compaction (IN-18f): the rows do not fit, or `S` is
    /// hard-linked or a symbolic link (IN-77b).
    compacts: bool,
}

impl PatchPlan {
    /// W5/C4 is the compaction ([`super::compact`]), not [`patch`]; the
    /// report's `delta state compacted` (IN-46b).
    #[must_use]
    pub const fn compacts(&self) -> bool {
        self.compacts
    }
}

/// Plan the state's patch (IN-46b "verified before re-hashed"; IN-70 U9):
/// read every block W5 will write and check it against its block-list
/// entry (IN-18c), put `patch`'s rows into the blocks, and decide in place
/// or compaction (a range, append area or tombstone bound that does not
/// fit, IN-18e; `S` hard-linked or a symbolic link, IN-77b). For a
/// compaction, IN-18f step 1's read of every block happens here too.
/// Called by the join in U9, **before W1**: a failure here leaves the old
/// pair untouched.
///
/// # Errors
/// `state-corrupt` (a block that fails its hash). Until U-2 lands:
/// `update-not-implemented (state5)`.
pub fn plan_patch(view: &StateView<'_>, patch: &StatePatch) -> Result<PatchPlan, IncrReason> {
    let _ = (view, patch);
    Err(IncrReason::UpdateNotImplemented(Some(Unimplemented::State5)))
}

/// Write the planned patch at W5/C4 (IN-46b, IN-77): the planned blocks,
/// their block-list entries, the directory entries, and the header but
/// its flags word, with the new binding: `output`, the `STAT` output row
/// W4 or C3 took (IN-77a: the tuple with its reference stamp; known only
/// after W4, so it is not in the plan), and `META`'s `output-synced` 1;
/// then `fdatasync`. Writes only: no read, no check. Every call through
/// `ops` on the view's descriptor. Called by U-7's `write` when
/// `!plan.compacts()`.
///
/// # Errors
/// [`WriteFailure::Failed`] for an I/O error (IN-77e). Until U-2 lands:
/// [`WriteFailure::Fallback`] with `update-not-implemented (state5)`.
pub fn patch(
    view: &StateView<'_>,
    plan: &PatchPlan,
    output: &StatRow,
    ops: &mut dyn WriteOps,
) -> Result<(), WriteFailure> {
    let _ = (view, plan, output, ops);
    Err(WriteFailure::Fallback(IncrReason::UpdateNotImplemented(Some(Unimplemented::State5))))
}
