//! `--time-phases` (`spec/image.md` §11, I-88, I-89): the lines of the
//! phases that ran, printed after the diagnostics.

use mortar_image::phases::PhaseTimer;

/// A phase timer when `--time-phases` asks for one.
#[derive(Debug, Clone)]
pub struct Timer(Option<PhaseTimer>);

impl Timer {
    /// Starts timing when `on`.
    #[must_use]
    pub fn new(on: bool) -> Self {
        Self(on.then(PhaseTimer::start))
    }

    /// The phase `ordinal name` (model §8's names) ended now.
    pub fn phase(&mut self, ordinal: u16, name: &str) {
        if let Some(t) = &mut self.0 {
            t.phase(ordinal, name);
        }
    }

    /// The lines, each with its `mortar: time-phases: ` prefix and a
    /// newline; empty when not timing.
    #[must_use]
    pub fn finish(self) -> String {
        self.0.map(PhaseTimer::finish).unwrap_or_default().iter().fold(String::new(), |mut s, l| {
            s.push_str(l);
            s.push('\n');
            s
        })
    }
}
