//! WP-225 M2b-1: the U4 adapter (record §0.2, rulings M2a3-1): the
//! changed files' new rows in the UPDATE's id space ([`ChangedFiles`]),
//! from path (A)'s front through the capture's own per-file makers
//! (`capture::file_rows`, `file_rows_index_edges`), moved into the
//! update's ids by ONE function ([`translate`]).
//!
//! The two id spaces never meet by number (255 rulings D3-1): the link's
//! `SectionId`s and `SymbolId`s are the front's; a changed file's sections
//! take the ids past the state's `SECTIONS` count (in [`ChangedFiles::files`]
//! order), its strings the offsets past `STRINGS` (one buffer), its keys
//! are NAMES ([`KeyName`]), its mentions name keys by position.

use std::collections::HashMap;

use mortar_core::incr::{IncrReason, LiveEdge, LiveTarget, Unimplemented};
use mortar_core::inputs::Reader;
use mortar_core::{FileId, SectionId, SymbolId};
use mortar_incr::capture::{FileRows, Keys, Link, file_rows, file_rows_index_edges};
use mortar_incr::format::{NONE, Str, Strings, index_kind, kind, sym_kind};
use mortar_incr::update::rows::{CopyRow, EdgeRow, FileRow, MentionRow, edge_kind};
use mortar_incr::update::testing::MemoryState;
use mortar_incr::update::view::{
    Change, ChangedFile, ChangedFiles, KeyName, MentionValue, OdrRows, StateView,
};

use crate::front::Front;

fn resolve() -> IncrReason {
    IncrReason::UpdateNotImplemented(Some(Unimplemented::Resolve))
}

fn corrupt(what: impl std::fmt::Display) -> IncrReason {
    IncrReason::StateCorrupt(format!("the U4 adapter: {what}"))
}

