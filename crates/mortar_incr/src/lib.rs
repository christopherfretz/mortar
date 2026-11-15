//! Incremental linking (`spec/incr.md`): the link state, change detection,
//! delta layout and patching. May depend on every linker crate below the
//! facade (`docs/ARCHITECTURE.md`); never on `mortar_plumb`.
//!
//! The shell of the crate; the state's format, writer and reader land with
//! WP-061.
#![forbid(unsafe_code)]
