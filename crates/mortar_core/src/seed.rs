//! A malformed scheduling seed as a diagnostic (`spec/model.md` §6): an
//! error of the command-line phase, which the driver reports before the
//! first phase runs. `par` reads and parses the seed; this module only
//! names the error.

use mortar_diag::{Diagnostic, Key, Kind};

use crate::par::{BadSeed, SEED_VAR};

/// The key of the error (§8: the command-line phase, 10; one per link).
pub const SEED_KEY: Key = Key::new(10, 0, 0);

impl From<BadSeed> for Diagnostic {
    /// `invalid-option-value` with the variable's name and the value as
    /// set, and the note `expected a decimal integer below 2^64`.
    fn from(bad: BadSeed) -> Self {
        Diagnostic::error(Kind::InvalidOptionValue { option: SEED_VAR.to_owned(), value: bad.0 })
            .note("expected a decimal integer below 2^64")
    }
}
