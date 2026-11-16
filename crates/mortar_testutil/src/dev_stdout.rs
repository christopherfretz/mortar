//! The guard of every test that names the real `/dev/stdout`
//! (`spec/image.md` I-58b, WP-114).
//!
//! In a container whose processes run as root, `/dev` is writable: a
//! linker (or any tool) that replaces or removes `/dev/stdout` breaks it
//! for every process in the container, and every later test of every
//! checkout that writes to it. So each such test holds a [`Guard`]. When
//! it is created the guard records what `/dev/stdout` is ([`classify`]):
//!
//! - a symbolic link through the descriptor table (`/proc/self/fd/1`,
//!   `fd/1`, or any link that resolves where `/proc/self/fd/1` does):
//!   healthy, and its text is remembered;
//! - missing, or not a symbolic link (the two broken states WP-114 saw):
//!   restored with `ln -sfn /proc/self/fd/1 /dev/stdout` and the test
//!   FAILS, naming an earlier culprit;
//! - some other symbolic link, or a system without `/proc/self/fd`: not
//!   touched and not guarded (said on standard error).
//!
//! When the test is done, [`Guard::check`] wants the same symbolic link
//! with the same text ([`unchanged`]). If not, the test FAILS; the link is
//! restored only when the remembered text is `/proc/self/fd/1` (the one
//! command the operators allow), otherwise it is reported, not rewritten.
//! So one defect fails one test instead of cascading, and a healthy link
//! of another shape is never rewritten.
//!
//! Tests of descriptor states never use the real `/dev/stdout`: they write
//! through a symbolic link of their own in their scratch directory
//! (`tests/descriptors.rs`), which is safe by construction.

use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::process::Command;

/// The path every test of this module guards.
pub const DEV_STDOUT: &str = "/dev/stdout";
/// The text the one allowed restore command writes.
pub const TARGET: &str = "/proc/self/fd/1";

/// What a `/dev/stdout`-like path is.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Found {
    /// A symbolic link through the descriptor table, with its text.
    Healthy(PathBuf),
    /// Missing or not a symbolic link: what it is, for a message.
    Broken(String),
    /// A symbolic link elsewhere, with its text: neither healthy nor ours
    /// to rewrite.
    Foreign(PathBuf),
}

/// What `path` is now, for a message.
fn describe(path: &Path) -> String {
    match std::fs::symlink_metadata(path) {
        Err(e) => format!("missing ({e})"),
        Ok(m) if m.file_type().is_symlink() => {
            format!("a symbolic link to {:?}", std::fs::read_link(path).ok())
        }
        Ok(m) if m.is_file() => format!("a regular file of {} bytes", m.len()),
        Ok(m) => format!("a {:?}", m.file_type()),
    }
}

/// Classifies `path` (the real `/dev/stdout`, or a scratch link in a
/// test). Healthy: a symbolic link whose text is `/proc/self/fd/1` or
/// `fd/1`, or that resolves to where `/proc/self/fd/1` resolves.
#[must_use]
pub fn classify(path: &Path) -> Found {
    match std::fs::symlink_metadata(path) {
        Ok(m) if m.file_type().is_symlink() => {}
        _ => return Found::Broken(describe(path)),
    }
    let Ok(text) = std::fs::read_link(path) else {
        return Found::Broken(describe(path));
    };
    let same = match (std::fs::canonicalize(path), std::fs::canonicalize(TARGET)) {
        (Ok(a), Ok(b)) => a == b,
        _ => false,
    };
    if text == Path::new(TARGET) || text == Path::new("fd/1") || same {
        Found::Healthy(text)
    } else {
        Found::Foreign(text)
    }
}

/// Whether `path` is still a symbolic link whose text is `remembered`;
/// if not, what it is now.
///
/// # Errors
/// The description of what `path` is now.
pub fn unchanged(path: &Path, remembered: &Path) -> Result<(), String> {
    let link = std::fs::symlink_metadata(path).is_ok_and(|m| m.file_type().is_symlink());
    if link && std::fs::read_link(path).is_ok_and(|t| t == remembered) {
        Ok(())
    } else {
        Err(describe(path))
    }
}

/// Restores the real link with exactly `ln -sfn /proc/self/fd/1
/// /dev/stdout`; whether it is a link to `/proc/self/fd/1` afterwards.
fn restore() -> bool {
    let _ = Command::new("ln").args(["-sfn", TARGET, DEV_STDOUT]).status();
    unchanged(Path::new(DEV_STDOUT), Path::new(TARGET)).is_ok()
}

fn restored(done: bool) -> &'static str {
    if done {
        "restored with `ln -sfn /proc/self/fd/1 /dev/stdout`"
    } else {
        "it could NOT be restored: run `ln -sfn /proc/self/fd/1 /dev/stdout` as root"
    }
}

