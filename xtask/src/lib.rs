//! The harness behind `cargo xtask`. Contract: `spec/harness.md`.
//!
//! A library so that tests elsewhere in the workspace, and the harness's
//! own commands, can call the same code a command runs.
//!
//! Every process the harness starts goes through [`process`]: a bounded
//! child ([`process::run`], [`process::run_measured`]) or a cargo step
//! ([`process::run_step`]), never a bare `Command`.

#[cfg(unix)]
pub mod bench;
#[cfg(unix)]
pub mod census;
pub mod ci;
#[cfg(unix)]
pub mod identical;
#[cfg(unix)]
pub mod incr_oracle;
pub mod jobs;
#[cfg(unix)]
pub mod l10;
#[cfg(unix)]
pub mod linktest;
#[cfg(unix)]
pub mod mason;
#[cfg(unix)]
pub mod meta;
#[cfg(unix)]
pub mod odr_runtime;
pub mod plumb;
#[cfg(unix)]
pub mod process;
#[cfg(unix)]
pub mod realbuild;
#[cfg(unix)]
pub mod sweep;