/// U4's adapter (WP-225 M2b-1): every [`ChangedFile`] field of `files`
/// (each a file of the state that is also the front's file of that id:
/// `Changed` or `DrawnIn`) from path (A)'s front `f`, in the update's id
/// space over `view`. `capture` is the full link's capture of `f` (the
/// producer of each file's `FILES` row: hash, flags, `stack`,
/// `properties`, eh columns); `positional` is U3's set.
///
/// Per field: `sections`, `first_global`, `mentions`, `values`,
/// `versioned`, `odr.copies` from `capture::file_rows`; `index` and
/// `edges` from `capture::file_rows_index_edges` (`edges` only under
/// `--gc-sections`, from the front's `mortar_live::stored_edges` rows);
/// `row` the capture's (its stored `rank` and `round` kept, seam 6);
/// `meta_units` the file's `.mason.meta` sections in the front's
/// resolution (a count of SECTIONS: only `> 0` is read, `odr`'s
/// `keep_meta` refusal); `debug` a non-alloc `.debug_*` input section
/// (the join's [`super::object_has_debug`]). `keys` are the globals the
/// rows name (mentions, copies, `INDEX`/`EDGES` key targets); no local
/// key (none of these rows names one).
///
/// # Errors
/// `update-not-implemented (resolve)`: a file the front does not have at
/// that id (path or member differs), an appeared, gone, extracted or no
/// longer extracted file (M2b-1 makes `Changed` and `DrawnIn` only), a
/// file the front did not keep; `update-not-implemented (live)`: under
/// `--gc-sections` without the front's live rows, or an `EDGES` row whose
/// section target is another file's; `state-corrupt` for rows that are
/// not the file's own; `too-much-changed` past the `u32` id spaces.
pub fn changed_files(
    f: &Front<'_>,
    view: &StateView<'_>,
    files: &[(FileId, Change)],
    positional: &[FileId],
    capture: &MemoryState,
) -> Result<ChangedFiles, IncrReason> {
    use mortar_core::resolution::FileLive;
    let mut files = files.to_vec();
    files.sort_by_key(|(id, _)| *id);
    let link = Link {
        inputs: &f.inputs,
        resolution: &f.resolution,
        assignment: &f.assignment,
        split: &f.merge.split,
    };
    // The front's file of that id is the state's (identity by path and
    // member, never by number alone), kept by the front.
    for &(id, change) in &files {
        if !matches!(change, Change::Changed | Change::DrawnIn) {
            return Err(resolve());
        }
        let stored = view.file(id)?;
        let x = f.inputs.files.get(id).ok_or_else(resolve)?;
        let (name, ordinal) = x.member.as_ref().map_or((&b""[..], 0), |m| (m.name, m.ordinal));
        if x.path != view.string(stored.path)?.as_slice()
            || (stored.member != 0) != x.member.is_some()
            || (stored.member != 0
                && (name != view.string(stored.member_name)?.as_slice()
                    || ordinal != stored.member_ordinal))
            || f.resolution.file_live.get(id).is_none_or(|l| *l == FileLive::Dead)
        {
            return Err(resolve());
        }
    }
    let ids: Vec<FileId> = files.iter().map(|(id, _)| *id).collect();
    let keys = Keys::partial(&link, &|x| ids.binary_search(&x).is_ok()).map_err(corrupt)?;
    let copies = f.plan.diagnostics.odr.mode != mortar_core::plan::OdrMode::Off;
    let mut strings = Strings::default();
    let mut rows = file_rows(&link, &f.liveness, &keys, &f.layout, &mut strings, &ids, copies)
        .map_err(corrupt)?;
    let edges: Option<&[LiveEdge]> = if f.plan.shaping.gc_sections {
        match f.incr5.as_ref().and_then(|i| i.live_rows.as_ref()) {
            Some(Ok((e, _))) => Some(e),
            _ => return Err(IncrReason::UpdateNotImplemented(Some(Unimplemented::Live))),
        }
    } else {
        None
    };
    file_rows_index_edges(&link, f.arch.get(), edges, &mut rows).map_err(corrupt)?;
    // The `FILES` rows: the capture's, their strings into the same buffer.
    let mut file_row = Vec::with_capacity(ids.len());
    for &id in &ids {
        let mut r = capture.files.get(id.0 as usize).cloned().ok_or_else(|| corrupt("FILES"))?;
        let bytes = |s: Str| -> Result<&[u8], IncrReason> {
            let (a, b) = (s.off as usize, s.off as usize + s.len as usize);
            capture.strings.get(a..b).ok_or_else(|| corrupt("a capture string"))
        };
        r.path = strings.add(bytes(r.path)?);
        r.member_name = strings.add(bytes(r.member_name)?);
        let stored = view.file(id)?;
        (r.rank, r.round) = (stored.rank, stored.round);
        file_row.push(r);
    }
    let strings = strings.into_bytes();
    let base = u32::try_from(view.count(kind::STRINGS))
        .ok()
        .filter(|b| u32::try_from(strings.len()).is_ok_and(|n| b.checked_add(n).is_some()))
        .ok_or_else(|| IncrReason::TooMuchChanged("the update's strings past u32".into()))?;
    let mut next = u32::try_from(view.count(kind::SECTIONS))
        .map_err(|_| IncrReason::TooMuchChanged("SECTIONS past u32".into()))?;
    let mut out = ChangedFiles {
        files: Vec::with_capacity(ids.len()),
        appeared: Vec::new(),
        positional: positional.to_vec(),
        strings,
    };
    for ((fr, (_, change)), row) in rows.iter().zip(&files).zip(file_row) {
        let mut cf = translate(f, fr, *change, row, SectionId(next), base)?;
        cf.debug = match f.inputs.files.get(fr.file).map(|x| &x.reader) {
            Some(Reader::Object(o)) => super::object_has_debug(o),
            _ => false,
        };
        cf.meta_units = u32::try_from(f.resolution.meta.iter().filter(|m| m.0 == fr.file).count())
            .unwrap_or(u32::MAX);
        next = u32::try_from(cf.sections.len())
            .ok()
            .and_then(|n| next.checked_add(n))
            .ok_or_else(|| IncrReason::TooMuchChanged("SECTIONS past u32".into()))?;
        out.files.push(cf);
    }
    Ok(out)
}

