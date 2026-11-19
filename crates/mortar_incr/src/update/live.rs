//! Liveness from stored edges (`spec/incr.md` IN-29, IN-71a; IN-70 step
//! U6). WP-213 (U-1): the type [`LiveDelta`] and the entry point [`live`];
//! WP-238 Part 3 fills [`live`]: the OLD walk over the stored rows, checked
//! against `META.live_hash` (S4), and the NEW walk over the stored rows of
//! the unchanged files with the changed files' fresh rows and the
//! resolution's delta laid over them. Both are ONE walk ([`walk`]) over
//! ONE narrow source of rows and keys ([`Source`]), which the stored state
//! ([`Stored`]) and the overlay ([`Overlay`]) implement.

use std::collections::{HashMap, HashSet};

use mortar_core::incr::{IncrReason, OptionKeys, Unimplemented};
use mortar_core::{FileId, SectionId, SymbolId};

use super::resolve::ResolveDelta;
use super::rows::{EdgeRow, FileTable, ResolvedRow};
use super::view::{Change, ChangedFiles, StateView};

/// The sections whose liveness flips (WP-211 §C `LiveDelta`; IN-71a,
/// IN-27's liveness row), in the update's id space. Without
/// `--gc-sections` both lists are empty. Written by U-3; read by the join
/// (`SectionChange::live`), U-4, U-3 (`refill_set`).
///
/// WP-238 Part 3: the two sets are the exact difference of the old walk's
/// live set (stored ids) and the new walk's (the update's ids: stored ids
/// for unchanged files, `ChangedFile::first_section + k` for the changed,
/// appeared, drawn-in and extracted files). So a live section of a
/// replaced file is in `now_dead` by its OLD id, and a live section of a
/// file with fresh rows is in `now_live` by its NEW id.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct LiveDelta {
    /// Dead before, live now (sorted).
    pub now_live: Vec<SectionId>,
    /// Live before, dead now (sorted).
    pub now_dead: Vec<SectionId>,
    /// Unchanged files with a section in `now_live`/`now_dead` or whose
    /// split sections' marked offsets changed (sorted): the join reads
    /// each as `Change::DrawnIn` (IN-72 item 2a, R-4) before U-4.
    pub drawn_in: Vec<FileId>,
    /// Per live split section of a changed or drawn-in file, the offsets
    /// the new walk marked (sorted; `u32::MAX`: every piece): U-4 item 6,
    /// `FRAG_USE` (WP-239).
    pub pieces: Vec<(SectionId, Vec<u32>)>,
    /// The new walk's live-set hash (S4), for `META.live_hash`.
    pub live_hash: [u8; 16],
}

/// The update's liveness (WP-211 §C; IN-71a). Called by the join after
/// `resolve` (IN-70 U6). `options`: what the update reads of the plan,
/// passed in (WP-215 D3-D5). [`live_why`] with the detail dropped.
///
/// # Errors
/// As [`live_why`].
pub fn live(
    view: &StateView<'_>,
    changed: &ChangedFiles,
    resolved: &ResolveDelta,
    options: &OptionKeys,
) -> Result<LiveDelta, IncrReason> {
    live_why(view, changed, resolved, options).map_err(|(e, _)| e)
}

/// [`live`] with the refusal's detail (the rule's word first, as the
/// oracle's `refused <rule>: ...` line gives it).
///
/// Without `--gc-sections`: the empty delta, nothing read. With it:
/// 1. the refusals by option (`gc-keep-exported` R-5, `defsym-symbol` F9)
///    and a state captured without the rows (`no-live-hash`);
/// 2. the OLD walk over the stored rows (a `REFUSE` row of a file that is
///    not replaced: S5's `fde-global` / `def-dropped`), its hash compared
///    with `META.live_hash` before anything else is used (S4:
///    `state-corrupt` on a difference);
/// 3. the NEW walk over the overlay: the stored rows of the files not in
///    `changed`, the fresh rows of the changed, appeared, drawn-in and
///    extracted files (a fresh `REFUSE` row refuses), the gone and no
///    longer extracted files left out, `RESOLVED` overridden by
///    `resolved.resolved` for the affected keys (S2: the same reader);
/// 4. the delta of the two and the new walk's hash (F14's occurrence over
///    the files in link order, `FILES.rank`).
///
/// # Errors
/// `update-not-implemented (live)` by a named rule (the detail begins with
/// it: `gc-keep-exported`, `defsym-symbol`, `no-live-hash`, `fde-global`,
/// `def-dropped`, `def-unread`); `state-corrupt` for rows the walk cannot
/// follow or a hash that differs.
pub fn live_why(
    view: &StateView<'_>,
    changed: &ChangedFiles,
    resolved: &ResolveDelta,
    options: &OptionKeys,
) -> Result<LiveDelta, Fail> {
    if !options.gc_sections {
        return Ok(LiveDelta::default());
    }
    refuse_options(options)?;
    // A state captured without the rows: its hash is all zeros (S4).
    let stored = view.meta().map_err(io)?.live_hash;
    if stored == [0; 16] {
        return Err(refused(
            "no-live-hash",
            "no-live-hash: the state carries no liveness rows (META.live_hash is zero) under \
             --gc-sections"
                .into(),
        ));
    }
    // The OLD walk and S4's check, before anything else is trusted.
    let old = walk(&Stored { view }, options)?;
    if live_hash(view, &old.live, &old.pieces).map_err(io)? != stored {
        return Err(corrupt("META.live_hash: the stored rows do not give the stored live set"));
    }
    // The NEW walk over the overlay.
    let ov = Overlay::new(view, changed, resolved)?;
    let new = walk(&ov, options)?;
    let hash = sum_files(&ov.hash_files()?, &new.live, &new.pieces);
    ov.delta(&old, &new, hash)
}

