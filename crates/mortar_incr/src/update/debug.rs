//! Debug sections in an update (`spec/incr.md` IN-50..IN-57, IN-70's
//! debug condition). WP-213 (U-1): the file and its `mod` line only;
//! **filled by G-1** (per-file contributions), which adds no `mod` line
//! and no shared type (its entry point is G-1's to propose, through the
//! supervisor, when it starts). Until G-3 joins it, an update with a
//! changed file carrying debug sections is `update-not-implemented
//! (debug)` (the join's check on `ChangedFile::debug`).
