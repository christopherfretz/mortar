//! Diagnostics equal to a full link's (`spec/incr.md` IN-75a): the stored
//! warnings (`DIAGS`, kind 49) dropped by subject, recomputed, and emitted
//! in `spec/diag.md` §6.2's report order. WP-213 (U-1): the types
//! [`DiagRow`] and [`Subject`] and the entry point [`diags`]; **filled by
//! U-9** (the function only; the types are U-1's).

use mortar_core::incr::IncrReason;
use mortar_core::{FileId, OutSecId, SymbolId};
use mortar_diag::subject::{from_stored, stored};
use mortar_diag::{Context, Diagnostic, Relation};

use crate::format::Str;

use super::odr::OdrUpdate;
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
    /// An ODR key (odr.md O-4: name and class; WP-212 F-12), for a
    /// finding whose subject is an entity rather than a symbol. Its kind
    /// is `SubjectKind::Key`. **PROVISIONAL (U-8's design step, WP-213
    /// follow-up F-5)**: carries the name's string in `STRINGS` and the
    /// class until U-8's ruling numbers ODR keys; then it may carry that
    /// number instead.
    OdrKey {
        /// The mangled name with its `.symver` version (O-4), in `STRINGS`.
        name: Str,
        /// [`odr_class`].
        class: u8,
    },
    /// An output section, by its `OUTSECS` row (recomputed every update).
    OutputSection(OutSecId),
    /// The link (recomputed every update).
    Link,
}

/// An ODR key's class (odr.md O-4: function, variable or type).
/// **PROVISIONAL (U-8's design step)**, with [`Subject::OdrKey`].
pub mod odr_class {
    pub const FUNCTION: u8 = 0;
    pub const VARIABLE: u8 = 1;
    pub const TYPE: u8 = 2;
}

/// One reference of a reference-site warning (`gnu-warning` of a symbol,
/// `undefined-symbol` under `--warn-unresolved-symbols`; WP-221 seam 7):
/// a relocation that refers to the warning's key.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct DiagRef {
    /// The referring file's `FILES` row.
    pub file: FileId,
    /// The scan's seq of the reference (`spec/model.md` §8, phase 70):
    /// section index × 2^32 + relocation index in that section.
    pub seq: u64,
    /// The reference's offset in its section (the place's `+0x..`).
    pub offset: u64,
}

impl DiagRef {
    /// The referring section's index in its file.
    #[must_use]
    pub const fn section(&self) -> u32 {
        (self.seq >> 32) as u32
    }
}

/// `DIAGS`' row (WP-211 §C; kind 49, key-keyed; IN-75a): one warning of
/// the link that wrote the state. Key: `key`, then `code` and `text`
/// (diag.md §6.2's tie-breaks). Written by capture (every warning of a
/// full link in incremental mode) and U-2 (from [`diags`]' result); read
/// by U-9. U-2 encodes `subjects`, `code`, `text` and `refs` (variable
/// length) through `STRINGS`. Made by [`row`] only; read back by
/// [`diagnostic`] (WP-221 seam 7).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DiagRow {
    /// Every subject (plural, IN-75a), sorted.
    pub subjects: Vec<Subject>,
    /// The warning code (`mortar_diag::Kind::id`).
    pub code: String,
    /// The warning's stored form (`mortar_diag::subject::stored`): the
    /// model, never a rendering (WP-221 §1.1; IN-75a).
    pub text: String,
    /// diag.md §6.1's `Key`, `unit` stored as the file's `FILES` row (never
    /// an ordinal; mapped at emission to the full link's unit, the file's
    /// position in `FILES.rank` order).
    pub key: mortar_diag::Key,
    /// A reference-site warning's references, sorted, at most
    /// [`REFS_CAP`]: every one when `refs.len() == ref_count` (the row is
    /// exact under any change), else the first ones (capped). Empty for
    /// every other code.
    pub refs: Vec<DiagRef>,
    /// The reference-site warning's references in all (its listed places
    /// plus "referenced N more times"); 0 for every other code.
    pub ref_count: u64,
    pub tomb: bool,
}

