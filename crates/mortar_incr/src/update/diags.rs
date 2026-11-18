//! Diagnostics equal to a full link's (`spec/incr.md` IN-75a): the stored
//! warnings (`DIAGS`, kind 49) dropped by subject, recomputed, and emitted
//! in `spec/diag.md` §6.2's report order. WP-213 (U-1): the types
//! [`DiagRow`] and [`Subject`] and the entry point [`diags`]; **filled by
//! U-9** (the function only; the types are U-1's).

use std::collections::{BTreeMap, BTreeSet};

use mortar_core::incr::{IncrReason, Unimplemented};
use mortar_core::{FileId, OutSecId, SectionId, SymbolId};
use mortar_diag::subject::{from_stored, stored};
use mortar_diag::{Collector, Context, Diagnostic, Place, Relation, Style};

use crate::format::Str;

use super::odr::{Now, OdrUpdate};
use super::resolve::ResolveDelta;
use super::rows::{FileTable, file_flags};
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
/// order (units as `FILES` rows; [`emit`] maps them to the full link's).
/// Called by the join (U-10) after U7; its result is what the update
/// prints (through [`emit`]) and what the patch stores.
///
/// `fresh` (WP-221 §2.3, the join's table): every warning the update's
/// steps raised over what they read, as [`row`] makes it, in the update's
/// id space, with these conventions:
/// - a reference-site warning (`gnu-warning` of a key, `undefined-symbol`)
///   holds only the references in the files the update read (changed,
///   appeared, drawn-in, newly extracted), its listed places theirs; for
///   `undefined-symbol` the step gives one for every affected key that is
///   warned after the update, with no reference if no file it read has
///   one; for `gnu-warning` a `File` subject is a holder of
///   `.gnu.warning.K` the step read (its message is the warning's);
/// - `executable-stack`: one per read file whose new note asks for it.
///
/// # Errors
/// `update-not-implemented (diags)`: a warning the update cannot raise
/// again within O(change) (§2.2's table); `state-corrupt`: a row that does
/// not read back.
pub fn diags(
    view: &StateView<'_>,
    changed: &ChangedFiles,
    resolved: &ResolveDelta,
    odr: &OdrUpdate,
    fresh: Vec<DiagRow>,
) -> Result<Vec<DiagRow>, IncrReason> {
    let now = Now::new(view, changed, resolved);
    let touched: BTreeSet<FileId> = changed.files.iter().map(|f| f.file).collect();
    let affected = |k: SymbolId| resolved.affected.binary_search(&k).is_ok();
    let odr_key = |k: SymbolId| odr.keys.binary_search(&k).is_ok();
    // A key-scoped reference-site warning: stored (≤ 1) and fresh (≤ 1).
    let mut sites: BTreeMap<(String, SymbolId), (Option<DiagRow>, Option<DiagRow>)> =
        BTreeMap::new();
    let mut stack: (Option<DiagRow>, Vec<DiagRow>) = (None, Vec::new());
    let mut probes: Vec<DiagRow> = Vec::new();
    let mut out: Vec<DiagRow> = Vec::new();
    // IN-75a: `DIAGS` is read whole (a scan by rule, WP-213 F-3).
    for r in view.diags()? {
        match policy(&r) {
            Policy::FallBack => return Err(fall_back()),
            Policy::Dropped => {}
            Policy::Probe => probes.push(r),
            Policy::Site(k) => {
                let e = sites.entry((r.code.clone(), k)).or_default();
                if e.0.replace(r).is_some() {
                    return Err(corrupt("DIAGS: two rows of one reference-site warning"));
                }
            }
            Policy::Stack => {
                if stack.0.replace(r).is_some() {
                    return Err(corrupt("DIAGS: two executable-stack rows"));
                }
            }
            Policy::Odr(k) => {
                if !odr_key(k) {
                    out.push(r);
                }
            }
            Policy::Subjects => {
                let hit = r.subjects.iter().any(|s| match s {
                    Subject::File(f) => touched.contains(f),
                    Subject::Key(k) => affected(*k),
                    Subject::OdrKey { .. } | Subject::OutputSection(_) | Subject::Link => true,
                });
                if !hit {
                    out.push(r);
                }
            }
        }
    }
    for r in fresh {
        match policy(&r) {
            Policy::FallBack => return Err(fall_back()),
            Policy::Site(k) => {
                let e = sites.entry((r.code.clone(), k)).or_default();
                if e.1.replace(r).is_some() {
                    return Err(corrupt("two fresh rows of one reference-site warning"));
                }
            }
            Policy::Stack => stack.1.push(r),
            Policy::Probe => {
                probes.retain(|p| p.key.unit != r.key.unit);
                out.push(r);
            }
            Policy::Dropped | Policy::Odr(_) | Policy::Subjects => out.push(r),
        }
    }
    out.append(&mut probes);
    out.extend(odr.rows.iter().cloned());
    let rank = |f: FileId| now.rank(f.0);
    for ((code, key), (stored, fresh)) in sites {
        let still = stored.as_ref().is_some_and(|s| {
            !affected(key)
                && !s.subjects.iter().any(|x| matches!(x, Subject::File(f) if touched.contains(f)))
        });
        match (stored, fresh) {
            (Some(s), None) if still => out.push(s),
            (stored, fresh) => {
                let site = Site { view, now: &now, touched: &touched, key, code: &code };
                out.extend(site.recompute(stored, fresh, &rank)?);
            }
        }
    }
    out.extend(executable_stack(stack, &touched, &rank)?);
    sort(&mut out, &rank)?;
    Ok(out)
}

