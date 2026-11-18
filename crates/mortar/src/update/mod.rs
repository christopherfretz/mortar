//! The incremental update's orchestration (`spec/incr.md` IN-70: steps
//! U1..U10 in order) and its writes. WP-213 (U-1) created the files:
//! `write.rs` (**U-7**: IN-76, IN-77), `probe.rs` (**W-1**: IN-76a).
//!
//! **WP-225 (U-10, the join).** U1 runs in `crate::incremental::start`
//! (the lock, the flag, `state5::open`, `META`, the output's binding,
//! `boot_check`, `small-output`): a state that passes it is
//! `Start::Update`, and [`run`] takes the steps from U2 on. M1 (this
//! state of the tree): U2 (change detection on the `StateView`) and U3
//! (the positional set) are complete; IN-70's debug condition and the
//! option-only reasons follow; every later step is not wired and the
//! update falls back by the word of the first callee not yet called
//! (`update-not-implemented (resolve)`: U4's producers and U5). Nothing
//! is written before U9, so every fall-back here is before the first
//! write.
//!
//! The order of calls kept open for the packages after this one: the
//! debug step (G-3) is `debug::strings` before `delta::layout` and
//! `debug::place` after it (U7); liveness (WP-238) is U6, between U5 and
//! U7 (`live::live`, whose `LiveDelta` feeds `SectionChange::live`).

pub mod file_front;
pub mod probe;
pub mod stored_rows;
pub mod write;

use std::path::{Path, PathBuf};

use mortar_cli::{Parsed, Plan};
use mortar_core::id::FileId;
use mortar_core::incr::{IncrReason, Unimplemented};
use mortar_core::par::Pool;
use mortar_diag::{Collector, Policy};
use mortar_incr::detect::{Stat, stat_rule};
use mortar_incr::format::{StatRow, kind, stat_tag};
use mortar_incr::update::rows::file_flags;
use mortar_incr::update::view::StateView;
use mortar_input::store::Store;

use crate::incremental::Session;

/// U2 and U3's result (IN-22, IN-24, IN-28a), in the state's terms.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Detected {
    /// `STAT` rows (inputs) whose content differs (or that are gone).
    pub changed_paths: Vec<u32>,
    /// `STAT` rows not clean by the stat rule, hashed, and found equal.
    pub hashed_same: Vec<u32>,
    /// `FILES` rows whose path is one of `changed_paths`' (every member of a
    /// changed archive: the member-level judgement is U4's), sorted.
    pub changed_files: Vec<FileId>,
    /// U3: the files with `POSITIONAL` set, none of them changed.
    pub positional: Vec<FileId>,
}

/// What [`run`] needs to run path (A)'s front (WP-225 M2a step 1): the
/// full link's own resources, built by the driver before the session
/// starts. Borrowed: an update that falls back leaves them to the full
/// link unchanged.
pub struct Ctx<'c> {
    pub plan: &'c Plan,
    pub store: &'c Store,
    /// The link's diagnostic switches: path (A)'s front emits into a
    /// collector of its own (a fall-back after it must not report the
    /// front's diagnostics twice).
    pub policy: Policy,
    pub pool: Pool,
}

impl std::fmt::Debug for Ctx<'_> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Ctx").field("pool", &self.pool).finish_non_exhaustive()
    }
}

/// A finished update: what the driver puts into the `Invocation` (the
/// `incremental:` lines). Not made at this state of the tree (M2a: U5 on
/// are not wired).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Updated {
    pub lines: String,
}

