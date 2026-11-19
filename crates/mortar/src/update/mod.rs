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

pub mod changed;
pub mod file_front;
pub mod file_split;
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
use mortar_incr::ops::WriteFailure;
use mortar_incr::stamp::Tuple;

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
    pub out_path: &'c Path,
    /// The line's words (`Parsed::args`): the map's `arg` records, as the
    /// driver gives them to the full link's front (F-9).
    pub args: &'c [std::ffi::OsString],
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

/// Why [`run`] did not finish an update (ruling M2a-4).
#[derive(Debug)]
pub enum UpdateError {
    /// A reason found before the first write (IN-77e: before W1/C1, or a
    /// `Fallback` of `write_with`): the caller links in full, or under
    /// `--incremental=update` reports it as the error.
    Fallback(IncrReason),
    /// An I/O failure of a write step (IN-77e: W1..W6, C1..C5, and the
    /// map after them): the LINK FAILS with the step and the error, and
    /// no full link follows. From W1 on the state's flag is set (W1) and
    /// stays set on every failure before W6's flag write, so the next
    /// link refuses that state (IN-77f): no old state describes the
    /// output it left.
    Failed { step: &'static str, error: std::io::Error },
}

impl From<IncrReason> for UpdateError {
    fn from(r: IncrReason) -> Self {
        Self::Fallback(r)
    }
}

impl UpdateError {
    /// The failed link's diagnostic (`cannot-write-output`, naming the
    /// step and the error; the file is the state for W1, W5, W6, C4 and
    /// C5, the map for `map`, the output otherwise).
    #[must_use]
    pub fn failed_diagnostic(
        step: &str,
        error: &std::io::Error,
        out: &Path,
        state: &Path,
        map: Option<&Path>,
    ) -> mortar_diag::Diagnostic {
        let path = match step {
            "W1" | "W5" | "W6" | "C4" | "C5" => state,
            "map" => map.unwrap_or(out),
            _ => out,
        };
        mortar_diag::Diagnostic::error(mortar_diag::Kind::CannotWriteOutput {
            path: path.display().to_string(),
            reason: format!("the incremental update's {step} failed: {error}"),
        })
        .note("the update stopped there (spec/incr.md IN-77e); the next link is a full link")
    }
}

/// Steps U2 on (IN-70) for a session that U1 accepted
/// (`Start::Update`): the update, the reason it falls back (before its
/// first write), or the write step that failed.
///
/// # Errors
/// [`UpdateError`].
pub fn run(
    session: &Session,
    parsed: &Parsed,
    cwd: &Path,
    ctx: &Ctx<'_>,
) -> Result<Updated, UpdateError> {
    let ops = TEST_OPS.with(|t| t.borrow_mut().take());
    match ops {
        Some(mut ops) => run_with(session, parsed, cwd, ctx, &mut *ops),
        None => run_with(session, parsed, cwd, ctx, &mut mortar_incr::ops::StdOps),
    }
}

std::thread_local! {
    /// The tests' [`WriteOps`](mortar_incr::ops::WriteOps) for the next
    /// [`run`] on this thread ([`with_ops`]); `None` in every link.
    static TEST_OPS: std::cell::RefCell<Option<Box<dyn mortar_incr::ops::WriteOps>>> =
        const { std::cell::RefCell::new(None) };
}

/// Tests (ruling M2a-4: a failed W-step through `WriteOps`): `go` with
/// `ops` as the write operations of the first update [`run`] makes on
/// this thread (an in-process link, `crate::invoke_in`).
#[doc(hidden)]
pub fn with_ops<T>(ops: Box<dyn mortar_incr::ops::WriteOps>, go: impl FnOnce() -> T) -> T {
    TEST_OPS.with(|t| *t.borrow_mut() = Some(ops));
    let r = go();
    TEST_OPS.with(|t| *t.borrow_mut() = None);
    r
}

fn run_with(
    session: &Session,
    parsed: &Parsed,
    cwd: &Path,
    ctx: &Ctx<'_>,
    ops: &mut dyn mortar_incr::ops::WriteOps,
) -> Result<Updated, UpdateError> {
    let mut steps = Steps::start();
    let lock = session.lock().ok_or_else(|| IncrReason::NoState(String::new()))?;
    let view = mortar_incr::state5::open(lock, session.plan_hash())?;
    let detected = detect(&view, cwd)?;
    debug_condition(&view, &detected, cwd)?;
    // Option-only reasons the callees give from the plan alone, on every
    // update: `--gc-sections` (U6's `live::live`, WP-215 seam 3).
    if parsed.plan.shaping.gc_sections {
        return Err(IncrReason::UpdateNotImplemented(Some(Unimplemented::Live)).into());
    }
    // G-7: no update producer of the text map (`-Map`, `-M`,
    // `--print-map`), and none yet of the machine map's `arg` records or
    // of the dependency file: `(image)`, before anything is read.
    // The machine map (`--mortar-map`) is U9's `map::update_map` (record
    // 0.2; its `arg` records are the front's, as the full link's).
    let b = &ctx.plan.bookkeeping;
    if b.map.is_some() || b.print_map || b.dependency_file.is_some() {
        return Err(IncrReason::UpdateNotImplemented(Some(Unimplemented::Image)).into());
    }
    // M2b-1: any number of changed objects and archive members; U4's
    // member-level judgement (the capture's hash) and U5's answers
    // (`extract`, `resolve`, which must say what the front says) decide in
    // `reference_of`. Nothing changed: no update to make.
    if detected.changed_files.is_empty() {
        return Err(IncrReason::UpdateNotImplemented(Some(Unimplemented::Resolve)).into());
    }
    let out = session.out.as_ref().ok_or_else(|| IncrReason::NoState(String::new()))?;
    // U1's tuple of the output (IN-77a), taken before the front reads.
    let checked = Tuple::of(&out.metadata().map_err(|e| IncrReason::OutputChanged(e.to_string()))?);
    steps.mark("u1-u3");
    // The front is dropped before the first write.
    let reference = image_link(ctx, cwd, None, |f| {
        steps.mark("front");
        reference_of(session, &view, &detected, f, ctx, cwd, out, &mut steps)
    })?;
    write_identity(session, &view, ctx, cwd, out, checked, reference, ops, steps)
}

/// The update's wall time per step, for the report line (the cost table
/// of the record; `Instant` reads only, in the update path alone).
struct Steps {
    at: std::time::Instant,
    done: Vec<(&'static str, std::time::Duration)>,
}

impl Steps {
    fn start() -> Self {
        Self { at: std::time::Instant::now(), done: Vec::new() }
    }
    /// The time since the previous mark, as step `name`.
    fn mark(&mut self, name: &'static str) {
        let now = std::time::Instant::now();
        self.done.push((name, now - self.at));
        self.at = now;
    }
    fn line(&self) -> String {
        use std::fmt::Write as _;
        self.done.iter().fold(String::new(), |mut s, (n, d)| {
            let _ = write!(s, " {n}={:.3}", d.as_secs_f64());
            s
        })
    }
}

/// What the identity case writes (M2a, ruling M2a-1): the refill's
/// ranges and leaves, and the state the update writes, which is the
/// REFERENCE of judgement (d) (below).
struct Reference {
    refill: mortar_image::update::Refill,
    size: u64,
    state: mortar_incr::update::testing::MemoryState,
    /// U9's machine map (`map::update_map`), rendered, when the line asks
    /// for one; written after the output and the state (I-79: the maps of
    /// a successful link only).
    map: Option<Vec<u8>>,
}

/// **The reference of judgement (d)** (ruling M2a-1, G-11): the capture
/// of a full link with the update's placement, here the capture of path
/// (A)'s front `f`, whose fresh placement is checked equal to the stored
/// one first (the identity case of G-3: every decision row of the
/// capture equals the stored state's under IN-78's canonical form, the
/// output's binding, `STAT`, `LEAVES` and the changed file's `FILES` row
/// aside; else `update-not-implemented (layout)`), with the refill's
/// leaves and build id. M2a WRITES it whole (an M2a update is O(link)
/// twice over: the front and the whole state); M2b writes the patch and
/// keeps this as the in-crate (d)'s comparison.
///
/// The refill (U9): the changed file's sections, and every chunk that is
/// not an input-section body rewritten whole (headers, merged and
/// synthetic chunks: `.symtab`, `.eh_frame`, the note), so every byte of
/// `P_U` that can differ is refilled; a range equal to the previous
/// output's is dropped (IN-75).
#[allow(
    clippy::too_many_arguments,
    reason = "path (A)'s front, U1's view and binding, the step timer"
)]
fn reference_of(
    session: &Session,
    view: &StateView<'_>,
    detected: &Detected,
    f: &crate::front::Front<'_>,
    ctx: &Ctx<'_>,
    cwd: &Path,
    out: &std::fs::File,
    steps: &mut Steps,
) -> Result<Reference, IncrReason> {
    use mortar_core::incr::{LayoutDelta, RefillChunk, RefillSet, StoredLayout, SymtabPlan};
    let layout = || IncrReason::UpdateNotImplemented(Some(Unimplemented::Layout));
    let image = || IncrReason::UpdateNotImplemented(Some(Unimplemented::Image));
    // U4: the front's file of each id is the state's row (and no file
    // appeared: the front has the state's files, M2b-1).
    let files = &detected.changed_files;
    if f.inputs.files.len() as u64 != view.count(kind::FILES) {
        return Err(IncrReason::UpdateNotImplemented(Some(Unimplemented::Resolve)));
    }
    for &file in files {
        let row = view.file(file)?;
        let path = view.string(row.path)?;
        if f.inputs.files.get(file).is_none_or(|x| x.path != path.as_slice()) {
            return Err(IncrReason::UpdateNotImplemented(Some(Unimplemented::Resolve)));
        }
    }
    let meta = view.meta()?;
    let leaves = view.leaves()?;
    let rows = mortar_incr::state5::LayoutOf { view };
    let stored = StoredLayout::new(&rows);
    let delta = LayoutDelta { files: files.clone(), ..LayoutDelta::default() };
    let chunks: Vec<RefillChunk> = f
        .layout
        .placement
        .chunks
        .iter()
        .filter(|(_, c)| !matches!(c.kind, mortar_core::layout::ChunkKind::Body(_)))
        .map(|(chunk, _)| RefillChunk { chunk, reasons: 0 })
        .collect();
    let set = RefillSet { chunks, ..RefillSet::default() };
    let symtab = SymtabPlan::default();
    let want_map = ctx.plan.bookkeeping.mortar_map.is_some();
    let (refill, size, map) = crate::image::with_link(f, ctx.store, &|_| {}, |l| {
        let u = mortar_image::update::refill(l, &delta, &symtab, &set, &stored, out, &leaves)
            .map_err(|e| match e {
                mortar_image::update::RefillError::Fallback(r) => r,
                mortar_image::update::RefillError::Failed(_) => image(),
            })?;
        let map = if want_map {
            let m = mortar_image::map::update_map(l, &delta, &stored, &symtab)?;
            Some(mortar_image::map::render(&m, &f.pool))
        } else {
            None
        };
        Ok::<_, IncrReason>((u, mortar_image::update::output_size(l), map))
    })
    .map_err(|_| image())??;
    steps.mark("refill");
    // The capture of `f` with the refill's binding parts.
    let tree = match meta.leaf_fn {
        0 => mortar_digest::TreeFn::L128,
        1 => mortar_digest::TreeFn::Sha1,
        _ => mortar_digest::TreeFn::Sha256,
    };
    let written = mortar_image::link::Written {
        build_id: refill.build_id.clone(),
        leaves: Some((tree, Vec::new())),
        sealed: Some(meta.sealed_start..meta.sealed_end),
        file_size: size,
        file: None,
        rows: 0,
        jobs: 0,
    };
    let (mut state, rows5) = session.state(f, ctx.store, &written, cwd).map_err(|_| image())?;
    state.leaves.clone_from(&refill.leaves);
    let rows5 = rows5.ok_or_else(image)?;
    let state = mortar_incr::capture::state5_rows(&state, rows5, false).map_err(|_| image())?;
    steps.mark("capture");
    // U4-U5 (M2b-1): the changed files' rows in the update's ids, then
    // `extract` and `resolve` on them; the update goes on only where they
    // say what the front (its capture) says.
    changed::u5(view, &detected.changed_files, &detected.positional, f, &state, &mut |n| {
        steps.mark(n);
    })?;
    // The identity check (G-3's materialization in the identity case).
    let lock = session.lock().ok_or_else(image)?;
    let corrupt = |e: std::io::Error| IncrReason::StateCorrupt(e.to_string());
    let len = usize::try_from(lock.metadata().map_err(corrupt)?.len()).map_err(|_| image())?;
    let mut bytes = vec![0u8; len];
    std::os::unix::fs::FileExt::read_exact_at(lock, &mut bytes, 0).map_err(corrupt)?;
    let old = mortar_incr::state5::canon::canon(&bytes)
        .map_err(|e| IncrReason::StateCorrupt(e.to_string()))?;
    let new = mortar_incr::state5::canon::canon_of(&state)
        .map_err(|e| IncrReason::StateCorrupt(e.to_string()))?;
    let diffs = mortar_incr::state5::canon::compare(&new, &old, &stored);
    if !identity(&diffs, files) {
        return Err(layout());
    }
    steps.mark("check");
    Ok(Reference { refill, size, state, map })
}

