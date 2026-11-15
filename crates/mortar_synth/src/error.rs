//! Diagnostics of phase 80 (`spec/model.md` §8; `spec/synth.md` §1
//! "Diagnostics").

use mortar_core::SectionId;
use mortar_diag::{Collector, Diagnostic, Key, Kind, RefusalClass};

/// The phase ordinal of synthetic sections (`spec/model.md` §8).
pub const PHASE: u16 = 80;

/// A plan or write step failed; its errors are in the collector it was
/// given.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Failed;

/// The key of a diagnostic about a symbol or a count (unit 0).
#[must_use]
pub fn key(seq: u64) -> Key {
    Key::new(PHASE, 0, seq)
}

/// The key of a diagnostic about record `record` of input `.eh_frame`
/// section `section`: `seq = SectionId × 2^32 + record`.
#[must_use]
pub fn record_key(section: SectionId, record: u32) -> Key {
    key(u64::from(section.0) << 32 | u64::from(record))
}

/// An error of a write step or a lookup (`spec/image.md` I-106): the
/// diagnostic, which the caller places and keys (`(100, 0, ChunkId)` in
/// the image phase).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SynthError(pub Box<Diagnostic>);

impl From<Diagnostic> for SynthError {
    fn from(d: Diagnostic) -> Self {
        Self(Box::new(d))
    }
}

impl SynthError {
    /// The diagnostic.
    #[must_use]
    pub fn into_diagnostic(self) -> Diagnostic {
        *self.0
    }
}

/// `malformed-input` with `what` (synth.md §1: lld's wording).
#[must_use]
pub fn malformed<E: From<Diagnostic>>(what: impl Into<String>) -> E {
    Diagnostic::error(Kind::Malformed { what: what.into() }).into()
}

/// `refused` of class `feature` (`spec/diag.md` §4).
#[must_use]
pub fn feature(name: impl Into<String>) -> Diagnostic {
    Diagnostic::refused(RefusalClass::Feature, name)
}

/// `internal-error`: a table broke an invariant this crate relies on.
#[must_use]
pub fn internal<E: From<Diagnostic>>(what: impl Into<String>) -> E {
    Diagnostic::error(Kind::InternalError { what: what.into() }).into()
}

/// Emits `d` under `k` and fails.
pub fn fail<T>(diag: &Collector, k: Key, d: Diagnostic) -> Result<T, Failed> {
    diag.emit(k, d);
    Err(Failed)
}
