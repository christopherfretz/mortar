//! The command line (`spec/cli.md`): GNU ld's dialect, parsed to a
//! [`Plan`] that later phases read; the option table, with each option's
//! status; the texts made from the table (`--help`, `--version`,
//! `docs/status/options.md`).
//!
//! [`parse`] never fails: what is wrong is a diagnostic, keyed on the
//! argument, and the plan holds everything that could be read. Using an
//! option the table marks refused is not an error here; the facade asks
//! [`Parsed::refusals`] and fails the link with them.

#![forbid(unsafe_code)]

mod grammar;

/// The phase ordinal of the command line's diagnostics (`spec/model.md`
/// §8, `spec/cli.md` §2.10).
pub const PHASE: u16 = 10;
pub use mortar_core::plan;
pub mod response;
mod rows;
pub mod table;
mod text;

pub use grammar::{Parsed, Use, check_target, parse};
pub use plan::*;
pub use table::rows;
pub use text::{
    VERSION, help_text, print_version_text, status_markdown, version_line, version_text,
};
