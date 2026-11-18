//! The refill set (`spec/incr.md` IN-27, IN-74; IN-70 step U8). WP-213
//! (U-1): the entry point [`refill_set`]; **filled by U-3** (the function
//! only). The type `RefillSet` is `mortar_core::incr::RefillSet` (moved
//! there by U-1: `mortar_image` reads it and cannot see this crate).

use mortar_core::incr::{IncrReason, LayoutDelta, Unimplemented};
pub use mortar_core::incr::{RefillChunk, RefillSet, RefillUnit};

use super::defined::DefinedChange;
use super::resolve::ResolveDelta;
use super::view::StateView;

/// The units the update refills (WP-211 §C; IN-74). Called by the join
/// after `recompute` (IN-70 U8).
///
/// # Errors
/// `too-much-changed`, `state-corrupt`. Until U-3 lands:
/// `update-not-implemented (refill)`.
pub fn refill_set(
    view: &StateView<'_>,
    resolved: &ResolveDelta,
    layout: &LayoutDelta,
    defined: &[DefinedChange],
) -> Result<RefillSet, IncrReason> {
    let _ = (view, resolved, layout, defined);
    Err(IncrReason::UpdateNotImplemented(Some(Unimplemented::Refill)))
}
