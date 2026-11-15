//! What layout reports (`spec/layout.md` §20.4).
//!
//! Every report is a `mortar_diag` [`Diagnostic`]; the three kinds
//! §20.4 adds (`section-conflict`, `output-too-large`, `executable-stack`)
//! are `mortar_diag` kinds (`spec/diag.md` §3.1, the layout area).

use mortar_core::id::{ChunkId, SectionId};
use mortar_diag::{Diagnostic, Key, Kind, RefusalClass};

/// The phase ordinal of layout (`spec/model.md` §8).
pub const PHASE: u16 = 90;

/// One thing layout reports.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LayoutDiag {
    /// A diagnostic: `refused`, `invalid-option-value`, `internal-error`,
    /// `entry-not-found`, `section-conflict`, `output-too-large`,
    /// `executable-stack`; with its key when layout knows one
    /// (`spec/model.md` §8: `(90, 0, ChunkId)`), and the input section
    /// and offset it is about, which the caller (who has the file names)
    /// turns into the diagnostic's place.
    Diag { diagnostic: Box<Diagnostic>, key: Option<Key>, at: Option<(SectionId, u64)> },
}

impl LayoutDiag {
    /// The diagnostic.
    #[must_use]
    pub fn diagnostic(&self) -> &Diagnostic {
        let Self::Diag { diagnostic, .. } = self;
        diagnostic
    }

    /// The key `(90, 0, ChunkId)` of a diagnostic about one chunk.
    #[must_use]
    pub fn key(&self) -> Option<Key> {
        let Self::Diag { key, .. } = self;
        *key
    }

    /// The input section and offset the diagnostic is about.
    #[must_use]
    pub fn at(&self) -> Option<(SectionId, u64)> {
        let Self::Diag { at, .. } = self;
        *at
    }

    /// The diagnostic, keyed on `chunk` and about `at`.
    #[must_use]
    pub fn on_chunk(self, chunk: ChunkId, at: Option<(SectionId, u64)>) -> Self {
        let Self::Diag { diagnostic, .. } = self;
        Self::Diag { diagnostic, key: Some(Key::new(PHASE, 0, u64::from(chunk.0))), at }
    }

    /// The diagnostic, consumed.
    #[must_use]
    pub fn into_diagnostic(self) -> Diagnostic {
        let Self::Diag { diagnostic, .. } = self;
        *diagnostic
    }

    /// The stable id (`spec/diag.md` §3.1).
    #[must_use]
    pub fn id(&self) -> &'static str {
        self.diagnostic().id()
    }

    /// Whether the link fails.
    #[must_use]
    pub fn is_error(&self) -> bool {
        self.diagnostic().severity() == mortar_diag::Severity::Error
    }

    /// The message as `mortar_diag` renders it, without program name or
    /// severity.
    #[must_use]
    pub fn message(&self) -> String {
        let full = self.diagnostic().render(&mortar_diag::Style::plain(""));
        let body = full.trim_end_matches('\n');
        for prefix in [": error: ", ": warning: "] {
            if let Some(rest) = body.strip_prefix(prefix) {
                return rest.to_owned();
            }
        }
        body.to_owned()
    }

    /// The refusal class and name, for a refusal.
    #[must_use]
    pub fn refusal(&self) -> Option<(RefusalClass, &str)> {
        self.diagnostic().refusal()
    }
}

impl From<Diagnostic> for LayoutDiag {
    fn from(d: Diagnostic) -> Self {
        Self::Diag { diagnostic: Box::new(d), key: None, at: None }
    }
}

/// `refused`, class `feature`, by name.
pub(crate) fn feature(name: impl Into<String>) -> LayoutDiag {
    LayoutDiag::from(Diagnostic::refused(RefusalClass::Feature, name))
}

/// `refused` of another class.
pub(crate) fn refused(class: RefusalClass, name: impl Into<String>) -> LayoutDiag {
    LayoutDiag::from(Diagnostic::refused(class, name))
}

/// `internal-error`: a table mortar computed breaks its own rules.
pub(crate) fn internal(what: impl Into<String>) -> LayoutDiag {
    LayoutDiag::from(Diagnostic::error(Kind::InternalError { what: what.into() }))
}

/// resolve.md's `entry-not-found` (warning, L-103).
pub(crate) fn entry_not_found(name: &[u8]) -> LayoutDiag {
    LayoutDiag::from(Diagnostic::warning(Kind::EntryNotFound { name: text(name) }))
}

/// `section-conflict` (error, L-47, L-49, L-61): two live inputs of
/// output section `name` that cannot be combined.
pub(crate) fn section_conflict(name: String, first: &str, second: &str) -> LayoutDiag {
    LayoutDiag::from(Diagnostic::error(Kind::SectionConflict {
        name,
        first: first.to_owned(),
        second: second.to_owned(),
    }))
}

/// `output-too-large` (error, L-73).
pub(crate) fn output_too_large(what: &str, limit: u128) -> LayoutDiag {
    LayoutDiag::from(Diagnostic::error(Kind::OutputTooLarge {
        what: what.to_owned(),
        limit: format!("{limit:#x}"),
    }))
}

/// `executable-stack` (warning, L-84), naming `file`.
#[must_use]
pub fn executable_stack(file: &str) -> LayoutDiag {
    LayoutDiag::from(Diagnostic::warning(Kind::ExecutableStack { file: file.to_owned() }))
}

/// `invalid-option-value`.
pub(crate) fn invalid_option(option: &str, value: &str) -> LayoutDiag {
    LayoutDiag::from(Diagnostic::error(Kind::InvalidOptionValue {
        option: option.to_owned(),
        value: value.to_owned(),
    }))
}

/// Bytes as text for a message (U+FFFD for what is not UTF-8).
pub(crate) fn text(bytes: &[u8]) -> String {
    String::from_utf8_lossy(bytes).into_owned()
}
