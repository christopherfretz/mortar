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

pub mod capture;
pub mod delta;
pub mod detect;
pub mod format;
pub mod stamp;
mod tables;

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
}