// ---- WP-238 Part 2: the stored-form walk (S2, S3) and the hash (S4) ----

/// What [`walk_stored`] finds: the live set the stored rows give.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct StoredWalk {
    /// Per `SECTIONS` row: marked.
    pub live: Vec<bool>,
    /// Per marked split section (a `SPLIT` row), the offsets marked:
    /// sorted, distinct, `[u32::MAX]` when every piece (S4's lists).
    pub pieces: Vec<(SectionId, Vec<u32>)>,
}

/// A refusal or corruption with its detail.
pub type Fail = (IncrReason, String);

/// Every piece of a split section (`mortar_core::split::EVERY_PIECE`).
const WHOLE: u32 = mortar_core::split::EVERY_PIECE;

/// A v1 refusal of the walk (S5, R-5, F9): the rule's word and the
/// detail; `UpdateNotImplemented(Live)`.
fn refused(rule: &str, detail: String) -> Fail {
    debug_assert!(detail.starts_with(rule));
    (IncrReason::UpdateNotImplemented(Some(Unimplemented::Live)), detail)
}

fn corrupt(what: impl std::fmt::Display) -> Fail {
    let what = what.to_string();
    (IncrReason::StateCorrupt(what.clone()), format!("state-corrupt: {what}"))
}

/// A failure of the rows' reader, with its report as the detail.
#[allow(clippy::needless_pass_by_value, reason = "used as a map_err adapter")]
fn io(e: IncrReason) -> Fail {
    let d = format!("state-corrupt: {}", e.report());
    (e, d)
}

/// The option refusals (R-5, F9), before any row is read.
fn refuse_options(options: &OptionKeys) -> Result<(), Fail> {
    if options.gc_keep_exported {
        return Err(refused(
            "gc-keep-exported",
            "gc-keep-exported: --gc-keep-exported roots every exported definition (R-5)".into(),
        ));
    }
    if let Some(d) = options.defsym.iter().find(|d| d.value.base().is_some()) {
        return Err(refused(
            "defsym-symbol",
            format!(
                "defsym-symbol: --defsym {} names a symbol (F9)",
                String::from_utf8_lossy(&d.name)
            ),
        ));
    }
    Ok(())
}

/// A `FILES` row's name as a diagnostic shows it: `path` or
/// `path(member)`.
fn file_name(view: &StateView<'_>, f: FileId) -> Result<String, IncrReason> {
    let row = view.file(f)?;
    let path = String::from_utf8_lossy(&view.string(row.path)?).into_owned();
    if row.member == 0 {
        return Ok(path);
    }
    Ok(format!("{path}({})", String::from_utf8_lossy(&view.string(row.member_name)?)))
}

/// Whether `f` was removed by an update (its rows are dead).
fn gone(view: &StateView<'_>, f: FileId) -> Result<bool, IncrReason> {
    Ok(view.file(f)?.flags & super::rows::file_flags::GONE != 0 || view.file_index(f)?.gone)
}

/// `options.live_roots()` through `find_key` (WP-238 F1): per root name,
/// in order, its `SYMBOLS` row (`None`: no row of that name and version).
/// The ONE lookup of the walk's option roots, and the F1 test's subject.
///
/// # Errors
/// What `find_key` reads.
pub fn live_root_keys(
    view: &StateView<'_>,
    options: &OptionKeys,
) -> Result<Vec<Option<SymbolId>>, IncrReason> {
    options
        .live_roots()
        .into_iter()
        .map(|(name, _)| {
            let (name, version) = super::extract::split_version(name);
            view.find_key(name, version)
        })
        .collect()
}

/// The stored-form walk of WP-238 S2 and S3 over `view`'s `EDGES` alone
/// (with `RESOLVED`, `SYMBOLS`, `SECTIONS.name` and `NAMES`): [`walk`] over
/// the stored state. The full link's oracle (`MORTAR_INCR_LIVE_CHECK`,
/// P0-7) runs it over what the capture writes; it is [`live`]'s old walk.
///
/// # Errors
/// As [`walk`].
pub fn walk_stored(view: &StateView<'_>, options: &OptionKeys) -> Result<StoredWalk, Fail> {
    refuse_options(options)?;
    walk(&Stored { view }, options)
}