/// What [`diags`] does with a row, by its code (WP-221 §2.2).
enum Policy {
    /// `update-not-implemented (diags)`: a `link`-subject warning the
    /// update cannot raise again (`--warn-backrefs`' and the version
    /// script's, decided from the plan at U1 as well; codes that cannot
    /// occur in an incremental link).
    FallBack,
    /// Recomputed every update by the step that raises it (the front's
    /// `start`): a stored row is dropped.
    Dropped,
    /// A reference-site warning of this key.
    Site(SymbolId),
    /// `executable-stack` (F-1).
    Stack,
    /// A W1/W2 finding of this key (U-8's contract: replaced iff the key is
    /// in `OdrUpdate::keys`).
    Odr(SymbolId),
    /// Dropped iff a subject is touched (IN-75a).
    Subjects,
    /// `skipped-incompatible` (subject `link`): U2 re-stats every probe
    /// (input.md §4.6); a search whose outcome differs is run again and
    /// its warnings are fresh (they replace the stored ones of that
    /// argument); every other search skips what it skipped.
    Probe,
}

fn policy(r: &DiagRow) -> Policy {
    let key = r.subjects.iter().find_map(|s| if let Subject::Key(k) = s { Some(*k) } else { None });
    match r.code.as_str() {
        "archive-backref"
        | "archive-choice"
        | "version-reassigned"
        | "version-script-undefined-name"
        | "versioned-definition-hidden"
        | "duplicate-soname"
        | "needed-not-found"
        | "incremental-unsupported"
        | "copy-of-locally-bound-definition" => Policy::FallBack,
        "incremental-not-reproducible" => Policy::Dropped,
        // skipped-incompatible: the probes re-stat'ed by U2 (input.md
        // §4.6): a different outcome is a re-run whose warnings are fresh;
        // the same outcome, the same warning (§2.2).
        "skipped-incompatible" => Policy::Probe,
        "executable-stack" => Policy::Stack,
        // A key's reference-site warning, a dormant row included (§2.4);
        // `gnu-warning` at another phase is the file-level `.gnu.warning`
        // (resolve's, phase 30): its subjects decide.
        code if reference_site(code) && (code == "undefined-symbol" || r.key.phase == SCAN) => {
            key.map_or(Policy::Subjects, Policy::Site)
        }
        code if code.starts_with("odr-") && code != "odr-abi-mismatch" => {
            match (key, r.subjects.iter().any(|s| matches!(s, Subject::OdrKey { .. }))) {
                (Some(k), false) => Policy::Odr(k),
                _ => Policy::Subjects,
            }
        }
        _ => Policy::Subjects,
    }
}

