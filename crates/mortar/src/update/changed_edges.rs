//! WP-267 P-1b: the changed files' `INDEX` (M-4) and `EDGES` (M-5) rows
//! without front 1 (`spec/landed/incr/WP-267.md` §1 rows 7f and 7g; the
//! review's §4; `WIP-267-p1b.md`'s field list).
//!
//! [`changed_edges_of`] runs after U-3's `resolve` and P-1's
//! [`super::changed_rows::facts_after_resolve`] (it reads the update's
//! election through `changed_facts`, as P-1 does) and before U6, which
//! walks the rows. Per changed file:
//! - `INDEX`: `capture::index_file_of`, the body the capture's
//!   `index_file` shares, over the file's own object with the
//!   post-resolve facts (kind, kept, relocation section) and the keys by
//!   name (positions in `ChangedFile::keys`, P-1's numbering);
//! - `EDGES` (only under `--gc-sections`): `mortar_live::stored_edges`,
//!   the whole link's ONE producer, over a NARROW `LiveInputs`: a
//!   one-file link (`FileId(0)`, sections `0..count`, keys
//!   `0..keys.len()`) built from the file's object, its post-resolve facts,
//!   the `OUTSECS` names of their output sections, the merge phase's split
//!   of the file (`file_split::split_of`) and, for F11, the update's own
//!   `RESOLVED` rows of the file's keys (`ResolveDelta::resolved`). Every
//!   pass `stored` makes (`prepare`'s facts and frames, the F11 loop, the
//!   group ring) is then over that file only. The rows come back in the
//!   update's ids as `changed::translate` gives them.
//!
//! Not computed narrowly (and not needed): a row whose source is a changed
//! file's section that an UNCHANGED file's FDE adds by naming it through
//! a non-local symbol. That FDE has S5's `fde-global` `REFUSE` row in the
//! unchanged file's stored `EDGES`, on which the update's walk
//! (`mortar_incr::update::live`) refuses whatever the changed rows are.
//!
//! A plain link never calls this.

use std::collections::HashMap;

use mortar_core::assignment::{Assignment, OutOf, OutSec};
use mortar_core::column::Column;
use mortar_core::id::{
    FileId, FragId, IdRange, OptId, OutSecId, SectionId, Span, SymEntry, SymbolId,
};
use mortar_core::incr::{IncrReason, LiveEdge, LiveTarget, Unimplemented};
use mortar_core::inputs::{
    ContentHash, File, InputFlags, Inputs, LinkInputs, LinkTarget, Origin, Reader, StoreRef,
    SymbolKey,
};
use mortar_core::par::Pool;
use mortar_core::plan::Plan;
use mortar_core::resolution::{Def, DropReason, Resolution, SectionKind, SectionRow, SectionState};
use mortar_core::split::{Split, SplitRow};
use mortar_elf::consts::{
    SHF_ALLOC, SHF_GNU_RETAIN, SHT_FINI_ARRAY, SHT_INIT_ARRAY, SHT_NOTE, SHT_PREINIT_ARRAY,
};
use mortar_elf::{SectionIndex, SymbolIndex, SymbolSection};
use mortar_incr::capture;
use mortar_incr::format::{def_kind, sym_kind};
use mortar_incr::update::resolve::ResolveDelta;
use mortar_incr::update::rows::{EdgeRow, edge_kind};
use mortar_incr::update::view::{ChangedFile, ChangedFiles, StateView};
use mortar_live::{LiveInputs, LiveOptions};
use mortar_merge::phase::ArchRelocs;

use super::file_front::{FileFront, FrontCache};
use super::file_split::{SectionFact, changed_facts, split_of};

fn resolve() -> IncrReason {
    IncrReason::UpdateNotImplemented(Some(Unimplemented::Resolve))
}

fn live() -> IncrReason {
    IncrReason::UpdateNotImplemented(Some(Unimplemented::Live))
}

fn corrupt(what: impl std::fmt::Display) -> IncrReason {
    IncrReason::StateCorrupt(format!("WP-267 P-1b: {what}"))
}

/// A refusal by name: what was refused (the shadow's and the record's
/// word) and the update's reason.
type Refused = (String, IncrReason);