/// Where a file's rows come from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum FileAt {
    /// The stored `EDGES` range of this `FILES` row.
    Stored(FileId),
    /// The fresh rows of the overlay's `fresh[j]`.
    Fresh(usize),
}

/// The walk's ONE source of rows and keys (S2: one key reader for the old
/// walk, the new walk and the oracle). Generic in [`walk`]: no `dyn` in
/// the edge loop.
trait Source {
    /// The walk's section ids are `0 .. sections()`.
    fn sections(&self) -> Result<usize, Fail>;
    /// The files whose rows the walk reads.
    fn files(&self) -> Result<Vec<FileAt>, Fail>;
    /// A file's rows, every id in the walk's space.
    fn edges(&self, f: FileAt) -> Result<Vec<EdgeRow>, Fail>;
    /// A file's name for a refusal's detail.
    fn file_name(&self, f: FileAt) -> Result<String, Fail>;
    /// A section's name (a `NAMED` row's source).
    fn section_name(&self, s: u32) -> Result<Vec<u8>, Fail>;
    /// A key's `RESOLVED` row as the walk reads it.
    fn resolved(&self, k: SymbolId) -> Result<ResolvedRow, Fail>;
    /// A key's name (an `INTERNAL` key's start/stop test).
    fn key_name(&self, k: SymbolId) -> Result<Vec<u8>, Fail>;
    /// [`live_root_keys`] in the walk's key space.
    fn root_keys(&self, options: &OptionKeys) -> Result<Vec<Option<SymbolId>>, Fail>;
}

/// The stored state alone (the old walk, the oracle).
struct Stored<'v, 's> {
    view: &'v StateView<'s>,
}

impl Source for Stored<'_, '_> {
    fn sections(&self) -> Result<usize, Fail> {
        usize::try_from(self.view.count(crate::format::kind::SECTIONS))
            .map_err(|_| corrupt("SECTIONS over 2^32"))
    }
    fn files(&self) -> Result<Vec<FileAt>, Fail> {
        let files = u32::try_from(self.view.count(crate::format::kind::FILES))
            .map_err(|_| corrupt("FILES over 2^32"))?;
        let mut out = Vec::with_capacity(files as usize);
        for f in (0..files).map(FileId) {
            if !gone(self.view, f).map_err(io)? {
                out.push(FileAt::Stored(f));
            }
        }
        Ok(out)
    }
    fn edges(&self, f: FileAt) -> Result<Vec<EdgeRow>, Fail> {
        match f {
            FileAt::Stored(f) => self.view.edges_of(f).map_err(io),
            FileAt::Fresh(_) => Err(corrupt("a fresh file in the stored walk")),
        }
    }
    fn file_name(&self, f: FileAt) -> Result<String, Fail> {
        match f {
            FileAt::Stored(f) => file_name(self.view, f).map_err(io),
            FileAt::Fresh(_) => Err(corrupt("a fresh file in the stored walk")),
        }
    }
    fn section_name(&self, s: u32) -> Result<Vec<u8>, Fail> {
        let name = self.view.section(SectionId(s)).map_err(io)?.name;
        self.view.string(name).map_err(io)
    }
    fn resolved(&self, k: SymbolId) -> Result<ResolvedRow, Fail> {
        self.view.resolved(k).map_err(io)
    }
    fn key_name(&self, k: SymbolId) -> Result<Vec<u8>, Fail> {
        let sym = self.view.symbol(k).map_err(io)?;
        self.view.string(sym.name).map_err(io)
    }
    fn root_keys(&self, options: &OptionKeys) -> Result<Vec<Option<SymbolId>>, Fail> {
        live_root_keys(self.view, options).map_err(io)
    }
}

