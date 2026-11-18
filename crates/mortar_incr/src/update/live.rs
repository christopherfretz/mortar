//! Liveness from stored edges (`spec/incr.md` IN-29, IN-71a; IN-70 step
//! U6). WP-213 (U-1): the type [`LiveDelta`] and the entry point [`live`];
//! **filled by U-3** (the function only).

use mortar_core::incr::{IncrReason, OptionKeys, Unimplemented};
use mortar_core::{FileId, SectionId, SymbolId};

use super::resolve::ResolveDelta;
use super::view::{ChangedFiles, StateView};

/// The sections whose liveness flips (WP-211 §C `LiveDelta`; IN-71a,
/// IN-27's liveness row), in the update's id space. Without
/// `--gc-sections` both lists are empty. Written by U-3; read by the join
/// (`SectionChange::live`), U-4, U-3 (`refill_set`).
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
/// passed in (WP-215 D3-D5).
///
/// IN-29's stored graph and IN-71a's walk are not built (WP-215 seam 3,
/// G3; [later] WP-238): without `--gc-sections` the delta is empty and
/// nothing is read; with it, every update falls back, decided from the
/// option alone. Never "empty" under `--gc-sections`: a section of an
/// unchanged file that a change makes live would be missed.
///
/// # Errors
/// `update-not-implemented (live)` under `--gc-sections`.
pub fn live(
    view: &StateView<'_>,
    changed: &ChangedFiles,
    resolved: &ResolveDelta,
    options: &OptionKeys,
) -> Result<LiveDelta, IncrReason> {
    let _ = (view, changed, resolved);
    if options.gc_sections {
        return Err(IncrReason::UpdateNotImplemented(Some(Unimplemented::Live)));
    }
    Ok(LiveDelta::default())
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

/// Every piece of a split section (`mortar_core::split::EVERY_PIECE`).
const WHOLE: u32 = mortar_core::split::EVERY_PIECE;

/// A v1 refusal of the walk (S5, R-5, F9): the rule's word and the
/// detail; `UpdateNotImplemented(Live)`.
fn refused(rule: &str, detail: String) -> (IncrReason, String) {
    let _ = rule;
    (IncrReason::UpdateNotImplemented(Some(Unimplemented::Live)), detail)
}

fn corrupt(what: impl std::fmt::Display) -> (IncrReason, String) {
    let what = what.to_string();
    (IncrReason::StateCorrupt(what.clone()), format!("state-corrupt: {what}"))
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
/// (with `RESOLVED`, `SYMBOLS`, `SECTIONS.name` and `NAMES`): the roots
/// are every `ROOT` row's source and `options.live_roots()` through
/// `find_key`; every key-target row and every rooted key goes through
/// the ONE key reader ([`read_key`]); a section-target row marks its
/// target (at its offset when split); a `LINK_ORDER` or `GROUP` row
/// marks every piece. The full link's oracle (`MORTAR_INCR_LIVE_CHECK`,
/// P0-7) runs it over what the capture writes; Part 3's old and new
/// walks are this function.
///
/// # Errors
/// `update-not-implemented (live)` by a named rule, with the detail
/// (`gc-keep-exported` R-5; `defsym-symbol` F9; a `REFUSE` row's
/// `fde-global` or `def-dropped`, S5); `state-corrupt` for a row the
/// reader cannot follow.
pub fn walk_stored(
    view: &StateView<'_>,
    options: &OptionKeys,
) -> Result<StoredWalk, (IncrReason, String)> {
    use super::rows::edge_kind::{self, refuse};
    let io = |e: IncrReason| {
        let d = format!("state-corrupt: {}", e.report());
        (e, d)
    };
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
    let n = usize::try_from(view.count(crate::format::kind::SECTIONS))
        .map_err(|_| corrupt("SECTIONS over 2^32"))?;
    let files = u32::try_from(view.count(crate::format::kind::FILES))
        .map_err(|_| corrupt("FILES over 2^32"))?;
    let mut rows: Vec<super::rows::EdgeRow> = Vec::new();
    for f in (0..files).map(FileId) {
        if gone(view, f).map_err(io)? {
            continue;
        }
        let edges = view.edges_of(f).map_err(io)?;
        for e in &edges {
            if e.kind == edge_kind::REFUSE {
                let name = file_name(view, f).map_err(io)?;
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
        }
        rows.extend(edges);
    }
    let mut split = vec![false; n];
    let mut by_name: std::collections::HashMap<Vec<u8>, Vec<u32>> =
        std::collections::HashMap::new();
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
                let name = view.section(SectionId(e.source)).map_err(io)?.name;
                by_name.entry(view.string(name).map_err(io)?).or_default().push(e.source);
            }
            _ => {}
        }
    }
    for s in 0..n {
        first[s + 1] += first[s];
    }
    rows.sort_by_key(|e| e.source);
    let defsyms: std::collections::HashSet<&[u8]> = options.defsym_names().collect();
    let mut w = Walker {
        view,
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
    for k in live_root_keys(view, options).map_err(io)?.into_iter().flatten() {
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
struct Walker<'v, 's, 'o> {
    view: &'v StateView<'s>,
    live: Vec<bool>,
    split: Vec<bool>,
    at: Vec<(SectionId, u32)>,
    stack: Vec<u32>,
    by_name: std::collections::HashMap<Vec<u8>, Vec<u32>>,
    defsyms: std::collections::HashSet<&'o [u8]>,
}

impl Walker<'_, '_, '_> {
    /// Marks `SECTIONS` row `s`, and in a split section the offset `at`.
    fn mark(&mut self, s: u32, at: u32) -> Result<(), (IncrReason, String)> {
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
    fn read_key(&mut self, k: SymbolId) -> Result<(), (IncrReason, String)> {
        use crate::format::def_kind;
        let io = |e: IncrReason| {
            let d = format!("state-corrupt: {}", e.report());
            (e, d)
        };
        let r = self.view.resolved(k).map_err(io)?;
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
                let sym = self.view.symbol(k).map_err(io)?;
                let name = self.view.string(sym.name).map_err(io)?;
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

/// S4's live-set hash `H` of `view`'s files, `live` per `SECTIONS` row
/// and `pieces` per live split section (sorted by section, S4's lists):
/// the wrapping 128-bit little-endian sum over the files with a live
/// section of `L128(enc_f)`, `enc_f` = `u32` length and bytes of
/// `FILES.path`, of `FILES.member_name`, `u32` `member_ordinal`, `u32`
/// OCCURRENCE (F14: the earlier live `FILES` rows of the same path,
/// member name and ordinal), then per live section in ascending
/// `sh_index` its `sh_index`, `u32` n and n offsets. No row id enters it:
/// compaction leaves it unchanged. The capture's side and the walk's are
/// this function (with the full mark's sets, P0-3, or the walk's).
///
/// # Errors
/// `state-corrupt` from the rows.
pub fn live_hash(
    view: &StateView<'_>,
    live: &[bool],
    pieces: &[(SectionId, Vec<u32>)],
) -> Result<[u8; 16], IncrReason> {
    use super::rows::FileTable;
    let files = u32::try_from(view.count(crate::format::kind::FILES))
        .map_err(|_| IncrReason::StateCorrupt("FILES over 2^32".into()))?;
    let mut seen: std::collections::HashMap<(Vec<u8>, Vec<u8>, u32), u32> =
        std::collections::HashMap::new();
    let mut sum: u128 = 0;
    let put = |b: &mut Vec<u8>, v: u32| b.extend_from_slice(&v.to_le_bytes());
    for f in (0..files).map(FileId) {
        if gone(view, f)? {
            continue;
        }
        let row = view.file(f)?;
        let path = view.string(row.path)?;
        let member = view.string(row.member_name)?;
        let ident = (path, member, row.member_ordinal);
        let occurrence = seen.get(&ident).copied().unwrap_or(0);
        let range = view.file_index(f)?.ranges[FileTable::Sections as usize];
        let mut body = Vec::new();
        for k in 0..range.count {
            let s = range.first + k;
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
            enc.extend_from_slice(&ident.0);
            put(&mut enc, u32::try_from(ident.1.len()).unwrap_or(u32::MAX));
            enc.extend_from_slice(&ident.1);
            put(&mut enc, ident.2);
            put(&mut enc, occurrence);
            enc.extend_from_slice(&body);
            let mut h = mortar_digest::L128::new();
            h.update(&enc);
            sum = sum.wrapping_add(u128::from_le_bytes(h.finish()));
        }
        seen.insert(ident, occurrence + 1);
    }
    Ok(sum.to_le_bytes())
}
