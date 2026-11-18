//! ODR checks in an update (`spec/incr.md` IN-71d; odr.md §8.1): W1 and
//! W2 recomputed for the affected keys from `COPIES`, `RESOLVED` and the
//! changed files' copies, by the full link's comparator
//! (`mortar_odr::foreign::compare_copies`); every other check of odr.md
//! falls back from the switches alone
//! (`mortar_odr::register::update_fallback`). WP-213 (U-1): the entry
//! point [`odr`]; WP-220 seam: its signature takes the switches; **filled
//! by U-8** (the function only).

use mortar_core::incr::{IncrReason, Unimplemented};
use mortar_core::plan::Odr;

use super::diags::DiagRow;
use super::resolve::ResolveDelta;
use super::view::{ChangedFiles, StateView};

/// The ODR findings of the ODR-affected keys (WP-211 §C; IN-71d), as
/// `DIAGS` rows for U-9's union. Called by the join after `resolve`.
///
/// # Errors
/// `state-corrupt`; `update-not-implemented (meta)` when the switches
/// turn on anything but W1 and W2 (IN-71d), or under `--keep-mason-meta`
/// with units in a changed file. The switches are passed in, never
/// stored. Until U-8 lands: `update-not-implemented (meta)` when any
/// changed file carries `.mason.meta` units, else no findings.
pub fn odr(
    view: &StateView<'_>,
    changed: &ChangedFiles,
    resolved: &ResolveDelta,
    odr: &Odr,
) -> Result<Vec<DiagRow>, IncrReason> {
    let _ = (view, resolved, odr);
    if changed.files.iter().any(|f| f.meta_units > 0) {
        return Err(IncrReason::UpdateNotImplemented(Some(Unimplemented::Meta)));
    }
    Ok(Vec::new())
}