/// THE translation (the one function from the link's ids to the update's):
/// `fr`'s rows with its sections at `first ..`, its `Str`s (offsets into
/// the update's buffer) past `base`, its keys by name (position in
/// `keys`, in the order the rows first name them).
fn translate(
    f: &Front<'_>,
    fr: &FileRows,
    change: Change,
    mut row: FileRow,
    first: SectionId,
    base: u32,
) -> Result<ChangedFile, IncrReason> {
    let rebase =
        |s: Str| if s.len == 0 { Str::default() } else { Str { off: base + s.off, len: s.len } };
    row.path = rebase(row.path);
    row.member_name = rebase(row.member_name);
    let count = u32::try_from(fr.sections.len()).map_err(|_| corrupt("sections"))?;
    // A section of the link's ids, the file's own: its update id.
    let section = |s: u32| -> Result<u32, IncrReason> {
        if s == NONE {
            return Ok(NONE);
        }
        match s.checked_sub(fr.first_section) {
            Some(k) if k < count => Ok(first.0 + k),
            _ => Err(corrupt(format!("file {}: section {s} is not its own", fr.file.0))),
        }
    };
    let mut keys: Vec<KeyName> = Vec::new();
    let mut at: HashMap<u32, u32> = HashMap::new();
    let mut key = |id: u32| -> Result<u32, IncrReason> {
        if let Some(&k) = at.get(&id) {
            return Ok(k);
        }
        let k = f.inputs.symbols.get(SymbolId(id)).ok_or_else(|| corrupt("a key out of range"))?;
        keys.push(KeyName {
            name: k.name.to_vec(),
            version: k.version.map(<[u8]>::to_vec).unwrap_or_default(),
            kind: if k.version.is_some() { sym_kind::VERSIONED } else { sym_kind::GLOBAL },
            index: 0,
        });
        let p = u32::try_from(keys.len() - 1).map_err(|_| corrupt("keys"))?;
        at.insert(id, p);
        Ok(p)
    };
    let mut sections = fr.sections.clone();
    for s in &mut sections {
        s.anchor = rebase(s.anchor);
        s.name = rebase(s.name);
        // U-4 places them.
        (s.place_chunk, s.place_offset) = (NONE, 0);
    }
    let mut mentions = Vec::with_capacity(fr.mentions.len());
    for m in &fr.mentions {
        mentions.push(MentionRow {
            symbol: key(m.symbol)?,
            kind: m.kind,
            bind: m.bind,
            typ: m.typ,
            other: m.other,
            section: section(m.section)?,
        });
    }
    let values = fr.values.iter().map(|&(value, size)| MentionValue { value, size }).collect();
    let mut copies = Vec::with_capacity(fr.copies.len());
    for &(k, index, size, typ) in &fr.copies {
        copies.push(CopyRow { key: key(k)?, file: fr.file.0, index, size, typ, tomb: false });
    }
    let mut index = fr.index.clone();
    for r in &mut index {
        r.unit = section(r.unit)?;
        r.target = match r.kind {
            index_kind::GLOBAL | index_kind::SLOT => key(r.target)?,
            _ => section(r.target)?,
        };
    }
    let mut edges = Vec::with_capacity(fr.edges.len());
    for e in &fr.edges {
        use edge_kind::Target;
        let live = || IncrReason::UpdateNotImplemented(Some(Unimplemented::Live));
        let target = match (edge_kind::target(e.kind), e.target) {
            (Some(Target::Key), LiveTarget::Key(k)) => key(k.0)?,
            (Some(Target::Section), LiveTarget::Section(t)) => section(t.0).map_err(|_| live())?,
            (Some(Target::Zero), LiveTarget::None) => 0,
            (Some(Target::Code), LiveTarget::Code(c)) => c,
            _ => {
                return Err(corrupt(format!(
                    "EDGES: a row of kind {} with the wrong target",
                    e.kind
                )));
            }
        };
        edges.push(EdgeRow {
            kind: e.kind,
            source: section(e.source.0)?,
            target,
            offset: e.offset,
        });
    }
    Ok(ChangedFile {
        file: fr.file,
        change,
        row,
        first_section: first,
        sections,
        keys,
        first_global: fr.first_global,
        mentions,
        values,
        index,
        edges,
        meta_units: 0,
        debug: false,
        versioned: fr.versioned,
        odr: OdrRows { copies },
    })
}

/// A section's identity `(file, anchor kind, anchor, name, ordinal)`,
/// names by BYTES (the oracle's and the agreement check's key).
pub type SectionKey = (u32, u8, Vec<u8>, Vec<u8>, u32);

/// The identity of section `s` of the update's id space: a stored row
/// below the state's `SECTIONS` count, else a changed file's fresh row.
///
/// # Errors
/// `state-corrupt`: an id no row has.
pub fn update_section(
    view: &StateView<'_>,
    cf: &ChangedFiles,
    s: u32,
) -> Result<SectionKey, IncrReason> {
    let row = if u64::from(s) < view.count(kind::SECTIONS) {
        let r = view.section(SectionId(s))?;
        return Ok((
            r.file,
            r.anchor_kind,
            view.string(r.anchor)?,
            view.string(r.name)?,
            r.ordinal,
        ));
    } else {
        cf.files
            .iter()
            .find_map(|c| s.checked_sub(c.first_section.0).and_then(|k| c.sections.get(k as usize)))
            .ok_or_else(|| corrupt(format!("section {s} is no row of the update")))?
    };
    Ok((
        row.file,
        row.anchor_kind,
        cf.string(view, row.anchor)?,
        cf.string(view, row.name)?,
        row.ordinal,
    ))
}

