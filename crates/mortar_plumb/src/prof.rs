//! Phase marks for measuring the verifier itself (WP-109): with
//! `MORTAR_PLUMB_PROFILE` set in the environment, each [`mark`] records
//! the time since the first mark, the resident set and the peak resident
//! set since the previous mark (`/proc/self/status` `VmRSS` and `VmHWM`;
//! the peak is reset by writing `5` to `/proc/self/clear_refs`), and each
//! [`gauge`] the size of a structure; a value other than `1` is a file
//! each line is appended to as well. Without the variable both are a
//! test of a flag. The caller prints [`take`]'s lines (`xtask plumb`, on
//! standard error). Nothing of the verdict depends on them.

use std::sync::Mutex;
use std::sync::OnceLock;
use std::time::Instant;

struct State {
    start: Instant,
    lines: Vec<String>,
    /// The file each line is appended to as it is recorded (the
    /// variable's value, when it is not `1`): a run killed at a memory
    /// cap keeps its marks.
    file: Option<std::path::PathBuf>,
}

impl State {
    fn push(&mut self, line: String) {
        if let Some(f) = &self.file {
            use std::io::Write;
            if let Ok(mut h) = std::fs::OpenOptions::new().create(true).append(true).open(f) {
                let _ = writeln!(h, "{line}");
            }
        }
        self.lines.push(line);
    }
}

fn state() -> Option<&'static Mutex<State>> {
    static ON: OnceLock<Option<Mutex<State>>> = OnceLock::new();
    ON.get_or_init(|| {
        std::env::var_os("MORTAR_PLUMB_PROFILE").map(|v| {
            let file = (v != "1" && !v.is_empty()).then(|| std::path::PathBuf::from(v));
            Mutex::new(State { start: Instant::now(), lines: Vec::new(), file })
        })
    })
    .as_ref()
}

/// Whether marks are recorded.
pub fn enabled() -> bool {
    state().is_some()
}

fn status_kib(field: &str) -> u64 {
    let Ok(s) = std::fs::read_to_string("/proc/self/status") else { return 0 };
    s.lines()
        .find_map(|l| l.strip_prefix(field))
        .and_then(|v| v.trim().trim_end_matches("kB").trim().parse().ok())
        .unwrap_or(0)
}

/// Records the end of a phase.
pub fn mark(phase: &str) {
    let Some(m) = state() else { return };
    let rss = status_kib("VmRSS:");
    let hwm = status_kib("VmHWM:");
    // Resets the peak to the current resident set (proc(5), clear_refs).
    let _ = std::fs::write("/proc/self/clear_refs", "5");
    if let Ok(mut s) = m.lock() {
        let ms = s.start.elapsed().as_millis();
        s.push(format!(
            "[plumb-profile] {ms:>8} ms  rss {:>6} MiB  peak {:>6} MiB  after {phase}",
            rss / 1024,
            hwm / 1024
        ));
    }
}

/// Records the size of a structure, in bytes (an estimate: the lengths
/// of its vectors by their element sizes, and its strings).
pub fn gauge(name: &str, bytes: u64) {
    let Some(m) = state() else { return };
    if let Ok(mut s) = m.lock() {
        s.push(format!("[plumb-profile] size {name}: {} MiB ({bytes} B)", bytes >> 20));
    }
}

/// The lines recorded so far, emptied.
pub fn take() -> Vec<String> {
    state()
        .and_then(|m| m.lock().ok())
        .map(|mut s| std::mem::take(&mut s.lines))
        .unwrap_or_default()
}
