//! Diagnostics equal to a full link's (`spec/incr.md` IN-75a): the stored
//! warnings (`DIAGS`, kind 49) dropped by subject, recomputed, and emitted
//! in `spec/diag.md` §6.2's report order. WP-213 (U-1): the types
//! [`DiagRow`] and [`Subject`] and the entry point [`diags`]; **filled by
//! U-9** (the function only; the types are U-1's).

use mortar_core::{FileId, OutSecId, SymbolId};

use super::resolve::ResolveDelta;
use super::view::{ChangedFiles, StateView};

/// One subject of a stored warning (IN-75a): what the warning's text or
/// its existence depends on. The kinds are `mortar_diag::subject`'s
/// `SubjectKind` (diag.md's subjects column).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Subject {
    /// A file, by its `FILES` row.
    File(FileId),
    /// A key, by its `SYMBOLS` row.
    Key(SymbolId),
    /// An output section, by its `OUTSECS` row (recomputed every update).
    OutputSection(OutSecId),
    /// The link (recomputed every update).
    Link,
}

/// `DIAGS`' row (WP-211 §C; kind 49, key-keyed; IN-75a): one warning of
/// the link that wrote the state. Key: `key`, then `code` and `text`
/// (diag.md §6.2's tie-breaks). Written by capture (every warning of a
/// full link in incremental mode) and U-2 (from [`diags`]' result); read
/// by U-9. U-2 encodes `subjects`, `code` and `text` (variable length)
/// through `STRINGS`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DiagRow {
    /// Every subject (plural, IN-75a), sorted.
    pub subjects: Vec<Subject>,
    /// The warning code (`mortar_diag::Kind::id`).
    pub code: String,
    /// The rendered text, as the report prints it.
    pub text: String,
    /// diag.md §6.1's `Key`, `unit` stored as the file's `FILES` row (never
    /// an ordinal; mapped at emission to the full link's unit, the file's
    /// position in `FILES.rank` order).
    pub key: mortar_diag::Key,
    pub tomb: bool,
}

/// The warnings of the update (WP-211 §C; IN-75a): the stored rows of
/// `view` with no changed subject, united with `fresh` (the recomputed
/// warnings of the changed subjects, and U-8's ODR findings), in diag.md
/// §6.2's report order. Called by the join (U-10) after U7; its result is
/// what the update prints and what the patch stores.
///
/// Until U-9 lands: `fresh`, unchanged.
#[must_use]
pub fn diags(
    view: &StateView<'_>,
    changed: &ChangedFiles,
    resolved: &ResolveDelta,
    fresh: Vec<DiagRow>,
) -> Vec<DiagRow> {
    let _ = (view, changed, resolved);
    fresh
}