/// The scan's phase (`spec/model.md` §8): a reference-site warning's key.
const SCAN: u16 = 70;

fn fall_back() -> IncrReason {
    IncrReason::UpdateNotImplemented(Some(Unimplemented::Diags))
}

fn corrupt(what: &str) -> IncrReason {
    IncrReason::StateCorrupt(format!("DIAGS: {what}"))
}

/// One key's reference-site warning, recomputed (WP-221 §1.5, ruling 3
/// A+, ruling 5).
struct Site<'a, 'v> {
    view: &'a StateView<'v>,
    now: &'a Now<'a, 'v>,
    touched: &'a BTreeSet<FileId>,
    key: SymbolId,
    code: &'a str,
}

impl Site<'_, '_> {
    fn recompute(
        &self,
        stored: Option<DiagRow>,
        fresh: Option<DiagRow>,
        rank: &dyn Fn(FileId) -> Result<u64, IncrReason>,
    ) -> Result<Option<DiagRow>, IncrReason> {
        let read =
            |r: &DiagRow| diagnostic(r).ok_or_else(|| corrupt("a row that does not read back"));
        let stored = stored.map(|r| read(&r).map(|d| (r, d))).transpose()?;
        let fresh = fresh.map(|r| read(&r).map(|d| (r, d))).transpose()?;
        let holder_of = |r: &DiagRow| {
            r.subjects.iter().find_map(|s| if let Subject::File(f) = s { Some(*f) } else { None })
        };
        // The head: the fresh row's for `undefined-symbol` (it says whether
        // the key is still warned); for `gnu-warning` the holder's message,
        // from the fresh row when the step read a holder, else from the
        // stored row when its holder is untouched (ruling 5).
        let head: (Diagnostic, Option<FileId>) = if self.code == "gnu-warning" {
            match (&fresh, &stored) {
                (Some((r, d)), _) if holder_of(r).is_some() => (d.clone(), holder_of(r)),
                (_, Some((r, d))) if holder_of(r).is_some_and(|h| !self.touched.contains(&h)) => {
                    (d.clone(), holder_of(r))
                }
                // No holder left (gone, or changed without the section):
                // no warning; a fresh row without a holder is the step's
                // references to a key it does not know to be warned.
                _ => return Ok(None),
            }
        } else {
            match &fresh {
                Some((_, d)) => (d.clone(), None),
                None => return Ok(None),
            }
        };
        let unchanged = self.unchanged(stored.as_ref().map(|s| &s.0))?;
        // The fresh references, with the places the fresh row lists.
        let (fresh_refs, fresh_count, fresh_places): (Vec<DiagRef>, u64, Vec<Place>) = match &fresh
        {
            Some((r, d)) => {
                let mut by_rank = r.refs.clone();
                sort_refs(&mut by_rank, rank)?;
                let places = listed(d);
                (by_rank, r.ref_count, places)
            }
            None => (Vec::new(), 0, Vec::new()),
        };
        if fresh_refs.iter().any(|x| !self.touched.contains(&x.file))
            || (fresh_refs.len() as u64) > fresh_count
        {
            return Err(corrupt("a fresh reference in a file the update did not read"));
        }
        let fresh_capped = (fresh_refs.len() as u64) < fresh_count;
        let total = unchanged.len() as u64 + fresh_count;
        if total == 0 {
            // A warned key no reference reaches: its dormant row (§2.4),
            // so that a later reference in a file an update reads has its
            // message while the holder is not read.
            return Ok(head.1.and_then(|h| dormant(&head.0, self.key, h)));
        }
        let mut unchanged = unchanged;
        sort_refs(&mut unchanged, rank)?;
        // Merged in (rank, seq) order: the first three are among the first
        // three of each side, so the fresh row's listed places suffice.
        let mut merged: Vec<(DiagRef, Option<Place>)> = Vec::new();
        let (mut u, mut f) =
            (unchanged.iter().peekable(), fresh_refs.iter().enumerate().peekable());
        loop {
            let take_u = match (u.peek(), f.peek()) {
                (None, None) => break,
                (Some(_), None) => true,
                (None, Some(_)) => false,
                (Some(a), Some((_, b))) => (rank(a.file)?, a.seq) < (rank(b.file)?, b.seq),
            };
            if take_u {
                let a = *u.next().ok_or_else(|| corrupt("merge"))?;
                merged.push((a, None));
            } else {
                let (i, b) = f.next().ok_or_else(|| corrupt("merge"))?;
                merged.push((*b, fresh_places.get(i).cloned()));
            }
        }
        // A capped fresh side: the references after its last stored one
        // are unknown; the row keeps the known prefix (capped).
        if fresh_capped && let Some(last) = fresh_refs.last() {
            let end = merged.iter().position(|(r, _)| r == last).map_or(merged.len(), |p| p + 1);
            merged.truncate(end);
        }
        let Some(first) = merged.first().map(|m| m.0) else {
            return Err(fall_back());
        };
        let (mut d, holder) = (strip(&head.0), head.1);
        for (r, place) in merged.iter().take(3) {
            let place = match place {
                Some(p) => p.clone(),
                None => self.place(*r)?,
            };
            d = d.with(Relation::ReferencedBy, place);
        }
        if total > 3 {
            d = d.note(format!("{}{}{}", MORE.0, total - 3, MORE.1));
        }
        let mut subjects = vec![Subject::Key(self.key)];
        subjects.extend(holder.map(Subject::File));
        let refs: Vec<DiagRef> = merged.iter().map(|m| m.0).take(REFS_CAP).collect();
        let key = mortar_diag::Key { phase: SCAN, unit: first.file.0, seq: first.seq };
        let mut r =
            row(key, &d, subjects, refs).ok_or_else(|| corrupt("a warning without its row"))?;
        if r.ref_count != total {
            return Err(corrupt("a reference count"));
        }
        // `row` keeps the first REFS_CAP by `DiagRef` order; keep them by
        // report order instead (the capped prefix, ruling 3).
        r.refs = merged.iter().map(|m| m.0).take(REFS_CAP).collect();
        r.refs.sort_unstable();
        Ok(Some(r))
    }

