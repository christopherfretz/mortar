//! Delta resolution (`spec/incr.md` IN-71, IN-71c; IN-70 step U5): the
//! affected keys re-resolved from their `KEY_REFS` rows and the changed
//! files' fresh mentions, COMDAT groups re-elected, commons re-chosen.
//! WP-213 (U-1): the type [`ResolveDelta`] and the entry point
//! [`resolve`]; **filled by U-3** (the function only).

use mortar_core::incr::{IncrReason, Unimplemented};
use mortar_core::{FileId, SymbolId};

use super::extract::Extraction;
use super::rows::{CommonRow, GroupRow, KeyRefRow, ResolvedRow, SymbolRow};
use super::view::{ChangedFiles, KeyEdits, StateView};

/// A key new to the state (no `SYMBOLS` row, so `StateRows::find_key`
/// found none): its id is the state's `SYMBOLS` count plus its position in
/// [`ResolveDelta::new_keys`], and the patch puts its `NAMES` row. A `GONE`
/// key that comes back is not new: `find_key` returns its old row, which
/// is affected and set again (IN-18e).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NewKey {
    /// Its `SYMBOLS` row as appended (name and version as strings the
    /// patch appends: U-2 interns them).
    pub row: SymbolRow,
    pub name: Vec<u8>,
    pub version: Vec<u8>,
}

/// U-3's resolution delta (WP-211 §C `ResolveDelta`; IN-71, IN-71c).
/// Written by U-3; read by U-3 (`live`, `refill_set`), U-4 (through the
/// join's `SectionChange`s: `kept`, commons), U-5 (the globals whose entry
/// changes), U-8, U-9, U-2 (its rows in the patch).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ResolveDelta {
    /// The affected keys (IN-71: every key a changed, appeared, gone,
    /// drawn-in, extracted or no longer extracted file mentions, old or new
    /// version, and the linker-defined keys of IN-71c), sorted.
    pub affected: Vec<SymbolId>,
    /// New keys, in the order their rows are appended.
    pub new_keys: Vec<NewKey>,
    /// Per changed file (`ChangedFiles::files` order), its
    /// `ChangedFile::keys` mapped to `SymbolId`s (position for position).
    pub key_ids: Vec<(FileId, Vec<SymbolId>)>,
    /// Each affected key's new `RESOLVED` row (sorted by key).
    pub resolved: Vec<(SymbolId, ResolvedRow)>,
    /// Keys no file mentions any more (`SYMBOLS.kind` `GONE`).
    pub gone: Vec<SymbolId>,
    /// `at_file`, `at_index` of keys whose first mention changed (IN-71).
    pub at: Vec<(SymbolId, u32, u32)>,
    /// `KEY_REFS`, `GROUPS`, `COMMONS` edits.
    pub key_refs: KeyEdits<KeyRefRow>,
    pub groups: KeyEdits<GroupRow>,
    pub commons: KeyEdits<CommonRow>,
    /// Groups whose keeper changed: `(signature, old keeper, new keeper)`
    /// (`FILES` rows; IN-72 item 2a's drawn-in files, O-21).
    pub reelected: Vec<(Vec<u8>, FileId, FileId)>,
}

/// Resolve the affected keys (WP-211 §C; IN-71). Called by the join after
/// `extract` (IN-70 U5).
///
/// # Errors
/// A §7 reason (`symbol-kind-changed`, `state-corrupt`, ...). Until U-3
/// lands: `update-not-implemented (resolve)`.
pub fn resolve(
    view: &StateView<'_>,
    changed: &ChangedFiles,
    extraction: &Extraction,
) -> Result<ResolveDelta, IncrReason> {
    let _ = (view, changed, extraction);
    Err(IncrReason::UpdateNotImplemented(Some(Unimplemented::Resolve)))
}