/// Written once per process, past the test harness's capture, so a hosted
/// run's log shows what its `/dev/stdout` is.
fn announce(found: &Found) {
    static ONCE: std::sync::Once = std::sync::Once::new();
    ONCE.call_once(|| {
        let _ = writeln!(std::io::stderr(), "[dev_stdout guard] /dev/stdout: {found:?}");
    });
}

/// Held by a test that names the real `/dev/stdout`.
#[derive(Debug)]
pub struct Guard {
    test: &'static str,
    /// The text of the healthy link when the guard was made; `None`: not
    /// guarded (no `/proc/self/fd`, or a foreign link).
    remembered: Option<PathBuf>,
    checked: bool,
}

impl Guard {
    /// Starts guarding. Panics, after restoring the link, if `/dev/stdout`
    /// is already missing or not a symbolic link (an earlier test or tool
    /// broke it).
    #[must_use]
    pub fn new(test: &'static str) -> Self {
        let mut guard = Self { test, remembered: None, checked: false };
        if !Path::new("/proc/self/fd").is_dir() {
            eprintln!("{test}: no /proc/self/fd: /dev/stdout is not guarded");
            return guard;
        }
        let found = classify(Path::new(DEV_STDOUT));
        announce(&found);
        match found {
            Found::Healthy(healthy) => guard.remembered = Some(healthy),
            Found::Broken(now) => {
                guard.checked = true;
                panic!(
                    "{test} (before it ran: an earlier process): /dev/stdout is not a symbolic \
                     link to /proc/self/fd/1 (a tool or the linker replaced or removed it): it \
                     is {now}; {}",
                    restored(restore())
                );
            }
            Found::Foreign(foreign) => eprintln!(
                "{test}: /dev/stdout is a symbolic link to {}, not through the \
                 descriptor table: left alone, not guarded",
                foreign.display()
            ),
        }
        guard
    }

    fn verify(&self) -> Result<(), String> {
        let Some(remembered) = &self.remembered else { return Ok(()) };
        let now = match unchanged(Path::new(DEV_STDOUT), remembered) {
            Ok(()) => return Ok(()),
            Err(now) => now,
        };
        let what = if remembered == Path::new(TARGET) {
            restored(restore())
        } else {
            "not restored (it was not `/proc/self/fd/1`): restore it by hand"
        };
        Err(format!(
            "{}: /dev/stdout is no longer the symbolic link to {} it was (a tool or \
             the linker replaced or removed it): it is {now}; {what}",
            self.test,
            remembered.display()
        ))
    }

    /// The test's last assertion: `/dev/stdout` is unchanged; if not the
    /// test fails (and the link is restored when it was `/proc/self/fd/1`).
    pub fn check(mut self) {
        self.checked = true;
        if let Err(m) = self.verify() {
            panic!("{m}");
        }
    }
}

impl Drop for Guard {
    /// A test that failed before [`Guard::check`] still checks and
    /// restores (it fails anyway; no second panic during unwinding).
    fn drop(&mut self) {
        if !self.checked
            && let Err(m) = self.verify()
        {
            if std::thread::panicking() {
                eprintln!("{m}");
            } else {
                panic!("{m}");
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{Found, classify, unchanged};
    use crate::Scratch;
    use std::os::unix::fs::symlink;
    use std::path::Path;

    #[test]
    fn the_decision_on_scratch_links() {
        if !Path::new("/proc/self/fd").is_dir() {
            return;
        }
        let scratch = Scratch::new("dev-stdout-classify");
        let d = scratch.path();
        // Healthy: the usual text.
        let usual = d.join("usual");
        symlink("/proc/self/fd/1", &usual).unwrap();
        assert_eq!(classify(&usual), Found::Healthy("/proc/self/fd/1".into()));
        assert_eq!(unchanged(&usual, Path::new("/proc/self/fd/1")), Ok(()));
        // Healthy: the `fd/1` style, beside `fd -> /proc/self/fd`.
        symlink("/proc/self/fd", d.join("fd")).unwrap();
        let short = d.join("short");
        symlink("fd/1", &short).unwrap();
        assert_eq!(classify(&short), Found::Healthy("fd/1".into()));
        assert_eq!(unchanged(&short, Path::new("fd/1")), Ok(()));
        // Missing.
        let missing = d.join("missing");
        assert!(matches!(classify(&missing), Found::Broken(m) if m.starts_with("missing")));
        assert!(unchanged(&missing, Path::new("/proc/self/fd/1")).is_err());
        // A regular file.
        let file = d.join("file");
        std::fs::write(&file, b"a map\n").unwrap();
        assert!(matches!(classify(&file), Found::Broken(m) if m.contains("regular file")));
        assert!(unchanged(&file, Path::new("/proc/self/fd/1")).is_err());
        // A changed target: foreign at the start, a failure at the end.
        let changed = d.join("changed");
        symlink("elsewhere", &changed).unwrap();
        assert_eq!(classify(&changed), Found::Foreign("elsewhere".into()));
        let now = unchanged(&changed, Path::new("/proc/self/fd/1")).unwrap_err();
        assert!(now.contains("elsewhere"), "{now}");
    }
}
