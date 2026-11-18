//! The update's writes and the crash protocol (`spec/incr.md` IN-76,
//! IN-77, IN-77a..f, IN-45; IN-70 step U10). WP-213 (U-1): the types
//! [`Pair`], [`Written`] and the entry point [`write`]; **filled by U-7**.
//! `WriteOps` lives in `mortar_incr::ops` (U-1 moved it there: the
//! state's patch in `mortar_incr` takes it too).

use std::fs::File;
use std::path::Path;

use mortar_core::incr::{DirtyRange, IncrReason, Unimplemented};
use mortar_incr::ops::{WriteFailure, WriteOps};
use mortar_incr::stamp::Tuple;
use mortar_incr::state5::PatchPlan;
use mortar_incr::update::view::StateView;
use mortar_incr::{WritePath, WritePolicy};

/// The output and the state as U1 opened them (IN-77a: one descriptor per
/// file, never reopened by path; the paths are for the directory
/// operations: the aside name `A`, `T`, the renames).
#[derive(Debug, Clone, Copy)]
pub struct Pair<'a> {
    /// `P`'s descriptor (read-write in place, read-only for the copy) and
    /// its path.
    pub out: &'a File,
    pub out_path: &'a Path,
    /// `S`'s descriptor (read-write, locked, IN-77b) and its path.
    pub state: &'a File,
    pub state_path: &'a Path,
}

/// What the writes did (for the report: IN-76 "the report says which",
/// IN-46b `delta state compacted`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Written {
    /// The new output's tuple (W4's or C3's binding, IN-77a).
    pub tuple: Tuple,
    /// The path taken.
    pub path: WritePath,
    /// W5/C4 was the compaction (IN-18f).
    pub compacted: bool,
}

/// Write the update (WP-211 §C; IN-77's W1..W6 or C1..C5): `ranges` with
/// their `bytes` (concatenated in range order) into the output, then the
/// state's patch the join planned before W1 (`plan`, from
/// `mortar_incr::state5::plan_patch` in U9) at W5/C4:
/// `mortar_incr::state5::patch`, or `compact` when `plan.compacts()`,
/// with W4's or C3's binding; every call through `ops`. Before W3's first
/// write it hands `ranges` to `mortar_incr::testhook::dirty_ranges`, and it
/// calls `mortar_incr::testhook::crash_point` at each of IN-77's points. `policy` gives the default path (the join
/// passes `mortar_incr::WRITE_PATH`, or a policy of `Copy` on both sides
/// for `--incremental-write=copy`); IN-45's conditions are checked here.
///
/// # Errors
/// [`WriteFailure`] (IN-77e). Until U-7 lands: `update-not-implemented
/// (write)`.
pub fn write(
    pair: &Pair<'_>,
    view: &StateView<'_>,
    ranges: &[DirtyRange],
    bytes: &[u8],
    plan: &PatchPlan,
    ops: &mut dyn WriteOps,
    policy: WritePolicy,
) -> Result<Written, WriteFailure> {
    let _ = (pair, view, ranges, bytes, plan, ops, policy);
    Err(WriteFailure::Fallback(IncrReason::UpdateNotImplemented(Some(Unimplemented::Write))))
}
