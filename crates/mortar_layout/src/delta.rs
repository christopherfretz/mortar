//! The delta layout (`spec/incr.md` IN-72 items 1-12, the growth step,
//! commons; IN-70 step U7): the update's `Placement` from the stored one,
//! never a fresh one. Not `mortar_incr::delta` (the floor's refill
//! report). WP-213 (U-1): the entry point [`layout`]; **filled by U-4**.
//! Its types are `mortar_core::incr`'s (`StoredLayout`, `SectionChange`,
//! `LayoutDelta`): this crate does not depend on `mortar_incr`.

use mortar_core::incr::{IncrReason, LayoutDelta, SectionChange, StoredLayout, Unimplemented};

use crate::target::Target;

/// Place the update's sections in the stored layout (WP-211 §C; IN-72).
/// `changes`: every section of every changed, appeared, drawn-in or newly
/// extracted file (sorted by id), plus the sections whose liveness or
/// keeping flips (the join builds them from U-3's results). Called by the
/// join (IN-70 U7).
///
/// # Errors
/// IN-72's reasons (`reserve-exhausted`, `island-exhausted`,
/// `new-output-section`, `property-changed`, `link-order-moved`,
/// `tls-layout-changed`, `copy-size-changed`, `state-corrupt`). Until U-4
/// lands: `update-not-implemented (layout)`.
pub fn layout(
    stored: &StoredLayout<'_>,
    changes: &[SectionChange<'_>],
    target: &Target,
) -> Result<LayoutDelta, IncrReason> {
    let _ = (stored, changes, target);
    Err(IncrReason::UpdateNotImplemented(Some(Unimplemented::Layout)))
}
