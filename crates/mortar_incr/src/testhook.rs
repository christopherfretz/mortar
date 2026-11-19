//! The test hooks of the crash protocol (`spec/incr.md` IN-77 "Crash
//! hooks"; IN-75's dirty-range invariant). WP-213 follow-up (U-1): H-2,
//! U-7 and H-1 code against this module and never see each other.
//!
//! Behind the Cargo feature `testhooks` of `mortar_incr`, forwarded by
//! `mortar` (off by default). **Without the feature no environment
//! variable is read** (IN-61: a shipped binary reads none): both functions
//! are empty and `#[inline]`, so a default build's code is unchanged.
//! With it:
//!
//! - `MORTAR_INCR_CRASH=<point>`: [`crash_point`] aborts the process at
//!   `<point>` (one of [`CRASH_POINTS`]), after printing
//!   `mortar: crash hook <point>` to standard error;
//! - `MORTAR_INCR_DIRTY=<path>`: [`dirty_ranges`] writes the update's
//!   dirty ranges to `<path>`;
//! - `MORTAR_INCR_UNENFORCED` (set to anything; read in
//!   `mortar::update::write`, WP-219): the probe of image.md I-59 answers that a
//!   lock is not enforced, so a link answers `lock-unavailable` (IN-77d);
//! - `MORTAR_INCR_STATE5=<path>`, `MORTAR_INCR_STATE5_COLLIDE=1`: WP-244's
//!   format-5 capture beside the format-4 state ([`state5_path`],
//!   [`state5_collide`]);
//! - `MORTAR_INCR_SMALL_OUTPUT=<bytes>`: `small-output`'s bound for the
//!   process (WP-225, G-13; [`small_output_bound`]);
//! - `MORTAR_INCR_LIVE_CHECK` (set to anything): WP-238 P0-7's oracle of
//!   the stored liveness rows ([`live_check`]);
//! - `MORTAR_INCR_LIVE_HASH=zero` or `=flip`: the capture writes a wrong
//!   `META.live_hash` (zeros, or its first bit flipped), for the test that
//!   the oracle judges the written bytes ([`live_hash_mutated`]).

use mortar_core::incr::DirtyRange;

/// Every crash point (IN-77 "Crash hooks"): the update's, in the order of
/// IN-77's list (placed by U-7 in `mortar::update::write`), then the
/// floor's (`F-...`, placed in `mortar::floor` and the state write
/// `mortar::incremental::write_atomically`, which a full link in
/// incremental mode shares; H-2 tests against these first). Each `F-`
/// point is after the named call returned:
///
/// - `F-flag`: the state's updating flag set and synced (IN-46 step 1);
/// - in place (IN-46 steps 2-4): `F-ident` (`e_ident` zeroed and synced),
///   `F-written` (the windows and the size written, not synced),
///   `F-synced`, `F-header-written` (the true `e_ident`, not synced),
///   `F-header` (synced);
/// - by copy (IN-47): `F-copy-written` (`T` copied and written, not
///   synced), `F-copy-synced`, `F-copy-renamed` (`T` renamed over `P`, the
///   directory not synced), `F-copy-dir`;
/// - the state, rewritten whole (IN-17): `F-state-written` (the temporary
///   written, not synced), `F-state-synced`, `F-state-renamed` (renamed
///   over `S`, the directory not synced), `F-state-dir`.
pub const CRASH_POINTS: &[&str] = &[
    "W1",
    "W2",
    "W3",
    "W4",
    "W5",
    "W6",
    "C1",
    "C2",
    "C3",
    "C4",
    "C5",
    "compact-written",
    "compact-renamed",
    "W2-renamed",
    "W2-written",
    "W3-written",
    "W4-written",
    "W4-linked",
    "W4-unlinked",
    "W5-written",
    "C1-written",
    "F-flag",
    "F-ident",
    "F-written",
    "F-synced",
    "F-header-written",
    "F-header",
    "F-copy-written",
    "F-copy-synced",
    "F-copy-renamed",
    "F-copy-dir",
    "F-state-written",
    "F-state-synced",
    "F-state-renamed",
    "F-state-dir",
];

