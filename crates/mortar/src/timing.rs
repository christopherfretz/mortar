//! `--time-phases` (`spec/image.md` §11, I-88, I-89): the lines of the
//! phases that ran, printed after the diagnostics; and the lines a phase
//! prints there on request whether or not it is timed (`--odr-stats`,
//! `spec/odr.md` O-44).

use mortar_image::phases::PhaseTimer;

/// A phase timer when `--time-phases` asks for one.
#[derive(Debug, Clone)]
pub struct Timer(Option<PhaseTimer>, Vec<String>);

impl Timer {
    /// Starts timing when `on`.
    #[must_use]
    pub fn new(on: bool) -> Self {
        Self(on.then(PhaseTimer::start), Vec::new())
    }

    /// Whether phases are timed (`--time-phases`).
    #[must_use]
    pub fn on(&self) -> bool {
        self.0.is_some()
    }

    /// The phase `ordinal name` (model §8's names) ended now.
    pub fn phase(&mut self, ordinal: u16, name: &str) {
        if let Some(t) = &mut self.0 {
            t.phase(ordinal, name);
        }
    }

    /// A line printed after the diagnostics, before the phases' (its
    /// own prefix included, no newline).
    pub fn line(&mut self, line: String) {
        self.1.push(line);
    }

    /// The lines given to [`Timer::line`], then the phases' lines, each
    /// with its `mortar: time-phases: ` prefix; each ends with a newline.
    /// Empty when not timing and no line was given.
    #[must_use]
    pub fn finish(self) -> String {
        let phases = self.0.map(PhaseTimer::finish).unwrap_or_default();
        self.1.iter().chain(phases.iter()).fold(String::new(), |mut s, l| {
            s.push_str(l);
            s.push('\n');
            s
        })
    }
}
