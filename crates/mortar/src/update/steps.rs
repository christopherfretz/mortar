//! WP-267 P-3 (`spec/landed/incr/WP-267.md` §4, P-3 row; rulings "After
//! the adversarial review" R-A1, R-A2): the parts of the update's U4-U7
//! and of the identity decision that need no `Front`.
//!
//! - [`link_arch_of`]: the link's architecture, target and layout options
//!   from the plan and the machine alone (record §1 row 9): the front's own
//!   selection (`front.rs` calls it), so the update can make it from
//!   `META.machine`.
//! - [`option_names`]: the link's option keys without the front's
//!   directives (row 8, M-6): a link that may have a linker script is
//!   refused by name.
//! - [`identity_prime`]: identity' (M-2, §2.3), from the update's own rows.
//! - [`identity_check`]: R-A1, the identity check over a front's capture
//!   (today's comparison), BINDING.
//! - [`resolved_within_affected`]: R-A2, the count check, BINDING.

use mortar_core::incr::{IncrReason, LayoutDelta, PlaceNow, SectionChange, StoredLayout};
use mortar_core::incr::{OptionKeys, Unimplemented};
use mortar_core::plan::{Plan, ScriptRole};
use mortar_core::{FileId, SymbolId};
use mortar_elf::Machine;
use mortar_incr::format::section_bits::COLLECTED;
use mortar_incr::format::{NONE, Str, index_kind, kind, stat_tag};
use mortar_incr::update::resolve::ResolveDelta;
use mortar_incr::update::rows::sym_kind;
use mortar_incr::update::rows::{FileTable, edge_kind};
use mortar_incr::update::testing::MemoryState;
use mortar_incr::update::view::{Change, ChangedFiles, StateView};

/// Why [`link_arch_of`] gives no architecture: the front's three refusals,
/// each reported by the front as it reports them today.
#[derive(Debug)]
pub(crate) enum ArchFail {
    /// `arch_status`'s diagnostic (a dynamic output not joined there).
    Status(mortar_diag::Diagnostic),
    /// `Options::from_plan`'s.
    Options(mortar_layout::LayoutDiag),
    /// No `LinkArch` for the machine.
    NoArch,
}

/// The link's architecture status, layout target, layout options and
/// `LinkArch` from `plan` and `machine` alone (record §1 row 9; the
/// front's selection, `front.rs`, is this function). `slack`: an
/// incremental session's link (`LoadEnv::slack`).
///
/// # Errors
/// [`ArchFail`].
pub(crate) fn link_arch_of(
    plan: &Plan,
    machine: Machine,
    slack: bool,
) -> Result<
    (mortar_arch::Status, mortar_layout::Target, mortar_layout::Options, crate::arch::LinkArch),
    ArchFail,
> {
    let status = crate::dynamic::arch_status(plan, machine).map_err(ArchFail::Status)?;
    let target = mortar_layout::Target::of(machine);
    let mut options =
        mortar_layout::Options::from_plan(plan, &target).map_err(ArchFail::Options)?;
    if slack {
        options.slack = mortar_layout::slack::Slack::INCREMENTAL;
    }
    let arch =
        crate::arch::LinkArch::of(machine, options.fix_843419, status).ok_or(ArchFail::NoArch)?;
    Ok((status, target, options, arch))
}

/// `META.machine` (an `e_machine`) as the front's [`Machine`].
#[must_use]
pub(crate) fn machine_of(e_machine: u16) -> Option<Machine> {
    [Machine::AArch64, Machine::X86_64, Machine::I386]
        .into_iter()
        .find(|m| m.e_machine() == e_machine)
}

fn script_refusal(what: &str) -> IncrReason {
    super::changed::record(&format!("M-6: option names: {what}: (resolve)"));
    IncrReason::UpdateNotImplemented(Some(Unimplemented::Resolve))
}

/// The link's option keys (`option_keys`) without the front (record §1
/// row 8): `-e`, `-u`, `--defsym`, ... from `plan`, with NO script
/// directives (`ENTRY`, `EXTERN`). M-6's conservative path: a link that
/// may carry directives is refused `update-not-implemented (resolve)` by
/// name: one with a `-T`/`--default-script` script, or one with an input
/// path of `STAT` that is no `FILES` row's path (an input that gave no
/// file: a linker script given as an input; also an archive of no members,
/// refused with it). O(inputs) over `STAT` and `FILES`, which U2 read.
///
/// # Errors
/// As above; the state's own.
pub(crate) fn option_names(view: &StateView<'_>, plan: &Plan) -> Result<OptionKeys, IncrReason> {
    if plan.scripts.iter().any(|s| matches!(s.role, ScriptRole::Script | ScriptRole::DefaultScript))
    {
        return Err(script_refusal("a linker script on the line"));
    }
    let files = u32::try_from(view.count(kind::FILES)).unwrap_or(u32::MAX);
    let mut paths: Vec<Vec<u8>> = Vec::with_capacity(files as usize);
    for f in 0..files {
        paths.push(view.string(view.file(FileId(f))?.path)?);
    }
    paths.sort_unstable();
    paths.dedup();
    let rows = u32::try_from(view.count(kind::STAT)).unwrap_or(u32::MAX);
    let mut missing: Vec<String> = Vec::new();
    for i in 0..rows {
        let r = view.stat(i)?;
        let path = view.string(r.path)?;
        if r.tag == stat_tag::INPUT && paths.binary_search(&path).is_err() {
            missing.push(String::from_utf8_lossy(&path).into_owned());
        }
    }
    if !missing.is_empty() {
        // P-3b (M-6's detection, findings): every such input by path, and
        // the FILES paths it was looked up in.
        let some: Vec<String> =
            paths.iter().take(4).map(|p| String::from_utf8_lossy(p).into_owned()).collect();
        return Err(script_refusal(&format!(
            "an input that gave no file (a script input?) {missing:?}; FILES paths {} e.g. \
             {some:?}",
            paths.len()
        )));
    }
    Ok(mortar_resolve::tables::option_keys(plan, &[]))
}