/// The crash point `name` (one of [`CRASH_POINTS`]): with the feature,
/// when `MORTAR_INCR_CRASH` equals `name`, prints `mortar: crash hook
/// <name>` to standard error and aborts (`std::process::abort`: no
/// destructor, no flush, no unwinding). A name not in [`CRASH_POINTS`] is a
/// `debug_assert`. Without the feature: nothing.
#[cfg(feature = "testhooks")]
pub fn crash_point(name: &str) {
    debug_assert!(CRASH_POINTS.contains(&name), "unknown crash point {name}");
    if std::env::var_os("MORTAR_INCR_CRASH").is_some_and(|v| v == name) {
        mortar_diag::stream::to_stderr(&format!("mortar: crash hook {name}\n"));
        std::process::abort();
    }
}

/// The crash point `name`: without the `testhooks` feature, nothing.
#[cfg(not(feature = "testhooks"))]
#[inline]
pub fn crash_point(name: &str) {
    let _ = name;
}

/// The update's dirty ranges (IN-75's invariant, checked by H-1 against
/// the previous output): with the feature and `MORTAR_INCR_DIRTY=<path>`
/// set, writes one `<offset> <len>` line per range (hexadecimal, `0x`) to
/// `<path>` (created, truncated); a failed write prints `mortar: dirty
/// ranges hook: <error>` and aborts (a test must not pass on a missing
/// file). **Called by U-7 in `mortar::update::write::write`, before W3's
/// first write (C1's on the copy path).** Without the feature: nothing.
#[cfg(feature = "testhooks")]
pub fn dirty_ranges(ranges: &[DirtyRange]) {
    use std::fmt::Write as _;
    let Some(path) = std::env::var_os("MORTAR_INCR_DIRTY") else {
        return;
    };
    let mut text = String::new();
    for r in ranges {
        let _ = writeln!(text, "{:#x} {:#x}", r.offset, r.len);
    }
    if let Err(e) = std::fs::write(&path, text) {
        mortar_diag::stream::to_stderr(&format!("mortar: dirty ranges hook: {e}\n"));
        std::process::abort();
    }
}

/// The update's dirty ranges: without the `testhooks` feature, nothing.
#[cfg(not(feature = "testhooks"))]
#[inline]
pub fn dirty_ranges(ranges: &[DirtyRange]) {
    let _ = ranges;
}

/// WP-244's switch (until the join, U-10, makes format 5 the state):
/// with the feature and `MORTAR_INCR_STATE5=<path>` set, an
/// incremental-mode full link also captures the format-5 state of the
/// link and writes it to `<path>`, beside the format-4 state it writes as
/// before, and lays out IN-34a's per-file `.eh_frame` pads. Without the
/// feature: `None`.
#[cfg(feature = "testhooks")]
#[must_use]
pub fn state5_path() -> Option<std::path::PathBuf> {
    std::env::var_os("MORTAR_INCR_STATE5").filter(|v| !v.is_empty()).map(Into::into)
}

/// WP-244's switch: without the `testhooks` feature, `None`.
#[cfg(not(feature = "testhooks"))]
#[inline]
#[must_use]
pub fn state5_path() -> Option<std::path::PathBuf> {
    None
}

/// WP-244's collision injection (WP-239 R-2, `state-collision`): with the
/// feature and `MORTAR_INCR_STATE5_COLLIDE=1`, the format-5 capture
/// repeats its first `FRAGS` row at another offset before its checks.
/// Without the feature: `false`.
#[cfg(feature = "testhooks")]
#[must_use]
pub fn state5_collide() -> bool {
    std::env::var_os("MORTAR_INCR_STATE5_COLLIDE").is_some_and(|v| v == "1")
}

/// WP-244's collision injection: without the `testhooks` feature, `false`.
#[cfg(not(feature = "testhooks"))]
#[inline]
#[must_use]
pub fn state5_collide() -> bool {
    false
}

/// WP-225's flip (format 5 is THE state): with the feature and
/// `MORTAR_INCR_STATE4=1`, an incremental-mode full link also writes the
/// link's FORMAT-4 state beside the state, at `<state>.format4`, for the
/// test floor (`mortar::floor`) and the dry run of `--incremental-report`
/// until both go (WP-225 M6). Without the feature: `false`.
#[cfg(feature = "testhooks")]
#[must_use]
pub fn state4() -> bool {
    std::env::var_os("MORTAR_INCR_STATE4").is_some_and(|v| v == "1")
}