/// M-4 and M-5 (WP-267 P-1b): each file of `cf` gets its `index` and,
/// under `--gc-sections` (`plan`), its `edges`, from the file alone (its
/// front in `cache`, opened by P-1's `changed_files_of`), its post-resolve
/// facts (`changed_facts` with the update's keepers `delta.keepers`) and
/// the update's `RESOLVED` rows of its keys (`delta`, F11). `arch` is the
/// link's architecture (the classification of a record; an implicit
/// addend). Without `--gc-sections` `edges` stays empty, as front 1's.
///
/// # Errors
/// `update-not-implemented (live)` with the reason: a relocation section
/// liveness cannot read (the full link reports it), a key the file
/// defines in a section the update resolves to it whose entry is not
/// found; `update-not-implemented (resolve)`: a file whose front `cache`
/// does not hold, and `changed_facts`' and `split_of`'s refusals;
/// `state-corrupt` for keys that are not P-1's.
pub fn changed_edges_of(
    view: &StateView<'_>,
    cache: &FrontCache<'_>,
    cf: &mut ChangedFiles,
    delta: &ResolveDelta,
    plan: &Plan,
    arch: &dyn mortar_arch::Arch,
    pool: &Pool,
) -> Result<(), (String, IncrReason)> {
    let refused = |what: &str, reason: IncrReason| (what.to_owned(), reason);
    let gc = plan.shaping.gc_sections;
    let mut made = Vec::with_capacity(cf.files.len());
    for (j, c) in cf.files.iter().enumerate() {
        let ff = cache.get(c.file).ok_or_else(|| refused("no front", resolve()))?;
        let facts = changed_facts(ff, view, plan, &delta.keepers)
            .map_err(|e| refused("changed_facts", e))?;
        let key_of = key_positions(ff, c).map_err(|e| refused("keys", e))?;
        let index = index_of(ff, c, &facts, &key_of, arch)
            .map_err(|e| (format!("INDEX: {e}"), resolve()))?;
        let edges = if gc {
            let ids = delta
                .key_ids
                .get(j)
                .filter(|(f, ids)| *f == c.file && ids.len() == c.keys.len())
                .map(|(_, ids)| ids.as_slice())
                .ok_or_else(|| refused("keys", corrupt("key_ids are not the changed files'")))?;
            edges_of(view, ff, c, &facts, &key_of, ids, delta, plan, arch, pool)?
        } else {
            Vec::new()
        };
        made.push((index, edges));
    }
    for (c, (index, edges)) in cf.files.iter_mut().zip(made) {
        c.index = index;
        c.edges = edges;
    }
    Ok(())
}

/// The position in `c.keys` of each non-local entry's key: P-1's numbering
/// (`changed_rows::file_rows_of`: the entries' keys by (name, version),
/// first mention first), checked against `c.keys`.
fn key_positions(ff: &FileFront<'_>, c: &ChangedFile) -> Result<Vec<u32>, IncrReason> {
    let mut at: HashMap<(&[u8], Option<&[u8]>), u32> = HashMap::new();
    let mut out = Vec::with_capacity(ff.keys.len());
    for k in &ff.keys {
        let next = u32::try_from(at.len()).map_err(|_| corrupt("keys past u32"))?;
        out.push(*at.entry((k.name, k.version)).or_insert(next));
    }
    if at.len() != c.keys.len()
        || at.iter().any(|(&(name, version), &p)| {
            c.keys.get(p as usize).is_none_or(|n| {
                n.name != name
                    || n.version.as_slice() != version.unwrap_or_default()
                    || (n.kind == sym_kind::VERSIONED) != version.is_some()
            })
        })
    {
        return Err(corrupt(format!("file {}: keys are not P-1's", c.file.0)));
    }
    Ok(out)
}

