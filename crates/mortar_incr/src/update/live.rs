//! Liveness from stored edges (`spec/incr.md` IN-29, IN-71a; IN-70 step
//! U6). WP-213 (U-1): the type [`LiveDelta`] and the entry point [`live`];
//! **filled by U-3** (the function only).

use mortar_core::SectionId;
use mortar_core::incr::{IncrReason, Unimplemented};

use super::resolve::ResolveDelta;
use super::view::{ChangedFiles, StateView};

/// The sections whose liveness flips (WP-211 §C `LiveDelta`; IN-71a,
/// IN-27's liveness row), in the update's id space. Without
/// `--gc-sections` both lists are empty. Written by U-3; read by the join
/// (`SectionChange::live`), U-4, U-3 (`refill_set`).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct LiveDelta {
    /// Dead before, live now (sorted).
    pub now_live: Vec<SectionId>,
    /// Live before, dead now (sorted).
    pub now_dead: Vec<SectionId>,
}

/// The update's liveness (WP-211 §C; IN-71a). Called by the join after
/// `resolve` (IN-70 U6).
///
/// # Errors
/// `state-corrupt`. Until U-3 lands: `update-not-implemented (resolve)`.
pub fn live(
    view: &StateView<'_>,
    changed: &ChangedFiles,
    resolved: &ResolveDelta,
) -> Result<LiveDelta, IncrReason> {
    let _ = (view, changed, resolved);
    Err(IncrReason::UpdateNotImplemented(Some(Unimplemented::Resolve)))
}