    /// The references of the files the update did not read: the stored
    /// row's, when it holds every one (A+; a capped row falls back), even
    /// when its holder changed (ruling 5: the message from the new
    /// section, the references from the row); with no stored row, the
    /// key's `KEY_REFS` must name no such file (else their places are
    /// unknown: a NEW holder, ruling 5; a key newly undefined).
    fn unchanged(&self, stored: Option<&DiagRow>) -> Result<Vec<DiagRef>, IncrReason> {
        if let Some(r) = stored {
            if (r.refs.len() as u64) < r.ref_count {
                return Err(fall_back());
            }
            return Ok(r
                .refs
                .iter()
                .filter(|x| !self.touched.contains(&x.file))
                .copied()
                .collect());
        }
        for k in self.view.key_refs(self.key)? {
            let f = FileId(k.file);
            if k.file == 0 || self.touched.contains(&f) {
                continue;
            }
            if self.view.file(f)?.flags & (file_flags::LAZY | file_flags::GONE) == 0 {
                return Err(fall_back());
            }
        }
        Ok(Vec::new())
    }

    /// The place of a reference in a file the update did not read: its
    /// display from `FILES`, its section's name from `SECTIONS`.
    fn place(&self, r: DiagRef) -> Result<Place, IncrReason> {
        let file = self.now.display(r.file)?;
        let first = self.view.file_index(r.file)?.ranges[FileTable::Sections as usize];
        if r.section() >= first.count {
            return Err(corrupt("a reference's section"));
        }
        let section = self.view.section(SectionId(first.first + r.section()))?;
        let name = String::from_utf8_lossy(&self.view.string(section.name)?).into_owned();
        Ok(Place::section(file, name, r.offset))
    }
}

