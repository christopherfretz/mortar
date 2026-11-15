//! Sub-step timers of the split, merge and scan phases (WP-032b): when
//! the environment variable [`STEPS_VAR`] is set (to anything), each
//! [`Steps::step`] prints one line to standard error,
//!
//! `mortar: steps: <phase> <step>: <ms> ms[, <n> <unit>, <ns> ns/<unit>]`
//!
//! Off, a step is one branch on a cached flag. Nothing a step prints
//! reaches the output file, the map or a diagnostic; it is for profiling
//! and `cargo xtask bench` only. (Prefixed for WP-032b: the supervisor
//! reconciles helpers of the same kind at merge.)

use std::fmt::Write as _;
use std::sync::OnceLock;
use std::time::Instant;

/// The environment variable that turns the step lines on.
pub const STEPS_VAR: &str = "MORTAR_STEPS";

/// Whether [`STEPS_VAR`] is set, read once per process.
#[must_use]
pub fn enabled() -> bool {
    static ON: OnceLock<bool> = OnceLock::new();
    *ON.get_or_init(|| std::env::var_os(STEPS_VAR).is_some())
}

/// The timer of one phase's sub-steps.
#[derive(Debug)]
pub struct Steps {
    phase: &'static str,
    last: Option<Instant>,
}

impl Steps {
    /// Starts timing `phase` when [`enabled`].
    #[must_use]
    pub fn new(phase: &'static str) -> Self {
        Self { phase, last: enabled().then(Instant::now) }
    }

    /// Whether the steps print.
    #[must_use]
    pub const fn on(&self) -> bool {
        self.last.is_some()
    }

    /// The step `name` ended now; `n` units of `unit` were done in it
    /// (`n` 0: no unit cost is printed).
    #[allow(clippy::cast_precision_loss)] // a printed duration
    pub fn step(&mut self, name: &str, n: u64, unit: &str) {
        let Some(last) = self.last else { return };
        let now = Instant::now();
        let ns = now.duration_since(last).as_nanos();
        let mut line = format!("mortar: steps: {} {name}: {:.2} ms", self.phase, ns as f64 / 1e6);
        if n != 0 {
            let _ = write!(line, ", {n} {unit}, {:.1} ns/{unit}", ns as f64 / n as f64);
        }
        eprintln!("{line}");
        self.last = Some(Instant::now());
    }
}
