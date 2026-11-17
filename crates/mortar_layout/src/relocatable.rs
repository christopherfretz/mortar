//! Layout at `-r` (`spec/layout.md` §24, `spec/model.md` §4.6 MD-R1).
//! Owner: **RL-3** (layout at `-r`): L-R1 to L-R5, L-R7, L-R8, L-R10;
//! MD-R1's chunks; L-R6's decisions (which FDEs are removed) as data.
//! RL-3 also owns the `-r` branches of `assign.rs` and `order.rs`, and
//! L-R10's checks in `mortar_core/src/check/relocatable.rs`. After it:
//! RL-7 (L-R6a, the stack-note function only), RL-8 (the group and eh
//! parts), RL-11 (`--unique`'s key, L-R11), RL-12 (the Mason container's
//! function only), one after the other.
//!
//! WP-181 (RL-1) wrote this file with [`layout`]'s signature and its
//! refusal; RL-3 fills [`layout`] here and may add private items to this
//! file.

use mortar_core::par::Pool;
use mortar_core::plan::Plan;
use mortar_core::{Inputs, Liveness, RelocatableLayout, Resolution};

use crate::{LayoutDiag, Target};

/// The layout of a `-r` link: the assignment by L-R1/L-R2 and the
/// placement of MD-R1 (`PlacementKind::Relocatable`, no segment, no
/// address, L-R8's order, file offsets), with L-R6's removed `.eh_frame`
/// records as data. `Layout::check` (L-R10) holds of the result. RL-7's
/// driver calls it at `-r` in place of `assign::assign` and `place`, and
/// hands the result to `mortar_image::relocatable::write`.
///
/// # Errors
///
/// Every diagnostic of the layout (`section-conflict`, the refusals of
/// `spec/diag.md` §4.4 that layout raises). Until RL-3 lands:
/// `unsupported feature: relocatable output (-r)`.
pub fn layout<'s>(
    inputs: &Inputs<'s>,
    resolution: &Resolution,
    liveness: &Liveness,
    target: &Target,
    plan: &Plan,
    pool: &Pool,
) -> Result<RelocatableLayout<'s>, Vec<LayoutDiag>> {
    let _ = (inputs, resolution, liveness, target, plan, pool);
    Err(vec![LayoutDiag::from(
        mortar_diag::relocatable::refused_relocatable_output()
            .note("layout at -r is RL-3's (spec/layout.md §24)"),
    )])
}
