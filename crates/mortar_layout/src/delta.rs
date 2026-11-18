//! The delta layout (`spec/incr.md` IN-72 items 1-12, the growth step,
//! commons; IN-70 step U7): the update's `Placement` from the stored one,
//! never a fresh one. Not `mortar_incr::delta` (the floor's refill
//! report). WP-213 (U-1): the entry point [`layout`]; **filled by U-4**.
//! Its types are `mortar_core::incr`'s (`StoredLayout`, `SectionChange`,
//! `LayoutDelta`): this crate does not depend on `mortar_incr`.

use mortar_core::incr::{IncrReason, LayoutDelta, LayoutInput, StoredLayout, Unimplemented};

use crate::target::Target;

/// Place the update's sections in the stored layout (WP-211 §C; IN-72).
/// `input.changes`: every section of every changed, appeared, drawn-in or
/// newly extracted file (sorted by id), plus the sections whose liveness
/// or keeping flips (the join builds them from U-3's results); per such
/// file, `input.eh`, `input.pieces` and `input.demands` (items 5, 6, 7;
/// WP-216 seam 4). Called by the join (IN-70 U7), after the join's
/// `update-not-implemented (common)` check (R-2).
///
/// # Errors
/// IN-72's reasons (`reserve-exhausted`, `island-exhausted`,
/// `new-output-section`, `property-changed`, `link-order-moved`,
/// `tls-layout-changed`, `copy-size-changed`, `state-corrupt`). Until U-4
/// lands: `update-not-implemented (layout)`.
pub fn layout(
    stored: &StoredLayout<'_>,
    input: &LayoutInput<'_>,
    target: &Target,
) -> Result<LayoutDelta, IncrReason> {
    let _ = (stored, input, target);
    Err(IncrReason::UpdateNotImplemented(Some(Unimplemented::Layout)))
}
