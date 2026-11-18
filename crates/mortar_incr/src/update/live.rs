//! Liveness from stored edges (`spec/incr.md` IN-29, IN-71a; IN-70 step
//! U6). WP-213 (U-1): the type [`LiveDelta`] and the entry point [`live`];
//! **filled by U-3** (the function only).

use mortar_core::incr::{IncrReason, OptionKeys, Unimplemented};
use mortar_core::{FileId, SectionId};

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
    /// Unchanged files with a section in `now_live`/`now_dead` or whose
    /// split sections' marked offsets changed (sorted): the join reads
    /// each as `Change::DrawnIn` (IN-72 item 2a, R-4) before U-4.
    pub drawn_in: Vec<FileId>,
    /// Per live split section of a changed or drawn-in file, the offsets
    /// the new walk marked (sorted; `u32::MAX`: every piece): U-4 item 6,
    /// `FRAG_USE` (WP-239).
    pub pieces: Vec<(SectionId, Vec<u32>)>,
    /// The new walk's live-set hash (S4), for `META.live_hash`.
    pub live_hash: [u8; 16],
}

/// The update's liveness (WP-211 §C; IN-71a). Called by the join after
/// `resolve` (IN-70 U6). `options`: what the update reads of the plan,
/// passed in (WP-215 D3-D5).
///
/// IN-29's stored graph and IN-71a's walk are not built (WP-215 seam 3,
/// G3; [later] WP-238): without `--gc-sections` the delta is empty and
/// nothing is read; with it, every update falls back, decided from the
/// option alone. Never "empty" under `--gc-sections`: a section of an
/// unchanged file that a change makes live would be missed.
///
/// # Errors
/// `update-not-implemented (live)` under `--gc-sections`.
pub fn live(
    view: &StateView<'_>,
    changed: &ChangedFiles,
    resolved: &ResolveDelta,
    options: &OptionKeys,
) -> Result<LiveDelta, IncrReason> {
    let _ = (view, changed, resolved);
    if options.gc_sections {
        return Err(IncrReason::UpdateNotImplemented(Some(Unimplemented::Live)));
    }
    Ok(LiveDelta::default())
}
