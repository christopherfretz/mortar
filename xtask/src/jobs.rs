//! xtask as a GNU make jobserver client (`spec/harness.md` "Jobserver").
//! Ported from hod's `xtask/src/jobs.rs`, itself from kiln's.
//!
//! The client itself is `mortar_testutil::jobs` (the test binaries are
//! clients too, and share it): the fifo named by `MAKEFLAGS`, the implicit
//! token, the helper thread, [`Token`] with release on `Drop`. This module
//! adds what only a long-running command needs:
//!
//! - [`global`] also starts a thread that catches SIGINT and SIGTERM (a
//!   `signal-hook` thread, so the `unsafe` of signal handling stays in that
//!   dependency): every live child of [`children`] is killed and reaped,
//!   then the held and spare pool bytes are written back, then the signal's
//!   default action ends the process. Only SIGKILL (which no process can
//!   catch) or another uncaught fatal signal loses the tokens xtask held,
//!   as it would for make or cargo; the refill loop of
//!   `scripts/jobserver.sh` restores the pool afterwards.
//! - [`children`]: the table of live children, and the cargo steps.

pub mod children;

use std::sync::Once;
use std::sync::atomic::{AtomicBool, Ordering};

pub use mortar_testutil::jobs::{Client, Refill, Token, fifo_from_makeflags, refill};

/// The jobserver this process inherited, if any (read from `MAKEFLAGS` once).
/// The first call also starts the SIGINT/SIGTERM thread (with or without a
/// pool: it kills the live children either way).
pub fn global() -> &'static Client {
    static SIGNALS: Once = Once::new();
    let client = mortar_testutil::jobs::global();
    SIGNALS.call_once(|| release_on_signal(client));
    client
}

/// Whether the SIGINT/SIGTERM thread runs (set once it is started).
static SIGNALS_CAUGHT: AtomicBool = AtomicBool::new(false);

/// Starts the SIGINT/SIGTERM thread if it is not running yet ([`global`])
/// and says whether it runs: when it does, a child in a process group of
/// its own is still stopped by an interrupt of xtask
/// ([`children::run_step`]).
pub fn signals_caught() -> bool {
    global();
    SIGNALS_CAUGHT.load(Ordering::SeqCst)
}

/// How long an interrupted xtask waits for its killed children to be reaped
/// before returning the tokens anyway (a child in uninterruptible sleep).
#[cfg(unix)]
const SHUTDOWN_GRACE: std::time::Duration = std::time::Duration::from_secs(10);

/// Starts a thread that, on SIGINT or SIGTERM, kills and reaps every live
/// child ([`children::kill_live_children`]), then returns every pool byte
/// the process holds, then ends it with the signal's default action (so the
/// parent still sees "killed by SIGINT"; exit status 128 + signal if that
/// cannot be emulated). The tokens go back after the children are dead, so
/// the pool is never oversubscribed by an orphan, and exactly once.
#[cfg(unix)]
fn release_on_signal(client: &'static Client) {
    use signal_hook::consts::{SIGINT, SIGTERM};
    let mut signals = match signal_hook::iterator::Signals::new([SIGINT, SIGTERM]) {
        Ok(s) => s,
        Err(e) => {
            eprintln!(
                "xtask: warning: cannot catch SIGINT/SIGTERM ({e}); an interrupted run \
                 will neither kill its children nor return its jobserver tokens"
            );
            return;
        }
    };
    let spawned = std::thread::Builder::new().name("jobserver-signals".into()).spawn(move || {
        if let Some(sig) = signals.forever().next() {
            let killed = children::kill_live_children(SHUTDOWN_GRACE);
            if killed > 0 {
                eprintln!("xtask: signal {sig}: killed {killed} child process(es)");
            }
            let n = client.release_all();
            if n > 0 {
                eprintln!("xtask: signal {sig}: returned {n} jobserver token(s)");
            }
            let _ = signal_hook::low_level::emulate_default_handler(sig);
            std::process::exit(128 + sig);
        }
    });
    match spawned {
        Ok(_) => SIGNALS_CAUGHT.store(true, Ordering::SeqCst),
        Err(e) => eprintln!("xtask: warning: cannot start the jobserver signal thread ({e})"),
    }
}

#[cfg(not(unix))]
fn release_on_signal(_: &'static Client) {}