/// M-4: the file's `INDEX` rows in the update's ids (units and section
/// targets `first_section + i`, key targets positions in `c.keys`), as
/// `changed::translate` gives front 1's. An error names what could not be
/// read (the full link reports it).
fn index_of(
    ff: &FileFront<'_>,
    c: &ChangedFile,
    facts: &[SectionFact],
    key_of: &[u32],
    arch: &dyn mortar_arch::Arch,
) -> Result<Vec<mortar_incr::format::IndexRow>, String> {
    let Some(table) = ff.object.symbols().map_err(|e| format!("symbols: {e}"))? else {
        return Ok(Vec::new());
    };
    let first = c.first_section.0;
    let section = |i: SectionIndex| {
        let f = facts.get(i.0 as usize)?;
        Some((first.checked_add(i.0)?, f.kind, f.kept, f.reloc))
    };
    let key = |k: usize| key_of.get(k).copied();
    let path = format!("file {}", c.file.0);
    capture::index_file_of(&ff.object, &table, arch, &section, &key, &path)
}

/// `mortar_layout::assign`'s `keep_by_rule` (model §3.4 `keep`: a root of
/// liveness by rule), over a section's type, flags and output section
/// name. A COPY: `mortar_layout` keeps it private (WP-267 P-1b asks for
/// it to be shared); the shadow compares every row this feeds.
fn keep_by_rule(sh_type: u32, flags: u64, out: &[u8]) -> bool {
    flags & SHF_GNU_RETAIN != 0
        || matches!(sh_type, SHT_INIT_ARRAY | SHT_FINI_ARRAY | SHT_PREINIT_ARRAY)
        || [&b".init"[..], b".fini", b".init_array", b".fini_array", b".preinit_array"]
            .contains(&out)
        || (sh_type == SHT_NOTE && flags & SHF_ALLOC != 0)
}