/// The walk of WP-238 S2 and S3 over `src`: the roots are every `ROOT`
/// row's source and `options.live_roots()`; every key-target row and every
/// rooted key goes through the ONE key reader ([`Walker::read_key`]); a
/// section-target row marks its target (at its offset when split); a
/// `LINK_ORDER` or `GROUP` row marks every piece.
///
/// # Errors
/// `update-not-implemented (live)` by a named rule, with the detail (a
/// `REFUSE` row's `fde-global` or `def-dropped`, S5; the overlay's
/// `def-unread`); `state-corrupt` for a row the reader cannot follow.
fn walk<S: Source>(src: &S, options: &OptionKeys) -> Result<StoredWalk, Fail> {
    use super::rows::edge_kind::{self, refuse};
    let n = src.sections()?;
    let mut rows: Vec<EdgeRow> = Vec::new();
    for f in src.files()? {
        let edges = src.edges(f)?;
        if let Some(e) = edges.iter().find(|e| e.kind == edge_kind::REFUSE) {
            let name = src.file_name(f)?;
            return Err(match e.target {
                refuse::FDE_GLOBAL => refused(
                    "fde-global",
                    format!(
                        "fde-global: an FDE of {name} names its code through a non-local symbol"
                    ),
                ),
                _ => refused(
                    "def-dropped",
                    format!(
                        "def-dropped: {name} defines a non-local symbol in a section that is not kept"
                    ),
                ),
            });
        }
        rows.extend(edges);
    }
    let mut split = vec![false; n];
    let mut by_name: HashMap<Vec<u8>, Vec<u32>> = HashMap::new();
    // Rows by source: `first[s]..first[s + 1]`.
    let mut first = vec![0u32; n + 1];
    for e in &rows {
        let s = e.source as usize;
        if s >= n {
            return Err(corrupt(format!("EDGES: source {s} past SECTIONS")));
        }
        first[s + 1] += 1;
        match e.kind {
            edge_kind::SPLIT => split[s] = true,
            edge_kind::NAMED => {
                by_name.entry(src.section_name(e.source)?).or_default().push(e.source);
            }
            _ => {}
        }
    }
    for s in 0..n {
        first[s + 1] += first[s];
    }
    rows.sort_by_key(|e| e.source);
    let defsyms: HashSet<&[u8]> = options.defsym_names().collect();
    let mut w = Walker {
        src,
        live: vec![false; n],
        split,
        at: Vec::new(),
        stack: Vec::new(),
        by_name,
        defsyms,
    };
    for e in rows.iter().filter(|e| e.kind == edge_kind::ROOT) {
        w.mark(e.source, WHOLE)?;
    }
    for k in src.root_keys(options)?.into_iter().flatten() {
        w.read_key(k)?;
    }
    while let Some(s) = w.stack.pop() {
        let span = first[s as usize] as usize..first[s as usize + 1] as usize;
        for e in rows.get(span).unwrap_or_default() {
            match edge_kind::target(e.kind) {
                Some(edge_kind::Target::Key) => w.read_key(SymbolId(e.target))?,
                Some(edge_kind::Target::Section) => {
                    let at = if matches!(e.kind, edge_kind::LINK_ORDER | edge_kind::GROUP) {
                        WHOLE
                    } else {
                        e.offset
                    };
                    w.mark(e.target, at)?;
                }
                Some(edge_kind::Target::Zero | edge_kind::Target::Code) => {}
                None => return Err(corrupt(format!("EDGES: kind {}", e.kind))),
            }
        }
    }
    Ok(StoredWalk { pieces: pieces_of(w.at), live: w.live })
}

/// S4's lists from `(section, offset)` pairs: by section, each sorted and
/// distinct, `[u32::MAX]` when it holds `u32::MAX`.
fn pieces_of(mut at: Vec<(SectionId, u32)>) -> Vec<(SectionId, Vec<u32>)> {
    at.sort_unstable();
    at.dedup();
    let mut out: Vec<(SectionId, Vec<u32>)> = Vec::new();
    for (s, o) in at {
        match out.last_mut() {
            Some((last, v)) if *last == s => v.push(o),
            _ => out.push((s, vec![o])),
        }
    }
    for (_, v) in &mut out {
        if v.last() == Some(&WHOLE) {
            *v = vec![WHOLE];
        }
    }
    out
}

/// The walk's state.
struct Walker<'a, 'o, S> {
    src: &'a S,
    live: Vec<bool>,
    split: Vec<bool>,
    at: Vec<(SectionId, u32)>,
    stack: Vec<u32>,
    by_name: HashMap<Vec<u8>, Vec<u32>>,
    defsyms: HashSet<&'o [u8]>,
}

impl<S: Source> Walker<'_, '_, S> {
    /// Marks section `s`, and in a split section the offset `at`.
    fn mark(&mut self, s: u32, at: u32) -> Result<(), Fail> {
        let Some(cell) = self.live.get_mut(s as usize) else {
            return Err(corrupt(format!("EDGES: section {s} past SECTIONS")));
        };
        if !*cell {
            *cell = true;
            self.stack.push(s);
        }
        if self.split[s as usize] {
            self.at.push((SectionId(s), at));
        }
        Ok(())
    }