/// The dormant row of a `.gnu.warning.K` holder whose key no reference
/// reaches (WP-221 §2.4): the message and the holder, no reference; key
/// `(70, holder, u64::MAX)`; stored, never printed ([`emit`] skips it).
#[must_use]
pub fn dormant(d: &Diagnostic, key: SymbolId, holder: FileId) -> Option<DiagRow> {
    let k = mortar_diag::Key { phase: SCAN, unit: holder.0, seq: u64::MAX };
    row(k, &strip(d), vec![Subject::Key(key), Subject::File(holder)], Vec::new())
}

/// Whether a row is a dormant one ([`dormant`]).
fn is_dormant(r: &DiagRow) -> bool {
    r.code == "gnu-warning" && r.key.phase == SCAN && r.ref_count == 0
}

/// The places a reference-site diagnostic lists, in order.
fn listed(d: &Diagnostic) -> Vec<Place> {
    d.context()
        .iter()
        .filter_map(|c| match c {
            Context::At(Relation::ReferencedBy, p) => Some(p.clone()),
            _ => None,
        })
        .collect()
}

/// A reference-site diagnostic without its references (its head: severity,
/// kind and place).
fn strip(d: &Diagnostic) -> Diagnostic {
    let head = Diagnostic::new(d.severity(), d.kind().clone());
    match d.place() {
        Some(p) => head.at(p.clone()),
        None => head,
    }
}

fn sort_refs(
    refs: &mut [DiagRef],
    rank: &dyn Fn(FileId) -> Result<u64, IncrReason>,
) -> Result<(), IncrReason> {
    let mut keyed = refs
        .iter()
        .map(|r| Ok(((rank(r.file)?, r.seq), *r)))
        .collect::<Result<Vec<_>, IncrReason>>()?;
    keyed.sort_unstable();
    for (slot, (_, r)) in refs.iter_mut().zip(keyed) {
        *slot = r;
    }
    Ok(())
}

/// `executable-stack` (L-84 names the first file in load order whose note
/// asks for an executable stack; WP-221 F-1, ruling 7): the least-ranked
/// of the fresh rows and the stored row of an untouched file. The stored
/// row's file touched and no fresh row at or before it: the next such
/// file is in a file the update did not read: fall back.
fn executable_stack(
    (stored, fresh): (Option<DiagRow>, Vec<DiagRow>),
    touched: &BTreeSet<FileId>,
    rank: &dyn Fn(FileId) -> Result<u64, IncrReason>,
) -> Result<Option<DiagRow>, IncrReason> {
    let file_of = |r: &DiagRow| {
        r.subjects
            .iter()
            .find_map(|s| if let Subject::File(f) = s { Some(*f) } else { None })
            .ok_or_else(|| corrupt("an executable-stack row without its file"))
    };
    let mut best: Option<(u64, DiagRow)> = None;
    for r in fresh {
        let at = rank(file_of(&r)?)?;
        if best.as_ref().is_none_or(|b| at < b.0) {
            best = Some((at, r));
        }
    }
    let Some(s) = stored else { return Ok(best.map(|b| b.1)) };
    let f = file_of(&s)?;
    let at = rank(f)?;
    if !touched.contains(&f) {
        return Ok(Some(match best {
            Some(b) if b.0 < at => b.1,
            _ => s,
        }));
    }
    match best {
        Some(b) if b.0 <= at => Ok(Some(b.1)),
        _ => Err(fall_back()),
    }
}

/// Whether a phase's `unit` is a `FileId` (`spec/model.md` §8).
pub(crate) const fn file_unit(phase: u16) -> bool {
    matches!(phase, 30 | 40 | 45 | 50 | 60 | 70 | 100)
}