/// Whether the differences of the capture against the stored state are
/// those of the identity case (ruling M2a-2): only
/// - `META`'s fields of the output's binding: the build id, the boot id,
///   `output_synced` (W5's) and `compactions` (a history field), which the
///   canonical form clears (`state5::canon::canon_of`), so a `META`
///   difference that remains is any OTHER field: refused;
/// - the output's `STAT` stamp tuple (cleared by the canonical form) and
///   the changed input's `STAT` row (its stamps and hash);
/// - `LEAVES` (the leaves and their root: the bytes changed);
/// - the `FILES` rows of the changed files (`row <id>`: the canonical
///   `FILES` table is in id order; their stamps and hash; M2b-1: every
///   file of a changed path, the members whose bytes are equal giving no
///   difference).
fn identity(diffs: &[mortar_incr::state5::canon::CanonDiff], files: &[FileId]) -> bool {
    let rows: Vec<String> = files.iter().map(|f| format!("row {}", f.0)).collect();
    diffs.iter().all(|d| match d.kind {
        kind::STAT | kind::LEAVES => true,
        kind::FILES => rows.contains(&d.row),
        _ => false,
    })
}

#[allow(
    clippy::too_many_arguments,
    reason = "U10's inputs: the session, U1's binding, U9's result"
)]
/// U10 for the identity case: `write_with` the refill's ranges in place
/// (IN-76, IN-77), W5 the reference state with W4's binding of the
/// output, whole (`write_state_whole`, ruling M2a-1).
fn write_identity(
    session: &Session,
    view: &StateView<'_>,
    ctx: &Ctx<'_>,
    cwd: &Path,
    out: &std::fs::File,
    checked: Tuple,
    r: Reference,
    ops: &mut dyn mortar_incr::ops::WriteOps,
    mut steps: Steps,
) -> Result<Updated, UpdateError> {
    let lock = session.lock().ok_or_else(|| IncrReason::NoState(String::new()))?;
    let pair = write::Pair {
        out,
        out_path: ctx.out_path,
        state: lock,
        state_path: session.state_path(),
        checked,
        size: r.size,
    };
    // `--incremental=update` ignores `small-output` (U1): its write takes
    // the in-place path at every size; otherwise IN-45's bound (G-13's
    // knob in a testhooks build).
    let bound = if session.update_mode() {
        0
    } else {
        mortar_incr::testhook::small_output_bound().unwrap_or(mortar_incr::SMALL_OUTPUT)
    };
    let policy = mortar_incr::WritePolicy { in_place_from: bound, ..mortar_incr::WRITE_PATH };
    let state = r.state;
    let path = session.state_path();
    let mut w5 = |output: &StatRow, ops: &mut dyn mortar_incr::ops::WriteOps| {
        let mut m = state.clone();
        m.stat.push(output.clone());
        let bytes = mortar_incr::capture::state5_encode(&m)
            .map_err(|e| WriteFailure::Failed { step: "W5", error: std::io::Error::other(e) })?;
        match write::write_state_whole(path, &bytes, ops) {
            Ok(true) => Ok(()),
            Ok(false) => {
                Err(WriteFailure::Failed { step: "W5", error: std::io::Error::other("state-busy") })
            }
            Err(error) => Err(WriteFailure::Failed { step: "W5", error }),
        }
    };
    // Ruling M2a2-1: an update writes no format-4 side state (format 5
    // only; the side state and the dry run go at M6) and removes a stale
    // one, before the first write: it describes an output that is about
    // to stop existing.
    let side = crate::incremental::format4_path(session.state_path());
    match std::fs::remove_file(&side) {
        Ok(()) => {}
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => return Err(UpdateError::Failed { step: "format4", error }),
    }
    let w = write::write_with(
        &pair,
        view,
        &r.refill.ranges,
        &r.refill.bytes,
        true,
        ops,
        policy,
        &mut w5,
    )
    .map_err(|e| match e {
        WriteFailure::Fallback(reason) => UpdateError::Fallback(reason),
        WriteFailure::Failed { step, error } => UpdateError::Failed { step, error },
    })?;
    if let (Some(m), Some(bytes)) = (ctx.plan.bookkeeping.mortar_map.as_deref(), &r.map) {
        let path = cwd.join(m);
        mortar_image::output::write_whole(&path, bytes, 0o666).map_err(|(_, d)| {
            UpdateError::Failed { step: "map", error: std::io::Error::other(format!("{d:?}")) }
        })?;
    }
    steps.mark("write");
    let c = &r.refill.counts;
    let line = format!(
        "incremental: update ({:?}{}) changed=1 ranges={} bytes={} (M2a: O(link) twice over, the front and the whole state) steps:{}\n",
        w.path,
        w.copy_because.map(|b| format!(": {b}")).unwrap_or_default(),
        c.ranges,
        c.dirty,
        steps.line()
    );
    // `--incremental-report`: the file, or standard error (the driver's).
    let mut lines = String::new();
    session.emit_report(&line, &mut lines);
    Ok(Updated { lines })
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
            f.args = ctx.args.iter().map(|a| a.as_encoded_bytes().to_vec()).collect();
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
        let t = Tuple::of(&m);
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
        if object_has_debug(&object) {
            return Err(debug());
        }
    }
    Ok(())
}

/// Whether `object` has a non-alloc `.debug_*` section (IN-70's debug
/// condition on the new bytes; `ChangedFile::debug`).
pub(crate) fn object_has_debug(object: &mortar_elf::Object<'_>) -> bool {
    use mortar_elf::consts::SHF_ALLOC;
    (1..object.section_count()).any(|i| {
        object.section(mortar_elf::SectionIndex(i)).is_ok_and(|s| {
            s.flags() & SHF_ALLOC == 0 && s.name().is_ok_and(|n| n.starts_with(b".debug_"))
        })
    })
}
