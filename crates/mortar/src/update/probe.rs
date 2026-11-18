//! The clone probe (`spec/incr.md` IN-76a): whether the output's file
//! system clones, so that the copy path is the default at every size.
//! WP-213 (U-1): the entry point [`clone_capable`]; **filled by W-1**
//! (after U-10).

use mortar_incr::update::rows::MetaRow;

/// Whether the copy path on device `dev` would be a clone, from `META`'s
/// probe record (IN-76a), or `None` when unknown (no record for `dev`).
/// Called by U-7 when choosing the path.
///
/// Until W-1 lands: `None`.
#[must_use]
pub fn clone_capable(dev: u64, meta: &MetaRow) -> Option<bool> {
    let _ = (dev, meta);
    None
}
