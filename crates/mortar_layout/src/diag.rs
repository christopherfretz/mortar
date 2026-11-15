//! What layout reports (`spec/layout.md` §20.4).
//!
//! Refusals, `invalid-option-value` and `internal-error` exist in
//! `mortar_diag` and are built as [`Diagnostic`]s. The three kinds §20.4
//! adds (`section-conflict`, `output-too-large`, `executable-stack`) are
//! not in `mortar_diag` yet: until they are, they are the variants of
//! [`LayoutDiag`] below, with the message text §20.4 gives, so that moving
//! them into `mortar_diag` changes no wording.

use mortar_diag::{Diagnostic, Kind, Place, RefusalClass};

/// One thing layout reports.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LayoutDiag {
    /// A diagnostic of a kind `mortar_diag` has: `refused`,
    /// `invalid-option-value`, `internal-error`.
    Diag(Box<Diagnostic>),
    /// `section-conflict` (error, L-47, L-49, L-61): two live inputs of one
    /// output section that cannot be combined. `first`/`second` are the
    /// class words (`TLS`, `non-TLS`, `allocated`, `non-allocated`) or
    /// `SHT_` names.
    SectionConflict { name: String, first: String, second: String, place: Option<Box<Place>> },
    /// `output-too-large` (error, L-73).
    OutputTooLarge { what: String, limit: String },
    /// `executable-stack` (warning, L-84).
    ExecutableStack { file: String },
}

impl LayoutDiag {
    /// The stable id (`spec/diag.md` §3.1).
    #[must_use]
    pub fn id(&self) -> &'static str {
        match self {
            Self::Diag(d) => d.id(),
            Self::SectionConflict { .. } => "section-conflict",
            Self::OutputTooLarge { .. } => "output-too-large",
            Self::ExecutableStack { .. } => "executable-stack",
        }
    }

    /// Whether the link fails.
    #[must_use]
    pub fn is_error(&self) -> bool {
        match self {
            Self::Diag(d) => d.severity() == mortar_diag::Severity::Error,
            Self::SectionConflict { .. } | Self::OutputTooLarge { .. } => true,
            Self::ExecutableStack { .. } => false,
        }
    }

    /// The message, in the words of §20.4 (and GNU ld's for
    /// `executable-stack`).
    #[must_use]
    pub fn message(&self) -> String {
        match self {
            Self::Diag(d) => format!("{:?}", d.kind()),
            Self::SectionConflict { name, first, second, .. } => {
                format!("section {name}: {first} and {second} inputs cannot be combined")
            }
            Self::OutputTooLarge { what, limit } => {
                format!("output too large: {what} exceeds {limit}")
            }
            Self::ExecutableStack { file } => format!(
                "{file}: requires executable stack (because the .note.GNU-stack section is executable)"
            ),
        }
    }

    /// The refusal class and name, for a refusal.
    #[must_use]
    pub fn refusal(&self) -> Option<(RefusalClass, &str)> {
        match self {
            Self::Diag(d) => d.refusal(),
            _ => None,
        }
    }
}

impl From<Diagnostic> for LayoutDiag {
    fn from(d: Diagnostic) -> Self {
        Self::Diag(Box::new(d))
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