/// A row's order key (`spec/diag.md` §6.2 with IN-75a's mapping): phase,
/// the unit's current rank (a `FileId` phase) or the unit, the seq (phase
/// 80's per-object finding: its file's rank, F-2).
fn order_key(
    r: &DiagRow,
    rank: &dyn Fn(FileId) -> Result<u64, IncrReason>,
) -> Result<(u16, u64, u64), IncrReason> {
    let k = r.key;
    let unit = if file_unit(k.phase) { rank(FileId(k.unit))? } else { u64::from(k.unit) };
    let seq = if k.phase == 80 && r.code == "unknown-property" {
        let f =
            r.subjects.iter().find_map(|s| if let Subject::File(f) = s { Some(*f) } else { None });
        f.map_or(Ok(k.seq), rank)?
    } else {
        k.seq
    };
    Ok((k.phase, unit, seq))
}

/// `rows` in report order: [`order_key`], then the plain text, then the
/// model (the collector's own tie-breaks, diag.md §6.2).
fn sort(
    rows: &mut Vec<DiagRow>,
    rank: &dyn Fn(FileId) -> Result<u64, IncrReason>,
) -> Result<(), IncrReason> {
    let plain = Style::plain("");
    let mut keyed = std::mem::take(rows)
        .into_iter()
        .map(|r| {
            let d = diagnostic(&r).ok_or_else(|| corrupt("a row that does not read back"))?;
            Ok((order_key(&r, rank)?, d.render(&plain), d, r))
        })
        .collect::<Result<Vec<_>, IncrReason>>()?;
    keyed.sort_by(|a, b| (a.0, &a.1).cmp(&(b.0, &b.1)).then_with(|| a.2.cmp(&b.2)));
    keyed.dedup_by(|a, b| a.0 == b.0 && a.2 == b.2);
    rows.extend(keyed.into_iter().map(|k| k.3));
    Ok(())
}

/// The update's warnings into its report (the join's call, after
/// [`diags`]): each row's diagnostic emitted with its key, the unit a
/// position in the current rank order (the full link's unit is the file's
/// position in load order: the order is the same), so `collector` sorts
/// them as the full link's report. `--fatal-warnings` and `-w` are the
/// collector's (diag.md §6.3): an update under `--fatal-warnings` that
/// keeps or raises a warning fails like the full link.
///
/// # Errors
/// `state-corrupt`: a row that does not read back, a file without its row.
pub fn emit(
    view: &StateView<'_>,
    changed: &ChangedFiles,
    resolved: &ResolveDelta,
    rows: &[DiagRow],
    collector: &Collector,
) -> Result<(), IncrReason> {
    let now = Now::new(view, changed, resolved);
    let rank = |f: FileId| now.rank(f.0);
    let mut keys = Vec::with_capacity(rows.len());
    let mut ranks: Vec<u64> = Vec::new();
    for r in rows {
        let k = order_key(r, &rank)?;
        if file_unit(k.0) || (k.0 == 80 && r.code == "unknown-property") {
            ranks.push(if file_unit(k.0) { k.1 } else { k.2 });
        }
        keys.push(k);
    }
    ranks.sort_unstable();
    ranks.dedup();
    let dense = |x: u64| u32::try_from(ranks.partition_point(|&r| r < x)).unwrap_or(u32::MAX);
    for (r, (phase, unit, seq)) in rows.iter().zip(keys) {
        if is_dormant(r) {
            continue;
        }
        let d = diagnostic(r).ok_or_else(|| corrupt("a row that does not read back"))?;
        let key = if file_unit(phase) {
            mortar_diag::Key { phase, unit: dense(unit), seq }
        } else if phase == 80 && r.code == "unknown-property" {
            mortar_diag::Key { phase, unit: r.key.unit, seq: u64::from(dense(seq)) }
        } else {
            r.key
        };
        collector.emit(key, d);
    }
    Ok(())
}