/// Steps U2 on (IN-70) for a session that U1 accepted
/// (`Start::Update`): the update, or the reason it falls back (before
/// its first write; the caller links in full, or under
/// `--incremental=update` reports it as the error).
///
/// # Errors
/// The fall-back's reason.
pub fn run(
    session: &Session,
    parsed: &Parsed,
    cwd: &Path,
    ctx: &Ctx<'_>,
) -> Result<Updated, IncrReason> {
    let lock = session.lock().ok_or_else(|| IncrReason::NoState(String::new()))?;
    let view = mortar_incr::state5::open(lock, session.plan_hash())?;
    let detected = detect(&view, cwd)?;
    debug_condition(&view, &detected, cwd)?;
    // Option-only reasons the callees give from the plan alone, on every
    // update: `--gc-sections` (U6's `live::live`, WP-215 seam 3).
    if parsed.plan.shaping.gc_sections {
        return Err(IncrReason::UpdateNotImplemented(Some(Unimplemented::Live)));
    }
    // G-7: no update producer of the text map (`-Map`, `-M`,
    // `--print-map`), and none yet of the machine map's `arg` records or
    // of the dependency file: `(image)`, before anything is read.
    let b = &ctx.plan.bookkeeping;
    if b.map.is_some() || b.print_map || b.mortar_map.is_some() || b.dependency_file.is_some() {
        return Err(IncrReason::UpdateNotImplemented(Some(Unimplemented::Image)));
    }
    // M2a: one changed object, not an archive member (U4's member-level
    // judgement and U5's extraction loop are M2b's).
    if detected.changed_files.len() != 1 || detected.changed_paths.len() != 1 {
        return Err(IncrReason::UpdateNotImplemented(Some(Unimplemented::Resolve)));
    }
    if view.file(detected.changed_files[0])?.member != 0 {
        return Err(IncrReason::UpdateNotImplemented(Some(Unimplemented::Resolve)));
    }
    image_link(ctx, cwd, None, |_front| {
        // U4 (the changed files' rows from path (A)'s front,
        // `capture::file_rows`) and U5 (extract, resolve): the next
        // lines of M2a (WIP.md).
        Err(IncrReason::UpdateNotImplemented(Some(Unimplemented::Resolve)))
    })
}

