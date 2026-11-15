//! Where the phase's time goes, per sub-step, and the work it counted:
//! what `MORTAR_TIME_STEPS` prints (the bench's unit costs). Never part of
//! a result: the clock decides nothing.

use std::time::{Duration, Instant};

/// Sub-step times and work counts, in the order they were taken.
#[derive(Clone, Debug)]
pub struct Steps {
    pub times: Vec<(&'static str, Duration)>,
    pub counts: Vec<(&'static str, u64)>,
    last: Instant,
}

impl Default for Steps {
    fn default() -> Self {
        Self { times: Vec::new(), counts: Vec::new(), last: Instant::now() }
    }
}

impl Steps {
    /// The step `name` ended now (it began when the last one ended).
    pub fn step(&mut self, name: &'static str) {
        let now = Instant::now();
        self.times.push((name, now - self.last));
        self.last = now;
    }

    /// A step measured elsewhere.
    pub fn time(&mut self, name: &'static str, d: Duration) {
        self.times.push((name, d));
    }

    /// Restarts the clock without recording a step.
    pub fn restart(&mut self) {
        self.last = Instant::now();
    }

    /// `n` units of the work `name`.
    pub fn count(&mut self, name: &'static str, n: u64) {
        self.counts.push((name, n));
    }
}