/// A `Str` of a [`MemoryState`]'s own buffer.
fn mem_string(m: &MemoryState, s: Str) -> Result<Vec<u8>, IncrReason> {
    let (a, b) = (s.off as usize, s.off as usize + s.len as usize);
    m.strings.get(a..b).map(<[u8]>::to_vec).ok_or_else(|| corrupt("a capture string"))
}

/// A FINDING of M2b-1 (a package's answer that is not the front's): the
/// named fall-back (a full link follows, correct by construction), and
/// both answers appended to the file `MORTAR_UPDATE_FINDINGS` names when
/// it is set (the lane's and the tests' record; never the diagnostic
/// stream: a fall-back adds no line there).
fn finding(what: &str) -> IncrReason {
    record(&format!("M2b-1 finding: {what}"));
    resolve()
}

/// U4-U5 of an M2b-1 update (record §0.2): U4's member-level judgement
/// (a file of a changed path is changed when the capture's `FILES` hash
/// is not the stored one), [`changed_files`], `extract`, `resolve` (with
/// its draw-ins, at most `RESOLVE_CALLS`), and their answers judged
/// against path (A)'s front through its capture `capture`: no member
/// newly extracted, dropped or with another round (else
/// `update-not-implemented (resolve)`: M2b-1 makes no extraction), and
/// every affected key's `RESOLVED` row equal to the capture's (key by
/// name, section by identity; `facts` aside, the scan's). A disagreement
/// is a FINDING ([`finding`]) and falls back by name. Returns the
/// `ChangedFiles` and the delta (M2b's next steps' inputs; liveness'
/// pair tests).
///
/// # Errors
/// The packages' own reasons; `update-not-implemented (resolve)` as
/// above.
///
/// `mark` is called with `"u4"` after the adapter and `"u5"` after the
/// judgement (the report line's step times). A refusal is also appended to
/// the `MORTAR_UPDATE_FINDINGS` file when it is set (the lane's count of
/// the fall-backs by name), as `M2b-1 fallback: <reason>`.
pub fn u5(
    view: &StateView<'_>,
    changed: &[FileId],
    positional: &[FileId],
    f: &Front<'_>,
    capture: &MemoryState,
    mark: &mut dyn FnMut(&'static str),
) -> Result<Option<(ChangedFiles, mortar_incr::update::resolve::ResolveDelta)>, IncrReason> {
    let r = u5_judged(view, changed, positional, f, capture, mark);
    if let Err(e) = &r {
        record(&format!("M2b-1 fallback: {}", e.report()));
    }
    r
}

/// Appends `line` to the `MORTAR_UPDATE_FINDINGS` file when it is set.
fn record(line: &str) {
    if let Some(path) = std::env::var_os("MORTAR_UPDATE_FINDINGS") {
        use std::io::Write as _;
        if let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true).open(path) {
            let _ = writeln!(f, "{line}");
        }
    }
}