/// A changed file's section id `s` (of `cf`'s id space) as the state's id
/// of the same `(file, index)`; an id below `SECTIONS` that is no changed
/// file's is itself. `NONE` stays `NONE`.
fn stored_section(view: &StateView<'_>, cf: &ChangedFiles, s: u32) -> Result<u32, String> {
    if s == NONE {
        return Ok(NONE);
    }
    for c in &cf.files {
        let n = u32::try_from(c.sections.len()).unwrap_or(u32::MAX);
        if s >= c.first_section.0 && s - c.first_section.0 < n {
            let r = view.file_index(c.file).map_err(|e| e.report())?;
            return Ok(r.ranges[FileTable::Sections as usize].first + (s - c.first_section.0));
        }
    }
    if u64::from(s) < view.count(kind::SECTIONS) {
        Ok(s)
    } else {
        Err(format!("section {s}: no row of the state or of a changed file"))
    }
}

/// **identity'** (M-2, record §2.3): whether the update's own rows say the
/// state after the update equals the stored one outside `STAT`, `LEAVES`
/// and the changed files' `FILES` rows (the predicate of the identity
/// check, [`identity_check`]), judged from the rows an update can change,
/// in O(change). `Ok`: identity; `Err`: the first row that says not
/// (anything identity' cannot judge is "not identity").
///
/// The rows, by cause (IN-78's list):
/// - the changed files' ranges (`ChangedFiles` against the stored rows,
///   row by row): only `Changed` files, none appeared; `SECTIONS` (every
///   column but the place and `COLLECTED`, which the layout half and the
///   flips judge; names by bytes), `FILE_INDEX.first_global`, `MENTIONS`
///   (keys through `ResolveDelta::key_ids`, sections by `(file, index)`),
///   `INDEX`, `EDGES` (targets by their class; both as sets, as the
///   capture writes them), `FRAG_USE` (the file's
///   pieces' fragments as a set), no `@` name;
/// - the affected keys: no new, no gone key, no first mention moved, the
///   `KEY_REFS`/`GROUPS`/`COMMONS` edits putting back what they delete, no
///   keeper re-elected, each `resolved` row (with `facts`) the stored one
///   (its section by `(file, index)`);
/// - the layout delta: nothing moved, killed, revived, no hole made, every
///   touched chunk (`hole_chunks`) with its stored holes, no region
///   opened, no growth step, no new `FRAGS`, no slot appended, dead or
///   revived, every changed section at its stored place with its stored
///   size, no liveness flip (`flips`), the `.eh_frame` ranges at their
///   stored offset, capacity and record count. The `.symtab` blocks'
///   counts by their causes: the same locals (`first_global`) and the same
///   mentions.
///
/// **`FRAGS` rows** (review §3): "no new `FRAGS`" does not cover a stored
/// `FRAGS` row that changes without a new row appearing (its `refs`). Its
/// cause is a changed file whose pieces name other fragments, which the
/// `FRAG_USE` set above rejects. That is enough for the bytes only
/// because the identity case refills every merged chunk whole and the
/// front replays `layout.merge`; a narrower refill must judge `FRAGS.refs`
/// itself.
///
/// Not judged here (the binding [`identity_check`] does): the merged
/// visibility and `referenced` of an affected key (`SYMTAB_KEYS`), the
/// `DIAGS` rows, `META` fields other than the binding's.
///
/// # Errors
/// `Err(why)` is "not identity", never a failure.
#[allow(clippy::too_many_lines, reason = "IN-78's rows, cause by cause")]
pub(crate) fn identity_prime(
    view: &StateView<'_>,
    stored: &StoredLayout<'_>,
    cf: &ChangedFiles,
    delta: &ResolveDelta,
    d: &LayoutDelta,
    changes: &[SectionChange<'_>],
    pieces: &[mortar_core::incr::FilePieces],
    demands: &[mortar_core::incr::FileDemands],
    flips: usize,
) -> Result<(), String> {
    let e = |x: IncrReason| x.report();
    if !cf.appeared.is_empty() {
        return Err(format!("{} files appeared", cf.appeared.len()));
    }
    for c in &cf.files {
        let file = c.file;
        if c.change != Change::Changed {
            return Err(format!("file {}: {:?}", file.0, c.change));
        }
        if c.versioned != 0 {
            return Err(format!("file {}: versioned names", file.0));
        }
        let fi = view.file_index(file).map_err(e)?;
        if fi.first_global != c.first_global {
            return Err(format!(
                "file {}: first_global {} (stored {})",
                file.0, c.first_global, fi.first_global
            ));
        }
        let keys = delta
            .key_ids
            .iter()
            .find(|(f, _)| *f == file)
            .map(|(_, k)| k.as_slice())
            .ok_or_else(|| format!("file {}: no key ids", file.0))?;
        let key = |p: u32| keys.get(p as usize).map(|k| k.0).ok_or_else(|| format!("key {p}"));
        let sec = |s: u32| stored_section(view, cf, s);
        // SECTIONS.
        let old = view.sections_of(file).map_err(e)?;
        if old.len() != c.sections.len() {
            return Err(format!(
                "file {}: {} sections (stored {})",
                file.0,
                c.sections.len(),
                old.len()
            ));
        }
        for (k, (n, o)) in c.sections.iter().zip(&old).enumerate() {
            let same = (n.file, n.anchor_kind, n.kind, n.bits & !COLLECTED, n.out, n.ordinal)
                == (o.file, o.anchor_kind, o.kind, o.bits & !COLLECTED, o.out, o.ordinal)
                && (n.sh_type, n.sh_flags, n.size, n.align, n.offset)
                    == (o.sh_type, o.sh_flags, o.size, o.align, o.offset)
                && cf.string(view, n.anchor).map_err(e)? == view.string(o.anchor).map_err(e)?
                && cf.string(view, n.name).map_err(e)? == view.string(o.name).map_err(e)?;
            if !same {
                return Err(format!("file {}: SECTIONS row {k}: {n:?} (stored {o:?})", file.0));
            }
        }
        // MENTIONS.
        let old = view.mentions_of(file).map_err(e)?;
        if old.len() != c.mentions.len() {
            return Err(format!(
                "file {}: {} mentions (stored {})",
                file.0,
                c.mentions.len(),
                old.len()
            ));
        }
        for (k, (n, o)) in c.mentions.iter().zip(&old).enumerate() {
            let mut m = *n;
            m.symbol = key(m.symbol)?;
            m.section = sec(m.section)?;
            if m != *o {
                return Err(format!("file {}: MENTIONS row {k}: {m:?} (stored {o:?})", file.0));
            }
        }
        // INDEX and EDGES, as sets in the state's ids: the capture writes
        // each file's rows sorted (in its ids) and without repeats
        // (`tables.rs`'s `index.sort_unstable(); dedup()`, `capture.rs`'s
        // `edges.sort_unstable_by_key; dedup()`), the producer gives them in
        // its walk's order with repeats (P-3b: the lane's "24 INDEX rows
        // (stored 15)" and EDGES row 7's kind 11 against 9 were order and
        // repeats only).
        let mut now = Vec::with_capacity(c.index.len());
        for n in &c.index {
            let mut m = *n;
            m.unit = sec(m.unit)?;
            m.target = match m.kind {
                index_kind::GLOBAL | index_kind::SLOT => key(m.target)?,
                _ => sec(m.target)?,
            };
            now.push(m);
        }
        let mut was = view.index_of(file).map_err(e)?;
        if let Some(k) = differ_as_sets(&mut now, &mut was) {
            return Err(format!("file {}: INDEX (as a set) {k}", file.0));
        }
        let mut now = Vec::with_capacity(c.edges.len());
        for n in &c.edges {
            let mut m = *n;
            m.source = sec(m.source)?;
            m.target = match edge_kind::target(m.kind) {
                Some(edge_kind::Target::Key) => key(m.target)?,
                Some(edge_kind::Target::Section) => sec(m.target)?,
                Some(edge_kind::Target::Zero | edge_kind::Target::Code) => m.target,
                None => return Err(format!("file {}: EDGES kind {}", file.0, m.kind)),
            };
            now.push(m);
        }
        let mut was = view.edges_of(file).map_err(e)?;
        if let Some(k) = differ_as_sets(&mut now, &mut was) {
            return Err(format!("file {}: EDGES (as a set) {k}", file.0));
        }
        // FRAG_USE, as a set.
        let mut now: Vec<(u32, u32, [u8; 16])> = pieces
            .iter()
            .filter(|p| p.file == file)
            .flat_map(|p| &p.pieces)
            .map(|p| (p.out.0, p.class.word(), p.hash))
            .collect();
        now.sort_unstable();
        now.dedup();
        let mut was: Vec<(u32, u32, [u8; 16])> =
            view.frag_uses_of(file).map_err(e)?.iter().map(|u| (u.out, u.class, u.hash)).collect();
        was.sort_unstable();
        was.dedup();
        if now != was {
            return Err(format!(
                "file {}: FRAG_USE {} fragments (stored {})",
                file.0,
                now.len(),
                was.len()
            ));
        }
    }
    // The affected keys.
    if !delta.new_keys.is_empty() || !delta.gone.is_empty() || !delta.at.is_empty() {
        return Err(format!(
            "{} new keys, {} gone, {} first mentions moved",
            delta.new_keys.len(),
            delta.gone.len(),
            delta.at.len()
        ));
    }
    key_edits_put_back(view, delta, demands)?;
    for (id, r) in &delta.resolved {
        let o = view.resolved(*id).map_err(e)?;
        let mut m = *r;
        m.section = stored_section(view, cf, m.section)?;
        if m != o {
            return Err(format!("key {}: RESOLVED {m:?} (stored {o:?})", id.0));
        }
    }
    // The layout delta.
    if flips != 0 {
        return Err(format!("{flips} sections flip liveness"));
    }
    // `hole_chunks` lists every chunk the delta touched (a changed
    // section's, a merged or slot chunk's), with no hole left or the same
    // holes (P-3b: the lane's 78 identity hops had 1 to 5): identity when
    // each one's holes are the stored ones.
    touched_chunks_keep_their_holes(stored, d)?;
    // A killed section (a changed file's stored section no change matched
    // that had a place): not identity, named with its stored row.
    if let Some(&k) = d.killed.first() {
        let r = view.section(k).map_err(e)?;
        return Err(format!(
            "{} killed, first section {} (file {}, {:?}, sh_type {}, size {}, place {} {}, \
             bits {:#x})",
            d.killed.len(),
            k.0,
            r.file,
            String::from_utf8_lossy(&view.string(r.name).map_err(e)?),
            r.sh_type,
            r.size,
            r.place_chunk,
            r.place_offset,
            r.bits
        ));
    }
    let counts = [
        d.moved.len(),
        d.killed.len(),
        d.revived.len(),
        d.holes.len(),
        d.regions_opened.len(),
        d.growth_steps.len(),
        d.frags.len(),
        d.slots_appended.len(),
        d.slots_dead.len(),
        d.slots_revived.len(),
    ];
    if counts.iter().any(|&n| n != 0) {
        return Err(format!(
            "the layout delta: moved, killed, revived, holes, regions, growth steps, new FRAGS, \
             slots appended, dead, revived: {counts:?}"
        ));
    }
    for c in changes {
        let Some(o) = c.old else {
            return Err(format!("section {} of file {}: no stored section", c.index, c.file.0));
        };
        let was = view.section(o).map_err(e)?;
        let now = match d.place_of(stored, c.id).map_err(e)? {
            PlaceNow::Here(p) => p.get(),
            PlaceNow::NotPlaced => None,
            PlaceNow::Stored(_) => return Err(format!("section {}: Stored", c.id.0)),
        };
        let then = (was.place_chunk != NONE).then_some((was.place_chunk, was.place_offset));
        if now.map(|(ch, off)| (ch.0, off)) != then || c.size != was.size {
            return Err(format!(
                "section {} of file {}: at {now:?} size {} (stored {then:?} size {})",
                c.index, c.file.0, c.size, was.size
            ));
        }
    }
    if let Some(eh) = &d.eh {
        if eh.whole {
            return Err(".eh_frame rewritten whole".into());
        }
        for r in &eh.ranges {
            let s = stored.eh_range_of(r.file).map_err(e)?;
            let same = s.is_some_and(|s| {
                (u64::from(s.offset), u64::from(s.capacity), u64::from(s.records))
                    == (r.offset, r.capacity, r.records.len() as u64)
            });
            if !same {
                return Err(format!(".eh_frame range of file {}: {r:?} (stored {s:?})", r.file.0));
            }
        }
    }
    Ok(())
}

/// Each chunk of `d.hole_chunks` (every chunk the delta touched: a
/// changed section's, a merged or a slot chunk's, listed with no hole left
/// or with the same holes) has the stored holes (P-3b: the lane's 78
/// identity hops had 1 to 5 such chunks, each with its holes unchanged).
fn touched_chunks_keep_their_holes(
    stored: &StoredLayout<'_>,
    d: &LayoutDelta,
) -> Result<(), String> {
    for &c in &d.hole_chunks {
        let mut now: Vec<_> = d.placement.holes.iter().filter(|h| h.chunk == c).copied().collect();
        now.sort_unstable();
        let mut was = stored.holes_in(c).map_err(|x| x.report())?;
        was.sort_unstable();
        if now != was {
            return Err(format!("chunk {}: holes {now:?} (stored {was:?})", c.0));
        }
    }
    Ok(())
}

/// **R-A1, the identity check (BINDING)**: the canonical form of a front's
/// capture `capture` against the stored state read whole from `lock`
/// (IN-78's `canon`), and whether the differences are those of the
/// identity case only (`STAT`, `LEAVES`, the changed files' `FILES` rows;
/// [`super::identity`]). Today's comparison, over any front: front 1's in
/// this package, front 2's (with the stored placement replayed) once P-5
/// removes front 1. Returns the answer, the differences and the stored
/// state decoded (R-A2 reads it: one whole read for both).
///
/// # Errors
/// `state-corrupt` for a state that does not read or decode.
pub(crate) fn identity_check(
    lock: &std::fs::File,
    capture: &MemoryState,
    files: &[FileId],
    stored: &StoredLayout<'_>,
) -> Result<(bool, Vec<mortar_incr::state5::canon::CanonDiff>, MemoryState), IncrReason> {
    let corrupt = |e: String| IncrReason::StateCorrupt(e);
    let len = usize::try_from(lock.metadata().map_err(|e| corrupt(e.to_string()))?.len())
        .map_err(|_| corrupt("the state's length".into()))?;
    let mut bytes = vec![0u8; len];
    std::os::unix::fs::FileExt::read_exact_at(lock, &mut bytes, 0)
        .map_err(|e| corrupt(e.to_string()))?;
    let old = mortar_incr::state5::read::decode(&bytes).map_err(|e| corrupt(e.to_string()))?;
    drop(bytes);
    let a = mortar_incr::state5::canon::canon_of(&old).map_err(|e| corrupt(e.to_string()))?;
    let b = mortar_incr::state5::canon::canon_of(capture).map_err(|e| corrupt(e.to_string()))?;
    let diffs = mortar_incr::state5::canon::compare(&b, &a, stored);
    Ok((super::identity(&diffs, files), diffs, old))
}

/// Sorts and dedups both; `None` when equal, else the first row that
/// differs (or the counts).
fn differ_as_sets<R: Ord + Copy + std::fmt::Debug>(
    now: &mut Vec<R>,
    was: &mut Vec<R>,
) -> Option<String> {
    now.sort_unstable();
    now.dedup();
    was.sort_unstable();
    was.dedup();
    if now == was {
        return None;
    }
    let k = now.iter().zip(was.iter()).position(|(a, b)| a != b);
    Some(match k {
        Some(k) => format!("row {k}: {:?} (stored {:?})", now[k], was[k]),
        None => format!("{} rows (stored {})", now.len(), was.len()),
    })
}

/// Whether the `KEY_REFS`/`GROUPS`/`COMMONS` edits of `delta` leave the
/// stored rows as they are (P-3b: the edits are not symmetric, so "put
/// equals delete" was wrong). `resolve`'s edits:
/// - `KEY_REFS`: `put` is every key a changed file mentions (its whole
///   rows, `demand` 0: the join ORs the file's `FileDemands` into the row
///   when written), `delete` the stored keys it no longer mentions (by
///   `(key, file)`). Identity: no delete, and each put row is the stored
///   live row of its `(key, file)` with `demand` the OR of `demands`' flags
///   of the file and key.
/// - `GROUPS`: `put` a holder no stored row has, `delete` a stored row no
///   holder has (disjoint by file). Identity: neither.
/// - `COMMONS`: `delete` a stored row whose file has no common any more,
///   `put` every edited file's row. Identity: no delete, each put row the
///   stored live row of its `(key, file)`.
/// - no keeper re-elected.
fn key_edits_put_back(
    view: &StateView<'_>,
    delta: &ResolveDelta,
    demands: &[mortar_core::incr::FileDemands],
) -> Result<(), String> {
    let e = |x: IncrReason| x.report();
    if !delta.key_refs.delete.is_empty()
        || !delta.groups.put.is_empty()
        || !delta.groups.delete.is_empty()
        || !delta.commons.delete.is_empty()
        || !delta.reelected.is_empty()
    {
        return Err(format!(
            "KEY_REFS deletes {}, GROUPS puts {} deletes {}, COMMONS deletes {}, keepers \
             re-elected {}",
            delta.key_refs.delete.len(),
            delta.groups.put.len(),
            delta.groups.delete.len(),
            delta.commons.delete.len(),
            delta.reelected.len()
        ));
    }
    for p in &delta.key_refs.put {
        let want = mortar_core::id::SymKey::global(SymbolId(p.key));
        let demand = demands
            .iter()
            .filter(|d| d.file.0 == p.file)
            .flat_map(|d| &d.demands)
            .filter(|d| d.key == want)
            .fold(0u16, |a, d| a | d.flags.bits());
        let row = mortar_incr::update::rows::KeyRefRow { demand, ..*p };
        let stored: Vec<_> = view
            .key_refs(SymbolId(p.key))
            .map_err(e)?
            .into_iter()
            .filter(|r| !r.tomb && r.file == p.file)
            .collect();
        if stored != [row] {
            return Err(format!("KEY_REFS put {row:?} (stored {stored:?})"));
        }
    }
    for p in &delta.commons.put {
        let stored: Vec<_> = view
            .commons(SymbolId(p.key))
            .map_err(e)?
            .into_iter()
            .filter(|r| !r.tomb && r.file == p.file)
            .collect();
        if stored != [*p] {
            return Err(format!("COMMONS put {p:?} (stored {stored:?})"));
        }
    }
    Ok(())
}

fn text(m: &MemoryState, s: Str) -> Option<&[u8]> {
    m.strings.get(s.off as usize..s.off as usize + s.len as usize)
}

/// A key's `RESOLVED` RESOLUTION (rulings "After P-3's hand-back" item 2:
/// kind, typ, bind, other, value, size; NOT `facts`) with its definer's
/// section by identity `(file, anchor kind, anchor, name, ordinal)`.
type Answer<'m> = ((u8, u8, u8, u8, u64, u64), Option<(u32, u8, &'m [u8], &'m [u8], u32)>);

fn answer(m: &MemoryState, k: usize) -> Option<Answer<'_>> {
    let r = m.resolved.get(k)?;
    let at = if r.section == NONE {
        None
    } else {
        let s = m.sections.get(r.section as usize)?;
        Some((s.file, s.anchor_kind, text(m, s.anchor)?, text(m, s.name)?, s.ordinal))
    };
    Some(((r.kind, r.typ, r.bind, r.other, r.value, r.size), at))
}

