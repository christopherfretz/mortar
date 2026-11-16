//! Incremental linking (`spec/incr.md`): the link state, change detection,
//! delta layout and patching. May depend on every linker crate below the
//! facade (`docs/ARCHITECTURE.md`); never on `mortar_plumb`.
//!
//! WP-061 (stage A): the state's bytes ([`format`], IN-18a), the stamps
//! and the stat rule ([`stamp`], IN-21, IN-22, IN-22a), and the
//! translation between a link's tables and the state's identity terms
//! ([`capture`]: the writer's rows and the replay of IN-61).
#![forbid(unsafe_code)]

pub mod capture;
pub mod delta;
pub mod detect;
pub mod format;
pub mod stamp;
mod tables;
