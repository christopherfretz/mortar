//! The one writer to the process's standard error and standard output
//! (`spec/cli.md` §3.15, `spec/diag.md` §7.1, WP-095).
//!
//! Every byte a shipped crate of mortar prints goes through here: the
//! report, `--print-gc-sections`' and `--time-phases`' lines, incremental
//! mode's lines and `MORTAR_STEPS`' lines to standard error; the answers
//! of `--version`, `-v`, `-V`, `--help` and `-M`'s map to standard output.
//! `print!`, `eprint!` and their `ln` forms panic when the write fails
//! (the reader of a pipe has gone, `EPIPE`, since Rust ignores `SIGPIPE`;
//! a full device, `ENOSPC`), and mortar builds with `panic = "abort"`: a
//! message nobody read would end the link in `SIGABRT` with a core and its
//! temporary output left behind. Here a failed write never panics:
//!
//! - [`to_stderr`] drops what cannot be written (rule S-1): the link's
//!   verdict and exit status do not depend on who reads its messages;
//! - [`to_stdout`] returns the error, and its caller decides: an answer to
//!   a request is dropped (S-1), `-M`'s map is an output and its failure is
//!   an error by name (S-2).
//!
//! It lives in `mortar_diag` because every crate that prints depends on it
//! already (`mortar_core`'s step timers included) and it depends on
//! nothing (`docs/ARCHITECTURE.md`, the dependency rules). A test
//! (`tests/streams.rs`) fails when a shipped crate prints any other way.

use std::io::{IsTerminal as _, Write as _};

/// Writes `text` whole to standard error under its lock, then flushes.
/// A write that fails is dropped without a word (`spec/cli.md` §3.15,
/// S-1): there is no other place to say it, and the link goes on to its
/// own verdict. Never panics.
pub fn to_stderr(text: &str) {
    if text.is_empty() {
        return;
    }
    let mut err = std::io::stderr().lock();
    let _ = err.write_all(text.as_bytes()).and_then(|()| err.flush());
}

/// Writes `text` whole to standard output under its lock, then flushes,
/// and returns the first error (`spec/cli.md` §3.15: S-1 for an answer,
/// S-2 for an output). Never panics.
pub fn to_stdout(text: &str) -> std::io::Result<()> {
    if text.is_empty() {
        return Ok(());
    }
    let mut out = std::io::stdout().lock();
    out.write_all(text.as_bytes())?;
    out.flush()
}

/// Whether standard error is a terminal (`--color-diagnostics=auto`,
/// `spec/cli.md` §3.9).
#[must_use]
pub fn stderr_is_terminal() -> bool {
    std::io::stderr().is_terminal()
}