/// The global keys of `m` by (name, version): their `SYMBOLS` rows.
fn globals(m: &MemoryState) -> std::collections::HashMap<(Vec<u8>, Vec<u8>), usize> {
    let mut by = std::collections::HashMap::new();
    for (k, s) in m.symbols.iter().enumerate() {
        if s.kind != sym_kind::LOCAL
            && let (Some(n), Some(v)) = (text(m, s.name), text(m, s.version))
        {
            by.entry((n.to_vec(), v.to_vec())).or_insert(k);
        }
    }
    by
}

/// **R-A2, the count check (BINDING)**: every global key whose `RESOLVED`
/// resolution in a front's capture `capture` differs from the stored
/// state's `old` (by name and version; kind, typ, bind, other, value, size
/// and its definer's section by identity: NOT `facts`, which
/// [`facts_within_changed`] judges), and every key the capture has that
/// `old` does not, is a key the update chose: in `delta.affected` (or,
/// new, in `delta.new_keys`). It is computed on the front, so it is independent of
/// the delta (identity' and U5 take their keys from `affected`). Locals
/// are a file's rows, not keys re-resolved. O(state), in memory, over the
/// decode [`identity_check`] made.
///
/// # Errors
/// `Err(what)`: the keys outside `affected`, named (the caller records a
/// FINDING and falls back by name).
pub(crate) fn resolved_within_affected(
    old: &MemoryState,
    capture: &MemoryState,
    delta: &ResolveDelta,
) -> Result<usize, String> {
    let was = globals(old);
    let mut outside: Vec<String> = Vec::new();
    let mut differ = 0usize;
    for (name, &k) in &globals(capture) {
        let now = answer(capture, k);
        let (then, id) = match was.get(name) {
            Some(&o) => (answer(old, o), Some(o)),
            None => (None, None),
        };
        if now.is_some() && now == then {
            continue;
        }
        differ += 1;
        let chosen = match id {
            Some(o) => {
                u32::try_from(o).is_ok_and(|o| delta.affected.binary_search(&SymbolId(o)).is_ok())
            }
            None => delta.new_keys.iter().any(|n| (&n.name, &n.version) == (&name.0, &name.1)),
        };
        if !chosen && outside.len() < 8 {
            outside.push(format!(
                "{} ({}): stored {then:?}, the front's {now:?}",
                String::from_utf8_lossy(&name.0),
                id.map_or_else(|| "new".to_string(), |o| o.to_string())
            ));
        }
    }
    let now = globals(capture);
    for (name, &o) in &was {
        if now.contains_key(name) || old.symbols.get(o).is_some_and(|s| s.kind == sym_kind::GONE) {
            continue;
        }
        differ += 1;
        let chosen =
            u32::try_from(o).is_ok_and(|o| delta.affected.binary_search(&SymbolId(o)).is_ok());
        if !chosen && outside.len() < 8 {
            outside.push(format!(
                "{} ({o}): stored, gone from the front",
                String::from_utf8_lossy(&name.0)
            ));
        }
    }
    if outside.is_empty() {
        Ok(differ)
    } else {
        Err(format!("keys re-resolved outside ResolveDelta::affected: {}", outside.join("; ")))
    }
}