    /// THE key reader (S2, F2), by `RESOLVED.def_kind` only (the row's
    /// kind is not read): a definition in a section marks it, at the
    /// piece of its value when split ([`mortar_core::split::piece_offset`],
    /// F12: the full mark's conversion); a linker-defined `__start_N` /
    /// `__stop_N` (N a C identifier, not a `--defsym` name) marks every
    /// `NAMED` section named N whole; anything else marks nothing.
    fn read_key(&mut self, k: SymbolId) -> Result<(), Fail> {
        use crate::format::def_kind;
        let r = self.src.resolved(k)?;
        match r.kind {
            def_kind::UNDEFINED | def_kind::ABSOLUTE | def_kind::COMMON | def_kind::SHARED => {
                Ok(())
            }
            def_kind::SECTION => {
                let s = r.section;
                let split = self.split.get(s as usize).copied().unwrap_or(false);
                self.mark(s, if split { mortar_core::split::piece_offset(r.value) } else { WHOLE })
            }
            def_kind::INTERNAL => {
                let name = self.src.key_name(k)?;
                let sec = name.strip_prefix(b"__start_").or_else(|| name.strip_prefix(b"__stop_"));
                let Some(sec) = sec.filter(|n| c_identifier(n)) else { return Ok(()) };
                if self.defsyms.contains(name.as_slice()) {
                    return Ok(());
                }
                let named = self.by_name.get(sec).cloned().unwrap_or_default();
                for s in named {
                    self.mark(s, WHOLE)?;
                }
                Ok(())
            }
            other => Err(corrupt(format!("RESOLVED: kind {other} for key {}", k.0))),
        }
    }
}

/// Whether `name` is a C identifier (`spec/layout.md` L-96).
fn c_identifier(name: &[u8]) -> bool {
    matches!(name.first(), Some(c) if c.is_ascii_alphabetic() || *c == b'_')
        && name.iter().all(|c| c.is_ascii_alphanumeric() || *c == b'_')
}

// ---- the new walk's overlay (Part 3) ----

/// One fresh file of the overlay: its position in `ChangedFiles::files`,
/// its section ids and its keys.
struct FreshFile<'c> {
    at: usize,
    first: u32,
    count: u32,
    keys: &'c [SymbolId],
}

/// The stored state with the update laid over it (the new walk): the
/// stored rows of the files not in `changed`; the fresh rows of the
/// changed, appeared, drawn-in and extracted files (sections at their new
/// ids, keys through `ResolveDelta::key_ids`); `RESOLVED` overridden for
/// the affected keys.
struct Overlay<'v, 's, 'c> {
    view: &'v StateView<'s>,
    changed: &'c ChangedFiles,
    resolved: &'c ResolveDelta,
    /// The state's `FILES`, `SECTIONS` and `SYMBOLS` counts.
    files_n: u32,
    sections_n: u32,
    symbols_n: u32,
    /// Per stored `FILES` row: in `changed` (its stored rows are not read).
    replaced: Vec<bool>,
    /// The stored section ranges of the replaced files, sorted.
    replaced_sections: Vec<(u32, u32)>,
    /// The files with fresh rows, by ascending first section.
    fresh: Vec<FreshFile<'c>>,
    /// One past the last section id of the update's space.
    end: u32,
}

impl<'v, 's, 'c> Overlay<'v, 's, 'c> {
    fn new(
        view: &'v StateView<'s>,
        changed: &'c ChangedFiles,
        resolved: &'c ResolveDelta,
    ) -> Result<Self, Fail> {
        use crate::format::kind;
        let n32 = |k: u32, what: &str| {
            u32::try_from(view.count(k)).map_err(|_| corrupt(format!("{what} over 2^32")))
        };
        let files_n = n32(kind::FILES, "FILES")?;
        let sections_n = n32(kind::SECTIONS, "SECTIONS")?;
        let symbols_n = n32(kind::SYMBOLS, "SYMBOLS")?;
        let mut replaced = vec![false; files_n as usize];
        let mut replaced_sections = Vec::new();
        let mut fresh = Vec::new();
        for (at, c) in changed.files.iter().enumerate() {
            if c.file.0 < files_n {
                replaced[c.file.0 as usize] = true;
                let r = view.file_index(c.file).map_err(io)?.ranges[FileTable::Sections as usize];
                if r.count > 0 {
                    replaced_sections.push((r.first, r.first + r.count));
                }
            }
            if matches!(c.change, Change::Gone | Change::NoLongerExtracted) {
                continue;
            }
            let count = u32::try_from(c.sections.len())
                .map_err(|_| corrupt("a changed file with 2^32 sections"))?;
            let keys = resolved
                .key_ids
                .iter()
                .find(|(f, _)| *f == c.file)
                .map_or(&[][..], |(_, k)| k.as_slice());
            fresh.push(FreshFile { at, first: c.first_section.0, count, keys });
        }
        replaced_sections.sort_unstable();
        fresh.sort_by_key(|f| f.first);
        let mut end = sections_n;
        for f in &fresh {
            if f.count == 0 {
                continue;
            }
            if f.first < end {
                return Err(corrupt(format!(
                    "a changed file's sections start at {} below {end}",
                    f.first
                )));
            }
            end = f.first.checked_add(f.count).ok_or_else(|| corrupt("section ids overflow"))?;
        }
        Ok(Self {
            view,
            changed,
            resolved,
            files_n,
            sections_n,
            symbols_n,
            replaced,
            replaced_sections,
            fresh,
            end,
        })
    }