/// Path (A) (record 0.1, ruling G-1): the WHOLE front over the new
/// inputs, the update's placement `P_U` replayed when `replay` is given
/// (materialized into the front's ids, G-3), and `go` on it. The front
/// emits into a collector of its own: a front that fails or says anything
/// at all is left to the full link, which says it once
/// (`update-not-implemented (diags)`: M2a updates silent links only).
///
/// # Errors
/// `go`'s, or the fall-back above.
pub fn image_link<T>(
    ctx: &Ctx<'_>,
    cwd: &Path,
    replay: Option<&crate::front::Replay<'_>>,
    go: impl FnOnce(&crate::front::Front<'_>) -> Result<T, IncrReason>,
) -> Result<T, IncrReason> {
    let diag = Collector::new(ctx.policy);
    let env = crate::front::Env { cwd, pool: ctx.pool, replay, slack: true, state5: true };
    let mut timer = crate::timing::Timer::new(false);
    let front = crate::front::front(ctx.plan.clone(), env, ctx.store, &diag, &mut timer);
    let quiet = diag.finish().diagnostics().is_empty();
    match front {
        Some(mut f) if quiet => {
            f.keep_leaves = true;
            go(&f)
        }
        _ => Err(IncrReason::UpdateNotImplemented(Some(Unimplemented::Diags))),
    }
}

/// A path of the state's `STRINGS`, relative to `cwd` as the link opened it.
fn path_of(bytes: &[u8], cwd: &Path) -> PathBuf {
    cwd.join(<std::ffi::OsStr as std::os::unix::ffi::OsStrExt>::from_bytes(bytes))
}

/// U2 (IN-22, IN-22a, IN-24) and U3 (IN-28a) on the view: every input path
/// of `STAT` by the stat rule against the state's device stamps, a path
/// not clean hashed and compared with its stored hash; then one pass over
/// `FILES` for the files of the changed paths and the positional set (U3:
/// the `POSITIONAL` bit of every row, IN-28a; O(files), the rule's own
/// read).
///
/// # Errors
/// The view's (`state-corrupt`).
pub fn detect(view: &StateView<'_>, cwd: &Path) -> Result<Detected, IncrReason> {
    let rows = u32::try_from(view.count(kind::STAT)).unwrap_or(u32::MAX);
    let mut stat: Vec<StatRow> = Vec::with_capacity(rows as usize);
    for i in 0..rows {
        stat.push(view.stat(i)?);
    }
    let devices: Vec<StatRow> =
        stat.iter().filter(|r| r.tag == stat_tag::DEVICE).cloned().collect();
    let mut d = Detected::default();
    for (i, r) in stat.iter().enumerate() {
        if r.tag != stat_tag::INPUT {
            continue;
        }
        let i = i as u32;
        let path = path_of(&view.string(r.path)?, cwd);
        let Ok(m) = std::fs::metadata(&path) else {
            d.changed_paths.push(i);
            continue;
        };
        let t = mortar_incr::stamp::Tuple::of(&m);
        if stat_rule(r, (t.dev, t.ino, t.size, t.mtime_ns, t.ctime_ns), &devices) == Stat::Clean {
            continue;
        }
        let same = std::fs::read(&path).is_ok_and(|b| {
            crate::incremental::hash_bytes(mortar_input::store::content_hash(&b)) == r.hash
        });
        if same { d.hashed_same.push(i) } else { d.changed_paths.push(i) }
    }
    // A file's `mention` is its ordinal among the files of its path
    // (`mortar_core::identity`), not a `STAT` row: the files of a changed
    // path are found by the path's bytes.
    let mut changed: Vec<Vec<u8>> = Vec::with_capacity(d.changed_paths.len());
    for &i in &d.changed_paths {
        changed.push(view.string(stat[i as usize].path)?);
    }
    changed.sort_unstable();
    let files = u32::try_from(view.count(kind::FILES)).unwrap_or(u32::MAX);
    for f in 0..files {
        let id = FileId(f);
        let row = view.file(id)?;
        if row.flags & file_flags::GONE != 0 {
            continue;
        }
        if !changed.is_empty() && changed.binary_search(&view.string(row.path)?).is_ok() {
            d.changed_files.push(id);
        } else if row.flags & file_flags::POSITIONAL != 0 {
            d.positional.push(id);
        }
    }
    Ok(d)
}

/// IN-70's debug condition, at the end of U2: a changed file whose old
/// version (its stored `SECTIONS` rows) or new version (its bytes) has a
/// non-allocated section named `.debug_*` ends the update with
/// `update-not-implemented (debug)` (until G-3), before anything is read
/// past the change detection. A new version that does not parse is left
/// to the full link (it reports it). The `DW_UT_skeleton` test of IN-55a
/// is G-2's `split::classify_object`, wired with U4.
fn debug_condition(view: &StateView<'_>, d: &Detected, cwd: &Path) -> Result<(), IncrReason> {
    use mortar_elf::consts::SHF_ALLOC;
    let debug = || IncrReason::UpdateNotImplemented(Some(Unimplemented::Debug));
    for &f in &d.changed_files {
        for r in view.sections_of(f)? {
            if r.sh_flags & SHF_ALLOC == 0 && view.string(r.name)?.starts_with(b".debug_") {
                return Err(debug());
            }
        }
        let row = view.file(f)?;
        let Ok(bytes) = std::fs::read(path_of(&view.string(row.path)?, cwd)) else { continue };
        let bytes = if row.member != 0 {
            let at = usize::try_from(row.base).unwrap_or(usize::MAX);
            let len = usize::try_from(row.len).unwrap_or(usize::MAX);
            match bytes.get(at..at.saturating_add(len)) {
                Some(b) => b.to_vec(),
                None => continue,
            }
        } else {
            bytes
        };
        let Ok(object) = mortar_elf::Object::parse(&bytes) else { continue };
        for i in 1..object.section_count() {
            let Ok(s) = object.section(mortar_elf::SectionIndex(i)) else { continue };
            if s.flags() & SHF_ALLOC == 0 && s.name().is_ok_and(|n| n.starts_with(b".debug_")) {
                return Err(debug());
            }
        }
    }
    Ok(())
}