/// The keys a changed file of `cf` mentions or defines (its `MENTIONS`:
/// the new rows through `ResolveDelta::key_ids`, and the stored rows of a
/// file the state has), in the update's ids, sorted: the set
/// [`facts_within_changed`] admits. `cf` is the final one (U6's drawn-in
/// files included).
///
/// # Errors
/// The state's own, reported.
pub(crate) fn changed_keys(
    view: &StateView<'_>,
    cf: &ChangedFiles,
    delta: &ResolveDelta,
) -> Result<Vec<SymbolId>, String> {
    let files = view.count(kind::FILES);
    let mut out: Vec<SymbolId> = Vec::new();
    for c in &cf.files {
        let ids = delta.key_ids.iter().find(|(f, _)| *f == c.file).map(|(_, k)| k.as_slice());
        for m in &c.mentions {
            if let Some(&k) = ids.and_then(|k| k.get(m.symbol as usize)) {
                out.push(k);
            }
        }
        if u64::from(c.file.0) < files && c.change != Change::Appeared {
            for m in view.mentions_of(c.file).map_err(|e| e.report())? {
                out.push(SymbolId(m.symbol));
            }
        }
    }
    out.sort_unstable();
    out.dedup();
    Ok(out)
}

/// **The facts check (BINDING; rulings "After P-3's hand-back" item 2)**:
/// every global key of both `old` and `capture` (by name and version)
/// whose `RESOLVED.facts` differ is in `keys` ([`changed_keys`]: a key a
/// changed file mentions or defines). `facts` is no resolution: it is the
/// packed `mortar_arch::SymFacts` (`SymFacts::pack`: bits 0-2 the target
/// kind, 3 `Shlib.func`, 8-11 `sym_type`, 16-23 `st_other`, **24 `dead`**
/// (the definer's section is not live: a dropped COMDAT copy's, or under
/// `--gc-sections` a collected one's), 25 `comdat_only`, 26
/// `gnu_warning`), so a changed referrer flips it while the key's
/// resolution stands. A new or gone key is R-A2's. O(state), in memory.
///
/// # Errors
/// `Err(what)`: the keys outside `keys`, named (a FINDING; the caller falls
/// back by name).
pub(crate) fn facts_within_changed(
    old: &MemoryState,
    capture: &MemoryState,
    keys: &[SymbolId],
) -> Result<usize, String> {
    let was = globals(old);
    let mut outside: Vec<String> = Vec::new();
    let mut differ = 0usize;
    for (name, &k) in &globals(capture) {
        let Some(&o) = was.get(name) else { continue };
        let (Some(now), Some(then)) = (capture.resolved.get(k), old.resolved.get(o)) else {
            continue;
        };
        if now.facts == then.facts {
            continue;
        }
        differ += 1;
        let mentioned = u32::try_from(o).is_ok_and(|o| keys.binary_search(&SymbolId(o)).is_ok());
        if !mentioned && outside.len() < 8 {
            outside.push(format!(
                "{} ({o}): facts stored {:#x}, the front's {:#x}",
                String::from_utf8_lossy(&name.0),
                then.facts,
                now.facts
            ));
        }
    }
    if outside.is_empty() {
        Ok(differ)
    } else {
        Err(format!("facts changed on keys no changed file mentions: {}", outside.join("; ")))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use mortar_incr::update::rows::{ResolvedRow, SymbolRow};

    /// A state of the keys `names` (globals), each defined in no section
    /// with `st_value` `values[k]`.
    fn state(names: &[&str], values: &[u64]) -> MemoryState {
        let mut m = MemoryState::default();
        for (n, &v) in names.iter().zip(values) {
            let off = u32::try_from(m.strings.len()).unwrap();
            m.strings.extend_from_slice(n.as_bytes());
            let name = Str { off, len: u32::try_from(n.len()).unwrap() };
            m.symbols.push(SymbolRow { kind: sym_kind::GLOBAL, name, ..SymbolRow::default() });
            m.resolved.push(ResolvedRow { section: NONE, value: v, ..ResolvedRow::default() });
        }
        m
    }

    fn delta(affected: &[u32]) -> ResolveDelta {
        ResolveDelta {
            affected: affected.iter().map(|&k| SymbolId(k)).collect(),
            ..ResolveDelta::default()
        }
    }

    #[test]
    fn r_a2_holds_when_every_re_resolved_key_is_affected() {
        let old = state(&["a", "b", "c"], &[1, 2, 3]);
        let now = state(&["a", "b", "c"], &[1, 20, 3]);
        assert_eq!(resolved_within_affected(&old, &now, &delta(&[1])), Ok(1));
        assert_eq!(resolved_within_affected(&old, &old, &delta(&[])), Ok(0));
    }

    /// The mutation: the key whose answer changed is dropped from
    /// `affected`; the check names it.
    #[test]
    fn r_a2_catches_a_key_dropped_from_affected() {
        let old = state(&["a", "b", "c"], &[1, 2, 3]);
        let now = state(&["a", "b", "c"], &[1, 20, 3]);
        let e = resolved_within_affected(&old, &now, &delta(&[0, 2])).unwrap_err();
        assert!(e.contains("b (1)"), "{e}");
    }

    #[test]
    fn r_a2_catches_a_new_or_gone_key_the_update_did_not_choose() {
        let old = state(&["a", "b"], &[1, 2]);
        let now = state(&["a", "b", "n"], &[1, 2, 9]);
        assert!(resolved_within_affected(&old, &now, &delta(&[])).unwrap_err().contains("n (new)"));
        let gone = state(&["a"], &[1]);
        assert!(resolved_within_affected(&old, &gone, &delta(&[])).is_err());
        assert_eq!(resolved_within_affected(&old, &gone, &delta(&[1])), Ok(1));
    }

    /// `state` with key `k`'s `RESOLVED.facts` set to `facts`.
    fn with_facts(mut m: MemoryState, k: usize, facts: u32) -> MemoryState {
        m.resolved[k].facts = facts;
        m
    }

    /// Bit 24 of `SymFacts::pack` (`dead`), the lane's flip (512 against
    /// 16777728 on `_Z5twicei`).
    const DEAD: u32 = 1 << 24;

    /// R-A2 compares the resolution only: a facts flip alone is not a key
    /// re-resolved.
    #[test]
    fn r_a2_ignores_a_facts_flip() {
        let old = with_facts(state(&["a", "b"], &[1, 2]), 1, 512);
        let now = with_facts(state(&["a", "b"], &[1, 2]), 1, 512 | DEAD);
        assert_eq!(resolved_within_affected(&old, &now, &delta(&[])), Ok(0));
    }

    /// The facts check: a flip on a key a changed file mentions passes; on
    /// a key no changed file mentions it is caught, by name.
    #[test]
    fn facts_flip_passes_on_a_mentioned_key_and_is_caught_otherwise() {
        let old = with_facts(state(&["a", "b", "c"], &[1, 2, 3]), 1, 512);
        let now = with_facts(state(&["a", "b", "c"], &[1, 2, 3]), 1, 512 | DEAD);
        assert_eq!(facts_within_changed(&old, &now, &[SymbolId(1)]), Ok(1));
        assert_eq!(facts_within_changed(&old, &old, &[]), Ok(0));
        let e = facts_within_changed(&old, &now, &[SymbolId(0), SymbolId(2)]).unwrap_err();
        assert!(e.contains("b (1)") && e.contains("0x1000200"), "{e}");
        // A new key's facts are R-A2's (no stored row to flip from).
        let new =
            with_facts(with_facts(state(&["a", "b", "c", "n"], &[1, 2, 3, 4]), 1, 512), 3, DEAD);
        assert_eq!(facts_within_changed(&old, &new, &[]), Ok(0));
    }

    /// identity' cause 1 (the lane's 50 hops): a changed file's `KEY_REFS`
    /// put rows are its whole mentions with `demand` 0 and no delete; they
    /// leave the stored rows as they are when each is the stored row with
    /// the file's demand OR-ed in.
    #[test]
    fn identity_prime_key_refs_put_back_by_row_and_demand() {
        use mortar_core::demands::DemandFlags;
        use mortar_core::id::SymKey;
        use mortar_core::incr::{FileDemands, SlotDemand};
        use mortar_incr::update::rows::{CommonRow, KeyRefRow};
        let mut m = state(&["a", "b"], &[1, 2]);
        let stored = KeyRefRow {
            key: 1,
            file: 5,
            def: 1,
            bind: 1,
            typ: 2,
            demand: 1,
            ..KeyRefRow::default()
        };
        m.key_refs.push(stored);
        let common = CommonRow { key: 0, file: 5, size: 8, align: 8, tomb: false };
        m.commons.push(common);
        let view = m.view();
        let mut d = delta(&[0, 1]);
        d.key_refs.put.push(KeyRefRow { demand: 0, ..stored });
        d.commons.put.push(common);
        let got = [FileDemands {
            file: FileId(5),
            demands: vec![SlotDemand { key: SymKey::global(SymbolId(1)), flags: DemandFlags::GOT }],
        }];
        assert_eq!(key_edits_put_back(&view, &d, &got), Ok(()));
        // No demand: the stored row's GOT bit is lost (not identity).
        assert!(key_edits_put_back(&view, &d, &[]).is_err());
        // A changed column, a delete, a COMMONS row changed: not identity.
        let mut x = d.clone();
        x.key_refs.put[0].def = 0;
        assert!(key_edits_put_back(&view, &x, &got).is_err());
        let mut x = d.clone();
        x.key_refs.delete.push(KeyRefRow { key: 0, file: 5, ..KeyRefRow::default() });
        assert!(key_edits_put_back(&view, &x, &got).is_err());
        let mut x = d.clone();
        x.commons.put[0].size = 16;
        assert!(key_edits_put_back(&view, &x, &got).is_err());
    }

    /// identity' causes 2 and 3 (the lane's INDEX and EDGES rows): the
    /// producer's rows in walk order with repeats against the capture's
    /// sorted, deduplicated ones are the same rows; a row that differs is
    /// still caught.
    #[test]
    fn identity_prime_index_and_edges_as_sets() {
        use mortar_incr::format::IndexRow;
        use mortar_incr::update::rows::EdgeRow;
        let i = |unit, kind, target| IndexRow { unit, kind, target };
        // The producer's walk: repeats, not sorted (the lane's "24 INDEX
        // rows (stored 15)", row 0's target 19 against 17).
        let mut now = vec![i(92, 0, 19), i(92, 0, 17), i(92, 0, 19), i(92, 2, 17), i(92, 0, 17)];
        let mut was = vec![i(92, 0, 17), i(92, 0, 19), i(92, 2, 17)];
        assert_eq!(differ_as_sets(&mut now, &mut was), None);
        let mut now = vec![i(92, 0, 19), i(92, 0, 18)];
        let mut was = vec![i(92, 0, 17), i(92, 0, 19)];
        assert!(differ_as_sets(&mut now, &mut was).is_some());
        let e = |kind, source| EdgeRow { kind, source, target: 0, offset: 0 };
        // Row 7's kind 11 (SPLIT) on 101 against kind 9 (ROOT) on 102.
        let mut now = vec![e(11, 101), e(9, 102)];
        let mut was = vec![e(9, 102), e(11, 101)];
        assert_eq!(differ_as_sets(&mut now, &mut was), None);
        let mut now = vec![e(11, 101)];
        let mut was = vec![e(9, 102), e(11, 101)];
        assert!(differ_as_sets(&mut now, &mut was).is_some());
    }

    /// identity' cause 4 (the lane's 78 hops after causes 1-3): a chunk
    /// the delta touched is in `hole_chunks` with its holes unchanged
    /// (identity); a hole that differs is caught.
    #[test]
    fn identity_prime_touched_chunks_by_their_holes() {
        use mortar_core::ChunkId;
        use mortar_core::incr::MemoryLayout;
        use mortar_core::layout::{Hole, HoleKind};
        let h = |chunk, offset, size| Hole {
            chunk: ChunkId(chunk),
            offset,
            size,
            kind: HoleKind::Fill,
        };
        let rows = MemoryLayout { holes: vec![h(3, 16, 8), h(4, 0, 4)], ..MemoryLayout::default() };
        let stored = StoredLayout::new(&rows);
        let mut d =
            LayoutDelta { hole_chunks: vec![ChunkId(2), ChunkId(3)], ..LayoutDelta::default() };
        d.placement.holes = vec![h(3, 16, 8)].into_boxed_slice();
        assert_eq!(touched_chunks_keep_their_holes(&stored, &d), Ok(()));
        d.placement.holes = vec![h(3, 16, 4)].into_boxed_slice();
        assert!(touched_chunks_keep_their_holes(&stored, &d).is_err());
        d.placement.holes = Vec::new().into_boxed_slice();
        assert!(touched_chunks_keep_their_holes(&stored, &d).is_err());
    }

    /// `link_arch_of` is the front's selection: every target the update
    /// reads from `META.machine` gives the architecture of that machine.
    #[test]
    fn link_arch_of_every_machine() {
        let plan =
            Plan { kind: mortar_core::plan::OutputKind::StaticExecutable, ..Plan::default() };
        for m in [Machine::AArch64, Machine::X86_64, Machine::I386] {
            assert_eq!(machine_of(m.e_machine()), Some(m));
            let (_, target, _, _arch) = link_arch_of(&plan, m, true).unwrap();
            assert_eq!(target, mortar_layout::Target::of(m));
        }
        assert_eq!(machine_of(0), None);
    }
}
