//! ODR checks in an update (`spec/incr.md` IN-71d; odr.md O-20..O-23):
//! the ODR-affected keys' classes recomputed from `ODR_REFS` and the
//! changed files' rows. WP-213 (U-1): the entry point [`odr`]; **filled by
//! U-8** (the function only).

use mortar_core::incr::{IncrReason, Unimplemented};
use mortar_core::plan::OdrMode;

use super::diags::DiagRow;
use super::resolve::ResolveDelta;
use super::view::{ChangedFiles, StateView};

/// The ODR findings of the ODR-affected keys (WP-211 §C; IN-71d), as
/// `DIAGS` rows for U-9's union. Called by the join after `resolve`.
///
/// # Errors
/// `state-corrupt`; a finding `mode` makes fatal fails the link as in a
/// full link (U-8 returns it as the join's error, IN-5). Until U-8 lands:
/// `update-not-implemented (meta)` when any changed file carries
/// `.mason.meta` units, else no findings.
pub fn odr(
    view: &StateView<'_>,
    changed: &ChangedFiles,
    resolved: &ResolveDelta,
    mode: OdrMode,
) -> Result<Vec<DiagRow>, IncrReason> {
    let _ = (view, resolved, mode);
    if changed.files.iter().any(|f| f.meta_units > 0) {
        return Err(IncrReason::UpdateNotImplemented(Some(Unimplemented::Meta)));
    }
    Ok(Vec::new())
}