/// The most references a `DIAGS` row stores (WP-221 ruling 3).
pub const REFS_CAP: usize = 64;

/// Whether `code` is a reference-site warning's: its places are the
/// references to its key (`mortar_scan`'s `with_refs`), the first three
/// listed and the rest counted.
#[must_use]
pub fn reference_site(code: &str) -> bool {
    matches!(code, "gnu-warning" | "undefined-symbol")
}

/// The note `with_refs` adds after three listed references.
const MORE: (&str, &str) = ("referenced ", " more times");

/// A reference-site diagnostic's references in all: its listed
/// `referenced by` places plus the note's count. `None` when its context
/// is not `with_refs`' (a place of another relation, or another note).
#[must_use]
pub fn reference_count(d: &Diagnostic) -> Option<u64> {
    let mut listed = 0u64;
    let mut more = 0u64;
    for c in d.context() {
        match c {
            Context::At(Relation::ReferencedBy, _) if more == 0 => listed += 1,
            Context::Note(n) if more == 0 && listed == 3 => {
                more = n.strip_prefix(MORE.0)?.strip_suffix(MORE.1)?.parse().ok()?;
                if more == 0 {
                    return None;
                }
            }
            _ => return None,
        }
    }
    Some(listed + more)
}

/// The row of the warning `d` with its key and subjects (WP-221 seam 7;
/// capture and every step of an update): `None` for a diagnostic that is
/// not a warning (a state holds warnings only, IN-75a). `refs`: a
/// reference-site warning's references (sorted and capped here), empty for
/// any other code; `ref_count` is the diagnostic's own.
#[must_use]
pub fn row(
    key: mortar_diag::Key,
    d: &Diagnostic,
    mut subjects: Vec<Subject>,
    mut refs: Vec<DiagRef>,
) -> Option<DiagRow> {
    let text = stored(d)?;
    let code = d.id().to_owned();
    let ref_count = if reference_site(&code) { reference_count(d)? } else { 0 };
    refs.sort_unstable();
    refs.dedup();
    if ref_count == 0 && !refs.is_empty() {
        return None;
    }
    refs.truncate(REFS_CAP.min(usize::try_from(ref_count).unwrap_or(usize::MAX)));
    subjects.sort_unstable();
    subjects.dedup();
    Some(DiagRow { subjects, code, text, key, refs, ref_count, tomb: false })
}

/// The warning a row stores (WP-221 seam 7; the update's report): `None`
/// when `text` is not a stored form of `code` (`state-corrupt`; a state
/// written by a build of mortar whose model differs ends here).
#[must_use]
pub fn diagnostic(row: &DiagRow) -> Option<Diagnostic> {
    from_stored(&row.code, &row.text)
}

/// The warnings of the update (WP-211 §C; IN-75a): the stored rows of
/// `view` with no changed subject, united with `fresh` (the recomputed
/// warnings of the changed subjects) and `odr.rows` (U-8's, replacing
/// every stored ODR row of a key in `odr.keys`), in diag.md §6.2's report
/// order. Called by the join (U-10) after U7; its result is what the
/// update prints and what the patch stores.
///
/// # Errors
/// `update-not-implemented (diags)`: a warning the update cannot raise
/// again within O(change); `state-corrupt`.
///
/// Until U-9 lands: `fresh` and `odr.rows`.
pub fn diags(
    view: &StateView<'_>,
    changed: &ChangedFiles,
    resolved: &ResolveDelta,
    odr: &OdrUpdate,
    fresh: Vec<DiagRow>,
) -> Result<Vec<DiagRow>, IncrReason> {
    let _ = (view, changed, resolved);
    let mut out = fresh;
    out.extend(odr.rows.iter().cloned());
    Ok(out)
}