    /// The fresh file whose new sections hold `s`.
    fn fresh_of(&self, s: u32) -> Option<&FreshFile<'c>> {
        let i = self.fresh.partition_point(|f| f.first + f.count <= s);
        self.fresh.get(i).filter(|f| f.first <= s && s < f.first + f.count)
    }

    /// Whether stored section `s` belongs to a replaced file.
    fn replaced_section(&self, s: u32) -> bool {
        let i = self.replaced_sections.partition_point(|r| r.1 <= s);
        self.replaced_sections.get(i).is_some_and(|r| r.0 <= s && s < r.1)
    }

    /// S4's file list in link order (`FILES.rank`; `FileId` order is rank
    /// order in the state, an appeared file takes its neighbours'
    /// midpoint): the stored non-gone files not gone in the update, the
    /// appeared files; each with its new section range.
    fn hash_files(&self) -> Result<Vec<HashFile>, Fail> {
        let mut out: Vec<(u64, u32, HashFile)> = Vec::new();
        let by_file: HashMap<FileId, usize> =
            self.changed.files.iter().enumerate().map(|(i, c)| (c.file, i)).collect();
        for f in (0..self.files_n).map(FileId) {
            if gone(self.view, f).map_err(io)? {
                continue;
            }
            let row = self.view.file(f).map_err(io)?;
            let ident = (
                self.view.string(row.path).map_err(io)?,
                self.view.string(row.member_name).map_err(io)?,
                row.member_ordinal,
            );
            let (first, count) = match by_file.get(&f).map(|&i| &self.changed.files[i]) {
                Some(c) if c.change == Change::Gone => continue,
                Some(c) if c.change == Change::NoLongerExtracted => (0, 0),
                Some(c) => (c.first_section.0, u32::try_from(c.sections.len()).unwrap_or(0)),
                None => {
                    let r =
                        self.view.file_index(f).map_err(io)?.ranges[FileTable::Sections as usize];
                    (r.first, r.count)
                }
            };
            out.push((row.rank, f.0, HashFile { ident, first, count }));
        }
        for &a in &self.changed.appeared {
            let Some(c) = by_file.get(&a).map(|&i| &self.changed.files[i]) else {
                return Err(corrupt(format!("appeared file {} has no rows", a.0)));
            };
            let s = |x| self.changed.string(self.view, x).map_err(io);
            let ident = (s(c.row.path)?, s(c.row.member_name)?, c.row.member_ordinal);
            let count = u32::try_from(c.sections.len()).unwrap_or(0);
            out.push((c.row.rank, a.0, HashFile { ident, first: c.first_section.0, count }));
        }
        out.sort_by_key(|x| (x.0, x.1));
        Ok(out.into_iter().map(|x| x.2).collect())
    }

    /// The delta of the old walk (stored ids) and the new (update ids).
    fn delta(
        &self,
        old: &StoredWalk,
        new: &StoredWalk,
        live_hash: [u8; 16],
    ) -> Result<LiveDelta, Fail> {
        let was = |s: usize| old.live.get(s).copied().unwrap_or(false);
        let is = |s: usize| new.live.get(s).copied().unwrap_or(false);
        let n = old.live.len().max(new.live.len());
        let id = |s: usize| SectionId(u32::try_from(s).unwrap_or(u32::MAX));
        let now_live: Vec<SectionId> = (0..n).filter(|&s| is(s) && !was(s)).map(id).collect();
        let now_dead: Vec<SectionId> = (0..n).filter(|&s| was(s) && !is(s)).map(id).collect();
        // Unchanged files with a flipped section or changed pieces.
        let mut drawn: Vec<FileId> = Vec::new();
        let mut note = |s: SectionId| -> Result<(), Fail> {
            if s.0 < self.sections_n && !self.replaced_section(s.0) {
                drawn.push(FileId(self.view.section(s).map_err(io)?.file));
            }
            Ok(())
        };
        for &s in now_live.iter().chain(&now_dead) {
            note(s)?;
        }
        let old_p: HashMap<SectionId, &Vec<u32>> =
            old.pieces.iter().map(|(s, v)| (*s, v)).collect();
        let new_p: HashMap<SectionId, &Vec<u32>> =
            new.pieces.iter().map(|(s, v)| (*s, v)).collect();
        for (s, v) in &old.pieces {
            if new_p.get(s) != Some(&v) {
                note(*s)?;
            }
        }
        for (s, v) in &new.pieces {
            if old_p.get(s) != Some(&v) {
                note(*s)?;
            }
        }
        drawn.sort_unstable();
        drawn.dedup();
        let mut pieces = Vec::new();
        for (s, v) in &new.pieces {
            let keep = if s.0 >= self.sections_n {
                true
            } else {
                let f = FileId(self.view.section(*s).map_err(io)?.file);
                drawn.binary_search(&f).is_ok()
            };
            if keep {
                pieces.push((*s, v.clone()));
            }
        }
        Ok(LiveDelta { now_live, now_dead, drawn_in: drawn, pieces, live_hash })
    }
}

