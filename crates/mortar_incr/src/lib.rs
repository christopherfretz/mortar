//! Incremental linking (`spec/incr.md`): the link state, change detection,
//! delta layout and patching. May depend on every linker crate below the
//! facade (`docs/ARCHITECTURE.md`); never on `mortar_plumb`.
//!
//! WP-061 (stage A): the state's bytes ([`format`], IN-18a), the stamps
//! and the stat rule ([`stamp`], IN-21, IN-22, IN-22a), and the
//! translation between a link's tables and the state's identity terms
//! ([`capture`]: the writer's rows and the replay of IN-61).
#![forbid(unsafe_code)]

/// `small-output` (`spec/incr.md` §7): when the output a state describes
/// (`META`'s output size) is smaller than this, `--incremental` links in
/// full instead of updating; `--incremental=update` is not bound by it.
/// A policy constant (WP-094 §7, WP-106), not a measured crossover: an
/// update was measured at 55 MB (0.67-1.15 of a plain link) and at
/// 583-832 MB, nothing in between; 256 MiB is a power of two between the
/// two, so that the mode can be told from `ls -l`.
pub const SMALL_OUTPUT: u64 = 256 << 20;

/// Whether an output of `size` bytes is below [`SMALL_OUTPUT`].
#[must_use]
pub const fn small_output(size: u64) -> bool {
    size < SMALL_OUTPUT
}

/// Which way an update writes the output (`spec/incr.md` IN-76).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum WritePath {
    /// In place (IN-46, IN-77's W1..W6).
    InPlace,
    /// By copy (IN-47, IN-77's C1..C5).
    Copy,
}

/// The write path's policy (IN-76): the default path at or above the
/// bound and below it. `--incremental-write=copy` and IN-45's conditions
/// override the default per update (U-7); W-1 (IN-76a) changes the policy
/// per device, not this constant.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct WritePolicy {
    /// Bytes: an output of at least this many bytes takes `at_or_above`.
    pub in_place_from: u64,
    pub at_or_above: WritePath,
    pub below: WritePath,
}

impl WritePolicy {
    /// The default path for an output of `size` bytes.
    #[must_use]
    pub const fn default_for(&self, size: u64) -> WritePath {
        if size >= self.in_place_from { self.at_or_above } else { self.below }
    }
}

/// IN-76's policy constant (operator, 2026-11-17): in place by default at
/// or above [`SMALL_OUTPUT`], the copy path below (an update below the
/// bound runs only under `--incremental=update`).
pub const WRITE_PATH: WritePolicy =
    WritePolicy { in_place_from: SMALL_OUTPUT, at_or_above: WritePath::InPlace, below: WritePath::Copy };

pub mod capture;
pub mod contribution;
pub mod delta;
pub mod detect;
pub mod format;
pub mod ops;
pub mod stamp;
pub mod state5;
mod tables;
pub mod update;

#[cfg(test)]
mod tests {
    // spec: incr.md §7 (`small-output`: below 256 MiB, by one constant)
    #[test]
    fn the_small_output_bound() {
        assert_eq!(super::SMALL_OUTPUT, 256 * 1024 * 1024);
        assert!(super::small_output(0));
        assert!(super::small_output(super::SMALL_OUTPUT - 1));
        assert!(!super::small_output(super::SMALL_OUTPUT));
        assert!(!super::small_output(u64::MAX));
    }

    // spec: incr.md IN-76 (in place from SMALL_OUTPUT, copy below)
    #[test]
    fn the_write_path_policy() {
        use super::{SMALL_OUTPUT, WRITE_PATH, WritePath};
        assert_eq!(WRITE_PATH.default_for(SMALL_OUTPUT), WritePath::InPlace);
        assert_eq!(WRITE_PATH.default_for(SMALL_OUTPUT - 1), WritePath::Copy);
    }
}