/// WP-225's format-4 side state: without the `testhooks` feature, `false`.
#[cfg(not(feature = "testhooks"))]
#[inline]
#[must_use]
pub fn state4() -> bool {
    false
}

/// WP-225's knob (G-13): with the feature and `MORTAR_INCR_SMALL_OUTPUT`
/// set to a number of bytes, `small-output`'s bound (`crate::SMALL_OUTPUT`)
/// is that number for this process (`0`: every output updates, so the
/// lane's small scenarios reach the update). A value that is not a
/// decimal number is ignored. Without the feature: `None` (a release
/// build keeps the policy constant).
#[cfg(feature = "testhooks")]
#[must_use]
pub fn small_output_bound() -> Option<u64> {
    std::env::var("MORTAR_INCR_SMALL_OUTPUT").ok()?.parse().ok()
}

/// WP-225's knob: without the `testhooks` feature, `None`.
#[cfg(not(feature = "testhooks"))]
#[inline]
#[must_use]
pub fn small_output_bound() -> Option<u64> {
    None
}

/// WP-238 P0-7's switch: with the feature and `MORTAR_INCR_LIVE_CHECK`
/// set (to anything), an incremental-mode capture under `--gc-sections`
/// walks the stored form it writes and compares it with the full mark
/// (`mortar::incremental::live_check`); a disagreement fails the link.
/// Read only on that path. Without the feature: `false`.
#[cfg(feature = "testhooks")]
#[must_use]
pub fn live_check() -> bool {
    std::env::var_os("MORTAR_INCR_LIVE_CHECK").is_some()
}

/// WP-238 P0-7's switch: without the `testhooks` feature, `false`.
#[cfg(not(feature = "testhooks"))]
#[inline]
#[must_use]
pub fn live_check() -> bool {
    false
}

/// WP-238 Part 3b's injection: with the feature and
/// `MORTAR_INCR_LIVE_HASH=zero` (`=flip`), the capture's `META.live_hash`
/// `h` is replaced by zeros (has its first bit flipped) before the state
/// is encoded, so that P0-7's oracle, which judges the written bytes,
/// must disagree. Any other value: `h`. Without the feature: `h`.
#[cfg(feature = "testhooks")]
#[must_use]
pub fn live_hash_mutated(h: [u8; 16]) -> [u8; 16] {
    match std::env::var("MORTAR_INCR_LIVE_HASH").as_deref() {
        Ok("zero") => [0; 16],
        Ok("flip") => {
            let mut x = h;
            x[0] ^= 1;
            x
        }
        _ => h,
    }
}

/// WP-238 Part 3b's injection: without the `testhooks` feature, `h`.
#[cfg(not(feature = "testhooks"))]
#[inline]
#[must_use]
pub fn live_hash_mutated(h: [u8; 16]) -> [u8; 16] {
    h
}

#[cfg(test)]
mod tests {
    use super::*;

    // spec: incr.md IN-77 "Crash hooks" (the list, each name once)
    #[test]
    fn crash_points_are_named_once() {
        let mut names = CRASH_POINTS.to_vec();
        names.sort_unstable();
        names.dedup();
        assert_eq!(names.len(), CRASH_POINTS.len());
        let text =
            std::fs::read_to_string(concat!(env!("CARGO_MANIFEST_DIR"), "/../../spec/incr.md"))
                .expect("spec/incr.md");
        let at = text.find("**Crash hooks.**").expect("IN-77's crash hooks");
        let para = &text[at..at + text[at..].find("\n\n").expect("its end")];
        for p in CRASH_POINTS.iter().filter(|p| !p.starts_with("F-")) {
            if p.len() == 2 {
                continue; // `W1`..`W6`, `C1`..`C5`: written as ranges
            }
            assert!(para.contains(&format!("`{p}`")), "{p} is not in IN-77's list");
        }
    }

    // spec: incr.md IN-61 (without the feature, nothing is read)
    #[cfg(not(feature = "testhooks"))]
    #[test]
    fn hooks_do_nothing_without_the_feature() {
        crash_point("W1");
        dirty_ranges(&[DirtyRange { offset: 0, len: 1 }]);
        assert!(state5_path().is_none() && !state5_collide());
        assert!(small_output_bound().is_none() && !state4());
    }
}
