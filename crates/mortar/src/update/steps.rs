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
    for i in 0..rows {
        let r = view.stat(i)?;
        if r.tag == stat_tag::INPUT && paths.binary_search(&view.string(r.path)?).is_err() {
            return Err(script_refusal("an input that gave no file (a script input?)"));
        }
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
///   `INDEX`, `EDGES` (targets by their class), `FRAG_USE` (the file's
///   pieces' fragments as a set), no `@` name;
/// - the affected keys: no new, no gone key, no first mention moved, the
///   `KEY_REFS`/`GROUPS`/`COMMONS` edits putting back what they delete, no
///   keeper re-elected, each `resolved` row (with `facts`) the stored one
///   (its section by `(file, index)`);
/// - the layout delta: nothing moved, killed, revived, no hole, no region
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
        // INDEX.
        let old = view.index_of(file).map_err(e)?;
        if old.len() != c.index.len() {
            return Err(format!(
                "file {}: {} INDEX rows (stored {})",
                file.0,
                c.index.len(),
                old.len()
            ));
        }
        for (k, (n, o)) in c.index.iter().zip(&old).enumerate() {
            let mut m = *n;
            m.unit = sec(m.unit)?;
            m.target = match m.kind {
                index_kind::GLOBAL | index_kind::SLOT => key(m.target)?,
                _ => sec(m.target)?,
            };
            if m != *o {
                return Err(format!("file {}: INDEX row {k}: {m:?} (stored {o:?})", file.0));
            }
        }
        // EDGES.
        let old = view.edges_of(file).map_err(e)?;
        if old.len() != c.edges.len() {
            return Err(format!(
                "file {}: {} EDGES rows (stored {})",
                file.0,
                c.edges.len(),
                old.len()
            ));
        }
        for (k, (n, o)) in c.edges.iter().zip(&old).enumerate() {
            let mut m = *n;
            m.source = sec(m.source)?;
            m.target = match edge_kind::target(m.kind) {
                Some(edge_kind::Target::Key) => key(m.target)?,
                Some(edge_kind::Target::Section) => sec(m.target)?,
                Some(edge_kind::Target::Zero | edge_kind::Target::Code) => m.target,
                None => return Err(format!("file {}: EDGES kind {}", file.0, m.kind)),
            };
            if m != *o {
                return Err(format!("file {}: EDGES row {k}: {m:?} (stored {o:?})", file.0));
            }
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
    if !same_edits(&delta.key_refs.put, &delta.key_refs.delete)
        || !same_edits(&delta.groups.put, &delta.groups.delete)
        || !same_edits(&delta.commons.put, &delta.commons.delete)
        || !delta.reelected.is_empty()
    {
        return Err("KEY_REFS, GROUPS or COMMONS edited, or a keeper re-elected".into());
    }
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
    let counts = [
        d.moved.len(),
        d.killed.len(),
        d.revived.len(),
        d.holes.len(),
        d.hole_chunks.len(),
        d.regions_opened.len(),
        d.growth_steps.len(),
        d.frags.len(),
        d.slots_appended.len(),
        d.slots_dead.len(),
        d.slots_revived.len(),
    ];
    if counts.iter().any(|&n| n != 0) {
        return Err(format!(
            "the layout delta: moved, killed, revived, holes, hole chunks, regions, growth steps, \
             new FRAGS, slots appended, dead, revived: {counts:?}"
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

/// `KEY_REFS`/`GROUPS`/`COMMONS` edits that put back what they delete.
fn same_edits<R: PartialEq>(put: &[R], delete: &[R]) -> bool {
    put.len() == delete.len() && put.iter().all(|p| delete.contains(p))
}

fn text(m: &MemoryState, s: Str) -> Option<&[u8]> {
    m.strings.get(s.off as usize..s.off as usize + s.len as usize)
}

/// A key's `RESOLVED` answer with its strings and its definer's section by
/// identity `(file, anchor kind, anchor, name, ordinal)`.
type Answer<'m> = ((u8, u8, u8, u8, u64, u64, u32), Option<(u32, u8, &'m [u8], &'m [u8], u32)>);

fn answer(m: &MemoryState, k: usize) -> Option<Answer<'_>> {
    let r = m.resolved.get(k)?;
    let at = if r.section == NONE {
        None
    } else {
        let s = m.sections.get(r.section as usize)?;
        Some((s.file, s.anchor_kind, text(m, s.anchor)?, text(m, s.name)?, s.ordinal))
    };
    Some(((r.kind, r.typ, r.bind, r.other, r.value, r.size, r.facts), at))
}

/// **R-A2, the count check (BINDING)**: every global key whose `RESOLVED`
/// answer in a front's capture `capture` differs from the stored state's
/// `old` (by name and version; the answer's columns and its definer's
/// section by identity), and every key the capture has that `old` does
/// not, is a key the update chose: in `delta.affected` (or, new, in
/// `delta.new_keys`). It is computed on the front, so it is independent of
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
    use std::collections::HashMap;
    let globals = |m: &MemoryState| -> HashMap<(Vec<u8>, Vec<u8>), usize> {
        let mut by: HashMap<(Vec<u8>, Vec<u8>), usize> = HashMap::new();
        for (k, s) in m.symbols.iter().enumerate() {
            if s.kind != sym_kind::LOCAL
                && let (Some(n), Some(v)) = (text(m, s.name), text(m, s.version))
            {
                by.entry((n.to_vec(), v.to_vec())).or_insert(k);
            }
        }
        by
    };
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
