//! The harness behind `cargo xtask`. Contract: `spec/harness.md`.
//!
//! A library so that tests elsewhere in the workspace, and the harness's
//! own commands, can call the same code a command runs.
//!
//! Every process the harness starts goes through [`process`]: a bounded
//! child ([`process::run`], [`process::run_measured`]) or a cargo step
//! ([`process::run_step`]), never a bare `Command`.

pub mod ci;
pub mod jobs;
pub mod plumb;
#[cfg(unix)]
pub mod process;
