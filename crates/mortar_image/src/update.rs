//! Refill and image in an update (`spec/incr.md` IN-73, IN-75; IN-70 step
//! U9): the refill set's units relocated again, the synthetic chunks and
//! headers rewritten, the dirty ranges and their bytes. WP-213 (U-1): the
//! type [`RefillError`] and the entry point [`refill`]; **filled by U-6**,
//! which also fills `crate::map::update_map`.

use std::fs::File;

use mortar_core::incr::{
    DirtyRange, IncrReason, LayoutDelta, RefillSet, SymtabPlan, Unimplemented,
};

use crate::fill::Failed;
use crate::link::Link;

/// How a refill ends without bytes (WP-213: WP-211 §C's `Failed` cannot
/// carry "until it lands", nor IN-75's `input-changed`).
#[derive(Debug)]
pub enum RefillError {
    /// A §7 reason: the update falls back (`input-changed`, IN-75;
    /// `update-not-implemented (image)`).
    Fallback(IncrReason),
    /// The link fails as a full link would (a relocation error of a
    /// refilled unit, IN-5).
    Failed(Failed),
}

/// Refill the update's units against `previous` (U1's descriptor of the
/// output) and give the dirty ranges, sorted and disjoint, with their
/// bytes concatenated in range order (WP-211 §C; IN-73, IN-75). Called by
/// the join (IN-70 U9).
///
/// # Errors
/// [`RefillError`]. Until U-6 lands: `update-not-implemented (image)`.
pub fn refill(
    link: &Link<'_, '_>,
    layout: &LayoutDelta,
    symtab: &SymtabPlan,
    set: &RefillSet,
    previous: &File,
) -> Result<(Vec<DirtyRange>, Vec<u8>), RefillError> {
    let _ = (link, layout, symtab, set, previous);
    Err(RefillError::Fallback(IncrReason::UpdateNotImplemented(Some(Unimplemented::Image))))
}