impl Source for Overlay<'_, '_, '_> {
    fn sections(&self) -> Result<usize, Fail> {
        Ok(self.end as usize)
    }
    fn files(&self) -> Result<Vec<FileAt>, Fail> {
        let mut out = Vec::new();
        for f in (0..self.files_n).map(FileId) {
            if !self.replaced[f.0 as usize] && !gone(self.view, f).map_err(io)? {
                out.push(FileAt::Stored(f));
            }
        }
        out.extend((0..self.fresh.len()).map(FileAt::Fresh));
        Ok(out)
    }
    fn edges(&self, f: FileAt) -> Result<Vec<EdgeRow>, Fail> {
        use super::rows::edge_kind::{self, Target};
        let ff = match f {
            FileAt::Stored(f) => return self.view.edges_of(f).map_err(io),
            FileAt::Fresh(j) => &self.fresh[j],
        };
        let c = &self.changed.files[ff.at];
        let own = |s: u32| ff.first <= s && s < ff.first + ff.count;
        let mut out = Vec::with_capacity(c.edges.len());
        for e in &c.edges {
            if !own(e.source) {
                return Err(corrupt(format!(
                    "a fresh EDGES row of file {} has source {} outside its sections",
                    c.file.0, e.source
                )));
            }
            let mut e = *e;
            match edge_kind::target(e.kind) {
                Some(Target::Key) => {
                    let Some(&k) = ff.keys.get(e.target as usize) else {
                        return Err(corrupt(format!(
                            "a fresh EDGES row of file {} names key {} past its keys",
                            c.file.0, e.target
                        )));
                    };
                    e.target = k.0;
                }
                Some(Target::Section) if !own(e.target) => {
                    return Err(corrupt(format!(
                        "a fresh EDGES row of file {} targets section {} outside its sections",
                        c.file.0, e.target
                    )));
                }
                Some(_) => {}
                None => return Err(corrupt(format!("EDGES: kind {}", e.kind))),
            }
            out.push(e);
        }
        Ok(out)
    }
    fn file_name(&self, f: FileAt) -> Result<String, Fail> {
        match f {
            FileAt::Stored(f) => file_name(self.view, f).map_err(io),
            FileAt::Fresh(j) => {
                let c = &self.changed.files[self.fresh[j].at];
                if c.file.0 < self.files_n {
                    return file_name(self.view, c.file).map_err(io);
                }
                let s = |x| self.changed.string(self.view, x).map_err(io);
                let path = String::from_utf8_lossy(&s(c.row.path)?).into_owned();
                if c.row.member == 0 {
                    return Ok(path);
                }
                Ok(format!("{path}({})", String::from_utf8_lossy(&s(c.row.member_name)?)))
            }
        }
    }
    fn section_name(&self, s: u32) -> Result<Vec<u8>, Fail> {
        if s < self.sections_n {
            return Stored { view: self.view }.section_name(s);
        }
        let Some(ff) = self.fresh_of(s) else {
            return Err(corrupt(format!("section {s} is in no fresh file")));
        };
        let row = &self.changed.files[ff.at].sections[(s - ff.first) as usize];
        self.changed.string(self.view, row.name).map_err(io)
    }
    fn resolved(&self, k: SymbolId) -> Result<ResolvedRow, Fail> {
        use crate::format::def_kind;
        let over = self.resolved.resolved.binary_search_by_key(&k, |r| r.0).ok();
        if let Some(i) = over {
            let r = self.resolved.resolved[i].1;
            if r.kind == def_kind::SECTION && r.section < self.sections_n {
                // A definition at a STORED id: its file's rows are the
                // stored ones, made under the old resolution, so whether
                // the section is kept is only known when it is the stored
                // definition (F11 on the reader's side: never a silent
                // miss). resolve draws in a file that becomes a definer.
                let before = if k.0 < self.symbols_n {
                    Some(self.view.resolved(k).map_err(io)?)
                } else {
                    None
                };
                let same =
                    before.is_some_and(|b| b.kind == def_kind::SECTION && b.section == r.section);
                if !same || self.replaced_section(r.section) {
                    return Err(refused(
                        "def-unread",
                        format!(
                            "def-unread: key {} is now defined in section {} of a file whose rows were not re-read",
                            k.0, r.section
                        ),
                    ));
                }
            }
            if r.kind == def_kind::SECTION
                && r.section >= self.sections_n
                && self.fresh_of(r.section).is_none()
            {
                return Err(corrupt(format!(
                    "RESOLVED: key {} names section {} of no file",
                    k.0, r.section
                )));
            }
            return Ok(r);
        }
        if k.0 >= self.symbols_n {
            return Err(corrupt(format!("a new key {} without a RESOLVED row", k.0)));
        }
        let r = self.view.resolved(k).map_err(io)?;
        if r.kind == def_kind::SECTION && self.replaced_section(r.section) {
            return Err(corrupt(format!(
                "key {} is not affected but is defined in a replaced file's section {}",
                k.0, r.section
            )));
        }
        Ok(r)
    }
    fn key_name(&self, k: SymbolId) -> Result<Vec<u8>, Fail> {
        if k.0 < self.symbols_n {
            return Stored { view: self.view }.key_name(k);
        }
        let i = (k.0 - self.symbols_n) as usize;
        self.resolved
            .new_keys
            .get(i)
            .map(|n| n.name.clone())
            .ok_or_else(|| corrupt(format!("key {} past the new keys", k.0)))
    }
    fn root_keys(&self, options: &OptionKeys) -> Result<Vec<Option<SymbolId>>, Fail> {
        let stored = live_root_keys(self.view, options).map_err(io)?;
        let names = options.live_roots();
        let mut out = Vec::with_capacity(stored.len());
        for (k, (name, _)) in stored.into_iter().zip(names) {
            if k.is_some() {
                out.push(k);
                continue;
            }
            let (name, version) = super::extract::split_version(name);
            let at =
                self.resolved.new_keys.iter().position(|n| n.name == name && n.version == version);
            out.push(at.map(|i| SymbolId(self.symbols_n + u32::try_from(i).unwrap_or(u32::MAX))));
        }
        Ok(out)
    }
}