/// M-5: the file's `EDGES` rows (S1's rows whose source is one of its
/// sections) from `mortar_live::stored_edges` over the narrow one-file
/// link, in the update's ids. `ids` maps `c.keys` to the state's keys
/// (`ResolveDelta::key_ids`), for F11's `RESOLVED` rows.
#[allow(clippy::too_many_arguments, reason = "the narrow link's inputs, (a)-(c)")]
#[allow(clippy::too_many_lines, reason = "the four narrow tables, then the translation")]
fn edges_of(
    view: &StateView<'_>,
    ff: &FileFront<'_>,
    c: &ChangedFile,
    facts: &[SectionFact],
    key_of: &[u32],
    ids: &[SymbolId],
    delta: &ResolveDelta,
    plan: &Plan,
    arch: &dyn mortar_arch::Arch,
    pool: &Pool,
) -> Result<Vec<EdgeRow>, Refused> {
    let refused = |what: String, reason: IncrReason| (what, reason);
    let count = u32::try_from(facts.len()).map_err(|_| refused("sections".into(), live()))?;
    let first = c.first_section.0;
    let own = |s: SectionId| s.0.checked_sub(first).filter(|k| *k < count).map(SectionId);
    let table = ff.object.symbols().map_err(|_| refused("symbols".into(), resolve()))?;
    let nkeys = u32::try_from(c.keys.len()).map_err(|_| refused("keys".into(), live()))?;
    let globals = u32::try_from(key_of.len()).map_err(|_| refused("keys".into(), live()))?;

    // Inputs: the one object, its keys by name.
    let symbols: Vec<SymbolKey<'_>> = c
        .keys
        .iter()
        .map(|k| SymbolKey {
            name: &k.name,
            version: (k.kind == sym_kind::VERSIONED).then_some(k.version.as_slice()),
            hash: 0,
        })
        .collect();
    let file = File {
        path: b"",
        member: None,
        origin: Origin::Input { input: 0, arg: 0, entry: None },
        state: InputFlags::empty(),
        reader: Reader::Object(ff.object),
        bytes: None,
        sections: count,
        syms: Span { start: 0, len: globals },
        hash: ContentHash(0),
    };
    let inputs = Inputs {
        target: LinkTarget::of(ff.object.machine()),
        files: Column::from_vec(vec![file]).map_err(|_| refused("files".into(), live()))?,
        sym_map: key_of.iter().map(|&p| SymbolId(p)).collect(),
        symbols: Column::from_vec(symbols).map_err(|_| refused("keys".into(), live()))?,
        comdats: Column::new(),
        archives: Column::new(),
        link: LinkInputs::of_files(1),
        store: StoreRef::none(),
    };

    // Resolution: the post-resolve rows; F11's definitions from the
    // update's `RESOLVED` rows (a key the file defines in one of its
    // sections, by the entry whose section that is).
    let mut res = Resolution::empty(1, nkeys, 0);
    res.section_base = Column::from_vec(vec![IdRange::new(SectionId(0), SectionId(count))])
        .map_err(|_| refused("sections".into(), live()))?;
    res.sections = Column::from_vec(
        facts
            .iter()
            .map(|f| SectionRow {
                kind: f.kind,
                state: if f.kept {
                    SectionState::Kept
                } else {
                    SectionState::Dropped(DropReason::Comdat)
                },
                reloc: f.reloc,
                comdat: OptId::NONE,
            })
            .collect(),
    )
    .map_err(|_| refused("sections".into(), live()))?;
    if let Some(table) = &table {
        // Per (key position, section index): the first entry of the key
        // defined in that section.
        let fg = table.first_global().0;
        let mut defined: HashMap<(u32, u32), SymbolIndex> = HashMap::new();
        for (k, &q) in key_of.iter().enumerate() {
            let Some(index) = u32::try_from(k).ok().and_then(|k| fg.checked_add(k)) else {
                continue;
            };
            let index = SymbolIndex(index);
            let Ok(sym) = table.get(index) else { continue };
            if let Ok(SymbolSection::Section(i)) = table.section_of(index, &sym) {
                defined.entry((q, i.0)).or_insert(index);
            }
        }
        for (p, id) in ids.iter().enumerate() {
            let Ok(at) = delta.resolved.binary_search_by_key(id, |r| r.0) else { continue };
            let r = &delta.resolved[at].1;
            // The definer is this file exactly when the section is one of
            // its own (the update's ids: `first_section ..`).
            if r.kind != def_kind::SECTION {
                continue;
            }
            let Some(want) = own(SectionId(r.section)) else { continue };
            let Some(&index) = defined.get(&(p as u32, want.0)) else {
                return Err(refused(
                    format!(
                        "def-entry: key {} resolves to file {} section {} and no entry of it \
                         defines it there",
                        String::from_utf8_lossy(&c.keys[p].name),
                        c.file.0,
                        r.section
                    ),
                    live(),
                ));
            };
            if let Some(d) = res.def.get_mut(SymbolId(p as u32)) {
                *d = Def::Defined(SymEntry { file: FileId(0), index });
            }
        }
    }

    // Assignment: each section's output section (the stored `OUTSECS`
    // row's name, renumbered within the file) and `keep` by rule.
    let rows = view.outsecs().map_err(|e| refused("OUTSECS".into(), e))?;
    let mut names: Vec<(OutSecId, Vec<u8>)> = Vec::new();
    for f in facts {
        if let Some(o) = f.out
            && !names.iter().any(|(x, _)| *x == o)
        {
            let row = rows.get(o.0 as usize).ok_or_else(|| {
                refused("OUTSECS".into(), corrupt("an output section past OUTSECS"))
            })?;
            names.push((o, view.string(row.name).map_err(|e| refused("OUTSECS".into(), e))?));
        }
    }
    let mut a = Assignment::empty(count);
    a.outsecs = Column::from_vec(
        names
            .iter()
            .map(|(_, n)| OutSec {
                name: n,
                sh_type: 0,
                flags: 0,
                merge: None,
                keep: false,
                fill: None,
                constraint: None,
                script_position: None,
            })
            .collect(),
    )
    .map_err(|_| refused("OUTSECS".into(), live()))?;
    for (k, f) in facts.iter().enumerate() {
        let Some(o) = f.out else { continue };
        let Some(n) = names.iter().position(|(x, _)| *x == o) else { continue };
        let id = SectionId(k as u32);
        if let Some(x) = a.out_of.get_mut(id) {
            *x = OutOf::Out(OutSecId(n as u32));
        }
        let s = ff.sections.get(k);
        if f.kept
            && s.is_some_and(|s| keep_by_rule(s.sh_type, s.flags, &names[n].1))
            && let Some(x) = a.keep.get_mut(id)
        {
            *x = true;
        }
    }

    // Split: the merge phase's split of the file (its split merge
    // sections' pieces; its `.eh_frame` records), renumbered.
    let null =
        SectionFact { kind: SectionKind::Null, kept: false, out: None, reloc: SectionIndex(0) };
    let by_index = |i: SectionIndex| facts.get(i.0 as usize).copied().unwrap_or(null);
    let relocs = ArchRelocs(arch);
    let fs = split_of(ff, view, &by_index, &relocs, pool, Some(delta))
        .map_err(|e| refused("split_of".into(), e))?;
    let mut merge_of: Vec<SplitRow> = Vec::new();
    let mut piece_off: Vec<u32> = Vec::with_capacity(fs.pieces.len());
    for p in &fs.pieces {
        let s = own(p.section).ok_or_else(|| refused("split".into(), corrupt("a piece")))?;
        let at = u32::try_from(piece_off.len()).map_err(|_| refused("split".into(), live()))?;
        match merge_of.last_mut() {
            Some(r) if r.section == s => r.pieces.len += 1,
            _ => merge_of.push(SplitRow { section: s, pieces: Span { start: at, len: 1 } }),
        }
        piece_off.push(u32::try_from(p.offset).map_err(|_| refused("split".into(), live()))?);
    }
    let (eh_rows, eh_off, mut eh_piece) = fs.eh.split_rows();
    let mut eh_of = Vec::with_capacity(eh_rows.len());
    for r in eh_rows.iter() {
        let s = own(r.section).ok_or_else(|| refused("split".into(), corrupt("an eh row")))?;
        eh_of.push(SplitRow { section: s, ..*r });
    }
    for p in eh_piece.iter_mut() {
        // An FDE whose code is another file's section (named through a
        // non-local symbol: S5's `REFUSE` row of this file): no edge of
        // this file's sources, as in the whole link.
        p.code = p.code.get().and_then(own).map_or(OptId::NONE, OptId::some);
    }
    let split = Split {
        piece_frag: (0..piece_off.len() as u32).map(FragId).collect(),
        merge_of: merge_of.into(),
        piece_off: piece_off.into(),
        frags: Column::new(),
        eh_of: eh_of.into(),
        eh_off,
        eh_piece,
    };

    let t = LiveInputs {
        inputs: &inputs,
        resolution: &res,
        assignment: &a,
        split: &split,
        roots: &[],
        arch: Some(arch),
    };
    let options = LiveOptions { gc_sections: true, keep_exported: plan.shaping.gc_keep_exported };
    let rows =
        mortar_live::stored_edges(&t, options, Some(&[FileId(0)]), pool).map_err(|(_, d)| {
            refused(
                format!(
                    "stored_edges: {}",
                    d.render(&mortar_diag::Style::plain("mortar")).trim_end()
                ),
                live(),
            )
        })?;
    rows.iter()
        .map(|e| edge_row(e, first, count, nkeys))
        .collect::<Result<_, _>>()
        .map_err(|e| refused("EDGES".into(), e))
}