fn u5_judged(
    view: &StateView<'_>,
    changed: &[FileId],
    positional: &[FileId],
    f: &Front<'_>,
    capture: &MemoryState,
    mark: &mut dyn FnMut(&'static str),
) -> Result<Option<(ChangedFiles, mortar_incr::update::resolve::ResolveDelta)>, IncrReason> {
    use mortar_incr::update::rows::file_flags::LAZY;
    use mortar_incr::update::{extract, resolve as res};
    let mut files = Vec::new();
    for &id in changed {
        let now = capture.files.get(id.0 as usize).ok_or_else(|| corrupt("FILES"))?;
        if now.hash != view.file(id)?.hash {
            files.push((id, Change::Changed));
        }
    }
    if files.is_empty() {
        return Ok(None);
    }
    let options = mortar_resolve::tables::option_keys(&f.plan, &f.inputs.link.directives);
    let mut cf = changed_files(f, view, &files, positional, capture)?;
    mark("u4");
    let ex = extract::extract(view, &cf, &options)?;
    if !(ex.extracted.is_empty() && ex.unextracted.is_empty() && ex.rounds.is_empty()) {
        // The extraction by the `LAZY` flag; a round is "any supporting
        // value" (IN-71b) and the capture writes none (0, which the
        // canonical form ignores: WP-214's finding), so it is not judged.
        let row = |m: FileId| capture.files.get(m.0 as usize);
        let front = ex.extracted.iter().all(|&(m, _)| row(m).is_some_and(|x| x.flags & LAZY == 0))
            && ex.unextracted.iter().all(|&m| row(m).is_some_and(|x| x.flags & LAZY != 0));
        if !front {
            let mut rows = String::new();
            for m in ex
                .extracted
                .iter()
                .map(|x| x.0)
                .chain(ex.unextracted.iter().copied())
                .chain(ex.rounds.iter().map(|x| x.0))
            {
                let old = view.file(m)?;
                let now = row(m).ok_or_else(|| corrupt("FILES"))?;
                let name = String::from_utf8_lossy(&view.string(old.member_name)?).into_owned();
                rows.push_str(&format!(
                    " [file {} {name}: stored flags {:#x} round {}, the front's flags {:#x} round {}]",
                    m.0, old.flags, old.round, now.flags, now.round
                ));
            }
            return Err(finding(&format!("extract gives {ex:?}; the front's FILES rows:{rows}")));
        }
        return Err(resolve());
    }
    let mut calls = 0;
    let delta = loop {
        calls += 1;
        match res::resolve(view, &cf, &ex, &options)? {
            res::Resolution::Delta(d) => break d,
            res::Resolution::DrawIn(more) if calls < res::RESOLVE_CALLS => {
                files.extend(more.iter().map(|&m| (m, Change::DrawnIn)));
                cf = changed_files(f, view, &files, positional, capture)?;
            }
            res::Resolution::DrawIn(more) => {
                return Err(IncrReason::InputChanged(format!("a second draw-in: {more:?}")));
            }
        }
    };
    // The front's answer per affected key: the capture's `RESOLVED` row
    // of the global of that name.
    let mut by_name: HashMap<(Vec<u8>, Vec<u8>), usize> = HashMap::new();
    for (k, s) in capture.symbols.iter().enumerate() {
        if s.kind != sym_kind::LOCAL {
            by_name
                .entry((mem_string(capture, s.name)?, mem_string(capture, s.version)?))
                .or_insert(k);
        }
    }
    let symbols = view.count(kind::SYMBOLS);
    for (id, r) in &delta.resolved {
        let name = if u64::from(id.0) < symbols {
            let s = view.symbol(*id)?;
            (view.string(s.name)?, view.string(s.version)?)
        } else {
            let n = delta
                .new_keys
                .get((u64::from(id.0) - symbols) as usize)
                .ok_or_else(|| corrupt(format!("key {} is no row of the update", id.0)))?;
            (n.name.clone(), n.version.clone())
        };
        let show = String::from_utf8_lossy(&name.0).into_owned();
        let Some(c) = by_name.get(&name).and_then(|&k| capture.resolved.get(k)) else {
            return Err(finding(&format!(
                "key {show}: resolve gives {r:?}; the front has no such key"
            )));
        };
        let at = |s: u32| -> Result<Option<SectionKey>, IncrReason> {
            Ok(if s == NONE { None } else { Some(update_section(view, &cf, s)?) })
        };
        let front_at = match capture.sections.get(c.section as usize) {
            Some(x) if c.section != NONE => Some((
                x.file,
                x.anchor_kind,
                mem_string(capture, x.anchor)?,
                mem_string(capture, x.name)?,
                x.ordinal,
            )),
            _ => None,
        };
        let mine = at(r.section)?;
        let same = (r.kind, r.typ, r.bind, r.other, r.value, r.size)
            == (c.kind, c.typ, c.bind, c.other, c.value, c.size)
            && mine == front_at;
        if !same {
            let mut files = String::new();
            for g in
                [mine.as_ref().map(|x| x.0), front_at.as_ref().map(|x| x.0)].into_iter().flatten()
            {
                let old = view.file(FileId(g))?;
                let now = capture.files.get(g as usize).ok_or_else(|| corrupt("FILES"))?;
                files.push_str(&format!(
                    " [file {g}: stored flags {:#x}, the front's flags {:#x}]",
                    old.flags, now.flags
                ));
            }
            return Err(finding(&format!(
                "key {show}: resolve gives {r:?} at {mine:?}; the front's capture {c:?} at {front_at:?};{files} extraction {ex:?}"
            )));
        }
    }
    mark("u5");
    Ok(Some((cf, *delta)))
}