// ---- S4's hash ----

/// One file of S4's sum: its identity and its section range.
struct HashFile {
    ident: (Vec<u8>, Vec<u8>, u32),
    first: u32,
    count: u32,
}

/// S4's live-set hash `H` of `view`'s files, `live` per `SECTIONS` row
/// and `pieces` per live split section (sorted by section, S4's lists):
/// the wrapping 128-bit little-endian sum over the files with a live
/// section of `L128(enc_f)`, `enc_f` = `u32` length and bytes of
/// `FILES.path`, of `FILES.member_name`, `u32` `member_ordinal`, `u32`
/// OCCURRENCE (F14: the earlier live `FILES` rows of the same path,
/// member name and ordinal), then per live section in ascending
/// `sh_index` its `sh_index`, `u32` n and n offsets. No row id enters it:
/// compaction leaves it unchanged. The capture's side, the old walk's and
/// (through the same [`sum_files`]) the new walk's.
///
/// # Errors
/// `state-corrupt` from the rows.
pub fn live_hash(
    view: &StateView<'_>,
    live: &[bool],
    pieces: &[(SectionId, Vec<u32>)],
) -> Result<[u8; 16], IncrReason> {
    let files = u32::try_from(view.count(crate::format::kind::FILES))
        .map_err(|_| IncrReason::StateCorrupt("FILES over 2^32".into()))?;
    let mut list = Vec::with_capacity(files as usize);
    for f in (0..files).map(FileId) {
        if gone(view, f)? {
            continue;
        }
        let row = view.file(f)?;
        let ident = (view.string(row.path)?, view.string(row.member_name)?, row.member_ordinal);
        let range = view.file_index(f)?.ranges[FileTable::Sections as usize];
        list.push(HashFile { ident, first: range.first, count: range.count });
    }
    Ok(sum_files(&list, live, pieces))
}

/// S4's sum over `files` in link order.
fn sum_files(files: &[HashFile], live: &[bool], pieces: &[(SectionId, Vec<u32>)]) -> [u8; 16] {
    let mut seen: HashMap<(&[u8], &[u8], u32), u32> = HashMap::new();
    let mut sum: u128 = 0;
    let put = |b: &mut Vec<u8>, v: u32| b.extend_from_slice(&v.to_le_bytes());
    for f in files {
        let ident = (f.ident.0.as_slice(), f.ident.1.as_slice(), f.ident.2);
        let occurrence = seen.get(&ident).copied().unwrap_or(0);
        let mut body = Vec::new();
        for k in 0..f.count {
            let s = f.first + k;
            if !live.get(s as usize).copied().unwrap_or(false) {
                continue;
            }
            put(&mut body, k);
            let at = pieces.binary_search_by_key(&SectionId(s), |p| p.0).ok();
            let list = at.map_or(&[][..], |i| pieces[i].1.as_slice());
            put(&mut body, u32::try_from(list.len()).unwrap_or(u32::MAX));
            for &o in list {
                put(&mut body, o);
            }
        }
        if !body.is_empty() {
            let mut enc = Vec::with_capacity(body.len() + ident.0.len() + ident.1.len() + 16);
            put(&mut enc, u32::try_from(ident.0.len()).unwrap_or(u32::MAX));
            enc.extend_from_slice(ident.0);
            put(&mut enc, u32::try_from(ident.1.len()).unwrap_or(u32::MAX));
            enc.extend_from_slice(ident.1);
            put(&mut enc, ident.2);
            put(&mut enc, occurrence);
            enc.extend_from_slice(&body);
            let mut h = mortar_digest::L128::new();
            h.update(&enc);
            sum = sum.wrapping_add(u128::from_le_bytes(h.finish()));
        }
        seen.insert(ident, occurrence + 1);
    }
    sum.to_le_bytes()
}