/// One narrow row in the update's ids (`changed::translate`'s rule):
/// sections `first + k`, keys their position, a zero target 0, a code
/// copied.
fn edge_row(e: &LiveEdge, first: u32, count: u32, nkeys: u32) -> Result<EdgeRow, IncrReason> {
    use edge_kind::Target;
    let section =
        |s: SectionId| (s.0 < count).then(|| first + s.0).ok_or_else(|| corrupt("a row's section"));
    let target = match (edge_kind::target(e.kind), e.target) {
        (Some(Target::Key), LiveTarget::Key(k)) if k.0 < nkeys => k.0,
        (Some(Target::Section), LiveTarget::Section(t)) => section(t)?,
        (Some(Target::Zero), LiveTarget::None) => 0,
        (Some(Target::Code), LiveTarget::Code(c)) => c,
        _ => return Err(corrupt(format!("EDGES: a row of kind {} with the wrong target", e.kind))),
    };
    Ok(EdgeRow { kind: e.kind, source: section(e.source)?, target, offset: e.offset })
}

// ---- the shadow (findings only): P-1b's rows against front 1's ----------

/// `changed_edges_equal_the_fronts` on the real path (WP-267 P-1b step 4,
/// called by P-1's shadow once P-1's rows equal front 1's): `mine` (P-1's
/// rows after `facts_after_resolve`) gets [`changed_edges_of`]'s `index`
/// and `edges`, compared with front 1's `oracle` (`changed::changed_files`)
/// file by file: `INDEX` row by row in its order (one body makes both),
/// `EDGES` as sorted lists (`stored`'s order is by the link's key ids,
/// which the narrow link numbers otherwise; key targets are positions in
/// the SAME `keys`, which P-1's shadow compared equal). The line:
/// `P-1b: equal (N files, R rows)`, `finding: P-1b <table> ..` with the
/// row and both values, or `P-1b: refused by name: ..`.
#[allow(clippy::too_many_arguments, reason = "the shadow's inputs, as P-1's")]
pub(super) fn shadow_line(
    view: &StateView<'_>,
    cache: &FrontCache<'_>,
    mine: &mut ChangedFiles,
    oracle: &ChangedFiles,
    delta: &ResolveDelta,
    plan: &Plan,
    arch: &dyn mortar_arch::Arch,
    pool: &Pool,
) -> String {
    let at = std::time::Instant::now();
    if let Err((what, e)) = changed_edges_of(view, cache, mine, delta, plan, arch, pool) {
        return format!("P-1b: refused by name: {what}: {}", e.report());
    }
    let ms = at.elapsed().as_secs_f64() * 1e3;
    if mine.files.len() != oracle.files.len() {
        return format!(
            "finding: P-1b files: mine {}, front 1's {}",
            mine.files.len(),
            oracle.files.len()
        );
    }
    let mut rows = 0usize;
    for (m, o) in mine.files.iter().zip(&oracle.files) {
        let id = m.file.0;
        rows += m.index.len().max(o.index.len());
        if m.index != o.index {
            let k = m
                .index
                .iter()
                .zip(&o.index)
                .position(|(a, b)| a != b)
                .unwrap_or_else(|| m.index.len().min(o.index.len()));
            let differ = m.index.iter().zip(&o.index).filter(|(a, b)| a != b).count()
                + m.index.len().abs_diff(o.index.len());
            return format!(
                "finding: P-1b INDEX file {id} row {k}: mine {:?}, front 1's {:?} ({differ} \
                 differ; {} rows mine, {} front 1's)",
                m.index.get(k),
                o.index.get(k),
                m.index.len(),
                o.index.len()
            );
        }
        let key = |e: &EdgeRow| (e.source, e.kind, e.target, e.offset);
        let mut a: Vec<_> = m.edges.iter().map(key).collect();
        let mut b: Vec<_> = o.edges.iter().map(key).collect();
        a.sort_unstable();
        b.sort_unstable();
        rows += a.len().max(b.len());
        if a != b {
            let only_mine: Vec<_> = a.iter().filter(|x| b.binary_search(x).is_err()).collect();
            let only_front: Vec<_> = b.iter().filter(|x| a.binary_search(x).is_err()).collect();
            let name = |e: Option<&&(u32, u8, u32, u32)>| match e {
                Some(&&(source, kind, target, offset)) => {
                    let t = match edge_kind::target(kind) {
                        Some(edge_kind::Target::Key) => m.keys.get(target as usize).map_or_else(
                            || format!("key {target}"),
                            |k| format!("key {target} {}", String::from_utf8_lossy(&k.name)),
                        ),
                        _ => format!("{target}"),
                    };
                    format!("(source {source} kind {kind} target {t} offset {offset:#x})")
                }
                None => "none".to_owned(),
            };
            return format!(
                "finding: P-1b EDGES file {id}: only mine {} first {}, only front 1's {} first \
                 {} ({} rows mine, {} front 1's)",
                only_mine.len(),
                name(only_mine.first()),
                only_front.len(),
                name(only_front.first()),
                a.len(),
                b.len()
            );
        }
    }
    format!(
        "P-1b: equal ({} files, {rows} rows; gc {}) ms changed_edges_of={ms:.3}",
        mine.files.len(),
        u8::from(plan.shaping.gc_sections)
    )
}
