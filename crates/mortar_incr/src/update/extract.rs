//! Archive extraction in an update (`spec/incr.md` IN-71b; IN-70 step
//! U5): members newly extracted and no longer extracted, with their
//! rounds. WP-213 (U-1): the type [`Extraction`] and the entry point
//! [`extract`]; **filled by U-3** (the function only).

use mortar_core::FileId;
use mortar_core::incr::{IncrReason, OptionKeys, Unimplemented};

use super::view::{ChangedFiles, StateView};

/// The members whose extraction changed (WP-211 §C `Extraction`; IN-71b).
/// Written by U-3; read by U-3 (`resolve`), the join (an extracted member
/// is read by the front and joins `ChangedFiles` as `Extracted`), U-2
/// (`FILES` flags and rounds).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Extraction {
    /// `LAZY` members now extracted and their round (`FILES` rows),
    /// sorted.
    pub extracted: Vec<(FileId, u32)>,
    /// Members no longer extracted (they become `LAZY`), sorted.
    pub unextracted: Vec<FileId>,
    /// Members still extracted whose stored round changed (any supporting
    /// value, IN-71b), sorted.
    pub rounds: Vec<(FileId, u32)>,
}

/// The extraction of the update (WP-211 §C; IN-71b). Called by the join
/// before `resolve` (IN-70 U5). `options`: the link's root keys (§4.2's
/// root edges, IN-71b), passed in (WP-215 D3-D5).
///
/// # Errors
/// `extraction-changed`, `closure-iterated`, `update-not-implemented
/// (versioned)`, `state-corrupt`. Until U-3 lands:
/// `update-not-implemented (resolve)`.
pub fn extract(
    view: &StateView<'_>,
    changed: &ChangedFiles,
    options: &OptionKeys,
) -> Result<Extraction, IncrReason> {
    let _ = (view, changed, options);
    Err(IncrReason::UpdateNotImplemented(Some(Unimplemented::Resolve)))
}
