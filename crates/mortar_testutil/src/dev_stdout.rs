//! The guard of every test that names the real `/dev/stdout`
//! (`spec/image.md` I-58b, WP-114).
//!
//! In a container whose processes run as root, `/dev` is writable: a
//! linker (or any tool) that replaces or removes `/dev/stdout` breaks it
//! for every process in the container, and every later test of every
//! checkout that writes to it. So each such test holds a [`Guard`]: when
//! the test is done, [`Guard::check`] finds `/dev/stdout` still a
//! symbolic link to `/proc/self/fd/1`, or restores it and FAILS the test
//! saying so, so that one defect fails one test instead of cascading. The
//! guard also checks when it is created: a test that finds the link
//! already broken restores it and fails, naming an earlier culprit.
//!
//! Tests of descriptor states never use the real `/dev/stdout`: they write
//! through a symbolic link of their own in their scratch directory
//! (`tests/descriptors.rs`), which is safe by construction.

use std::path::Path;
use std::process::Command;

/// The path every test of this module guards.
pub const DEV_STDOUT: &str = "/dev/stdout";
/// What it must be.
pub const TARGET: &str = "/proc/self/fd/1";

/// Whether `/dev/stdout` is a symbolic link to `/proc/self/fd/1`.
#[must_use]
pub fn intact() -> bool {
    std::fs::read_link(DEV_STDOUT).is_ok_and(|t| t == Path::new(TARGET))
}

/// What `/dev/stdout` is now, for a message.
fn describe() -> String {
    match std::fs::symlink_metadata(DEV_STDOUT) {
        Err(e) => format!("missing ({e})"),
        Ok(m) if m.file_type().is_symlink() => {
            format!("a symbolic link to {:?}", std::fs::read_link(DEV_STDOUT).ok())
        }
        Ok(m) if m.is_file() => format!("a regular file of {} bytes", m.len()),
        Ok(m) => format!("a {:?}", m.file_type()),
    }
}

/// Restores the link with exactly `ln -sfn /proc/self/fd/1 /dev/stdout`
/// (the one command the operators allow for it); whether it is intact
/// afterwards.
fn restore() -> bool {
    let _ = Command::new("ln").args(["-sfn", TARGET, DEV_STDOUT]).status();
    intact()
}

/// Checks `/dev/stdout`: `Ok` if intact (or on a system where it never
/// was a link to `/proc/self/fd/1`: `was` false); else restores it and
/// returns the message of the failure.
fn verify(what: &str, was: bool) -> Result<(), String> {
    if !was || intact() {
        return Ok(());
    }
    let now = describe();
    let restored = restore();
    Err(format!(
        "{what}: /dev/stdout is not a symbolic link to /proc/self/fd/1 (a tool or the linker \
         replaced or removed it): it is {now}; {}",
        if restored {
            "restored with `ln -sfn /proc/self/fd/1 /dev/stdout`"
        } else {
            "it could NOT be restored: run `ln -sfn /proc/self/fd/1 /dev/stdout` as root"
        }
    ))
}

/// Held by a test that names the real `/dev/stdout`.
#[derive(Debug)]
pub struct Guard {
    test: &'static str,
    /// `/dev/stdout` was meant to be the link: on Linux, where `/dev/fd`
    /// is `/proc/self/fd` (a system without it is not checked).
    linux: bool,
    checked: bool,
}

impl Guard {
    /// Starts guarding: panics, after restoring the link, if it is
    /// already broken (an earlier test or tool broke it).
    #[must_use]
    pub fn new(test: &'static str) -> Self {
        let linux = Path::new("/proc/self/fd").is_dir();
        if let Err(m) = verify(&format!("{test} (before it ran: an earlier process)"), linux) {
            panic!("{m}");
        }
        Self { test, linux, checked: false }
    }

    /// The test's last assertion: `/dev/stdout` is intact; if not it is
    /// restored and the test fails.
    pub fn check(mut self) {
        self.checked = true;
        if let Err(m) = verify(self.test, self.linux) {
            panic!("{m}");
        }
    }
}

impl Drop for Guard {
    /// A test that failed before [`Guard::check`] still restores the link
    /// (it fails anyway; no second panic during unwinding).
    fn drop(&mut self) {
        if !self.checked
            && let Err(m) = verify(self.test, self.linux)
        {
            if std::thread::panicking() {
                eprintln!("{m}");
            } else {
                panic!("{m}");
            }
        }
    }
}
