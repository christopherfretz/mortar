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
pub mod file_scan;
pub mod file_split;
pub mod file_symbols;
pub mod layout_in;
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
    _parsed: &Parsed,
    cwd: &Path,
    ctx: &Ctx<'_>,
    ops: &mut dyn mortar_incr::ops::WriteOps,
) -> Result<Updated, UpdateError> {
    let mut steps = Steps::start();
    let lock = session.lock().ok_or_else(|| IncrReason::NoState(String::new()))?;
    let view = mortar_incr::state5::open(lock, session.plan_hash())?;
    let detected = detect(&view, cwd)?;
    debug_condition(&view, &detected, cwd)?;
    // `--gc-sections` (gc-lift): U6's `live::live` runs in `reference_of`,
    // judged against path (A)'s own mark (`live_judged`).
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
#[allow(clippy::too_many_lines, reason = "U4 to the identity check, in IN-70's order")]
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
    if u64::from(f.inputs.files.len()) != view.count(kind::FILES) {
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
    let mut state = mortar_incr::capture::state5_rows(&state, rows5, false).map_err(|_| image())?;
    // gc-lift (FINDING gc-1): under `--gc-sections` the full link's
    // `META.live_hash` (WP-238 S4) is the driver's, after `state5_rows`
    // (`incremental.rs`, the capture's live-set hash); the update's state
    // takes it the same way, from the front's mark and positions, or it
    // would be written with a zero hash (the next update's `no-live-hash`).
    if let Some(Ok((_, at))) = f.incr5.as_ref().and_then(|i| i.live_rows.as_ref()) {
        state.index_sections()?;
        let h =
            mortar_incr::update::live::live_hash(&state.view(), f.liveness.live.as_slice(), at)?;
        state.meta.live_hash = mortar_incr::testhook::live_hash_mutated(h);
    }
    steps.mark("capture");
    // U4-U5 (M2b-1): the changed files' rows in the update's ids, then
    // `extract` and `resolve` on them; the update goes on only where they
    // say what the front (its capture) says.
    let u5 =
        changed::u5(view, &detected.changed_files, &detected.positional, f, &state, &mut |n| {
            steps.mark(n);
        })?;
    // U6 (gc-lift): under `--gc-sections`, `live::live`'s delta judged
    // against the front's mark (a disagreement is a FINDING, by name); the
    // number of sections whose liveness flips. The ONE walk of the update
    // (M2b-2d): `placement` takes its delta; without `--gc-sections` the
    // delta is empty and nothing is walked (`live::live`'s own answer).
    let mut flips = 0usize;
    // U6-U7 (M2b-2b): the delta's placement of the changed files, judged
    // against the front's (a disagreement is a FINDING, by name).
    let placed = match u5 {
        Some((cf, delta)) => {
            let mut live = mortar_incr::update::live::LiveDelta::default();
            if f.plan.shaping.gc_sections {
                (live, flips) = live_judged(view, f, &cf, &delta)?;
                steps.mark("u6");
            }
            // While the image cannot take the delta's placement (below), a
            // producer's refusal is recorded and the identity check decides
            // alone; a FINDING (the delta disagrees with the front) falls
            // back by name.
            let p =
                match placement(view, &stored, f, ctx.store, cwd, &cf, delta, &live, &state, out) {
                    Ok(p) => Some(p),
                    Err(Why::Refused) => None,
                    Err(Why::Finding(e)) => return Err(e),
                };
            steps.mark("u7");
            p
        }
        None => None,
    };
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
        // The image takes a placement only through the front's replay
        // hook, as a whole `Layout` in the front's ids (`image_link`'s
        // `replay`, `front.rs` `Env::replay`); no producer materializes a
        // format-5 `StoredLayout` with a `LayoutDelta` into one (the
        // record, M2b-2b): the delta's placement is judged, not written.
        if let Some(p) = &placed {
            changed::record(&format!(
                "M2b-2b placement: judged, not the stored one ({}): (layout)",
                p.summary
            ));
        }
        // A section that dies or revives changes the placement (its
        // `SECTIONS` row's `COLLECTED` bit, its place): taken from M2b-3 on.
        if flips > 0 {
            changed::record(&format!(
                "gc-lift: {flips} sections flip liveness, not the stored placement: (layout)"
            ));
        }
        // The tables that differ (kind: rows), for the findings file.
        let mut tables: std::collections::BTreeMap<u32, usize> = std::collections::BTreeMap::new();
        for d in diffs.iter().filter(|d| !identity(std::slice::from_ref(d), files)) {
            *tables.entry(d.kind).or_default() += 1;
        }
        changed::record(&format!("identity: not the stored state: (layout) tables {tables:?}"));
        return Err(layout());
    }
    // The identity case: the front's placement is the stored one, so the
    // delta's must be it too, offset for offset.
    if let Some(p) = &placed
        && !p.differ.is_empty()
    {
        return Err(changed::finding_as(
            &format!(
                "M2b-2b placement: the identity case, but delta::layout places {:?} ({})",
                p.differ, p.summary
            ),
            layout(),
        ));
    }
    // G-3 (WP-230): G-1's decisions judged against the front's bytes.
    debug_judged(
        view,
        &stored,
        f,
        files,
        placed.as_ref().and_then(|p| p.debug.as_ref()),
        &refill,
        out,
    )?;
    steps.mark("check");
    #[cfg(feature = "testhooks")]
    crate::incremental::materialize_hop(view, f);
    Ok(Reference { refill, size, state, map })
}

/// U6 under `--gc-sections` (gc-lift; IN-71a): `live::live` on the join's
/// change, judged against path (A)'s front. The front's mark (`f.liveness`,
/// the one the image discards by and the capture writes the new state's
/// `COLLECTED` bits and live hash from) is moved into the update's ids
/// (an unchanged file's section at its stored `SECTIONS` range, a section of
/// a file of `cf` at its `first_section`); the stored mark is the state's
/// `SECTIONS` bits (`KEPT` and not `COLLECTED`: the full link's mark as it
/// wrote it; a mark is a subset of the kept sections). `now_live` and
/// `now_dead` must be exactly the two marks' difference, and every unchanged
/// file with a section in it must be in `drawn_in`. A disagreement is a
/// FINDING (both sets, by name), and `live`'s own refusal falls back by its
/// name (`update-not-implemented (live)`, its rule recorded).
///
/// Returns the delta (the update's one walk, M2b-2d: [`placement`] takes
/// it) and the number of sections whose liveness flips: of the unchanged
/// files, and of the files of `cf` by `sh_index` (a replaced file's live
/// sections are in `now_dead` by their old ids and in `now_live` by their
/// fresh ones even when nothing flips).
#[allow(clippy::too_many_lines, reason = "live, the two marks, the judgement, the flips in order")]
#[allow(clippy::many_single_char_names, reason = "the front, a file, a section, a row, the delta")]
fn live_judged(
    view: &StateView<'_>,
    f: &crate::front::Front<'_>,
    cf: &mortar_incr::update::view::ChangedFiles,
    delta: &mortar_incr::update::resolve::ResolveDelta,
) -> Result<(mortar_incr::update::live::LiveDelta, usize), IncrReason> {
    use mortar_core::SectionId;
    use mortar_incr::format::section_bits::{COLLECTED, KEPT};
    use mortar_incr::update::rows::FileTable;
    let options = mortar_resolve::tables::option_keys(&f.plan, &f.inputs.link.directives);
    let d =
        mortar_incr::update::live::live_why(view, cf, delta, &options).map_err(|(e, why)| {
            changed::record(&format!("gc-lift fallback: live: {why}"));
            e
        })?;
    let finding = |what: String| {
        changed::finding_as(
            &format!("gc-lift: {what}"),
            IncrReason::UpdateNotImplemented(Some(Unimplemented::Live)),
        )
    };
    let files = u32::try_from(view.count(kind::FILES)).map_err(|_| finding("FILES".into()))?;
    // The stored mark, by the rows of the files that are not gone.
    let mut old: Vec<SectionId> = Vec::new();
    for g in (0..files).map(FileId) {
        if view.file(g)?.flags & file_flags::GONE != 0 {
            continue;
        }
        let r = view.file_index(g)?.ranges[FileTable::Sections as usize];
        for s in (r.first..r.first + r.count).map(SectionId) {
            let b = view.section(s)?.bits;
            if b & KEPT != 0 && b & COLLECTED == 0 {
                old.push(s);
            }
        }
    }
    old.sort_unstable();
    // The front's mark in the update's ids.
    let mut new: Vec<SectionId> = Vec::new();
    for (i, &v) in f.liveness.live.as_slice().iter().enumerate() {
        if !v {
            continue;
        }
        let s = SectionId(u32::try_from(i).map_err(|_| finding("section ids".into()))?);
        let (g, index) = f.resolution.sh_index(s).ok_or_else(|| finding(format!("{s:?}")))?;
        let (first, count) = match cf.files.iter().find(|c| c.file == g) {
            Some(c) => (c.first_section.0, u32::try_from(c.sections.len()).unwrap_or(u32::MAX)),
            None if g.0 < files => {
                let r = view.file_index(g)?.ranges[FileTable::Sections as usize];
                (r.first, r.count)
            }
            None => return Err(finding(format!("live section {s:?} of file {} past FILES", g.0))),
        };
        if index.0 >= count {
            return Err(finding(format!(
                "live section {} of file {} past its {count} rows",
                index.0, g.0
            )));
        }
        new.push(SectionId(first + index.0));
    }
    new.sort_unstable();
    let now_live: Vec<SectionId> =
        new.iter().filter(|s| old.binary_search(s).is_err()).copied().collect();
    let now_dead: Vec<SectionId> =
        old.iter().filter(|s| new.binary_search(s).is_err()).copied().collect();
    let base = view.count(kind::SECTIONS);
    let mut flipped: Vec<FileId> = now_live
        .iter()
        .chain(&now_dead)
        .filter(|s| u64::from(s.0) < base)
        .map(|&s| view.section(s).map(|r| FileId(r.file)))
        .collect::<Result<_, _>>()?;
    flipped.retain(|g| !cf.files.iter().any(|c| c.file == *g));
    flipped.sort_unstable();
    flipped.dedup();
    let missing: Vec<FileId> =
        flipped.iter().filter(|g| d.drawn_in.binary_search(g).is_err()).copied().collect();
    if d.now_live != now_live || d.now_dead != now_dead || !missing.is_empty() {
        let names = |v: &[SectionId]| -> Vec<String> {
            v.iter()
                .map(|&s| match changed::update_section(view, cf, s.0) {
                    Ok(k) => format!("{} {}:{}", s.0, k.0, String::from_utf8_lossy(&k.3)),
                    Err(e) => format!("{} ({})", s.0, e.report()),
                })
                .collect()
        };
        return Err(finding(format!(
            "live gives now_live {:?} now_dead {:?} drawn_in {:?}; the marks' difference now_live {:?} now_dead {:?}, unchanged files flipping {:?}",
            names(&d.now_live),
            names(&d.now_dead),
            d.drawn_in,
            names(&now_live),
            names(&now_dead),
            flipped
        )));
    }
    // The flips: the unchanged files' sections in the difference, and per
    // file of `cf` the `sh_index`es live on one side only.
    let mut flips = 0usize;
    for &s in now_live.iter().chain(&now_dead) {
        if u64::from(s.0) < base
            && !cf.files.iter().any(|c| c.file.0 == view.section(s).map_or(u32::MAX, |r| r.file))
        {
            flips += 1;
        }
    }
    for c in &cf.files {
        let r = view.file_index(c.file)?.ranges[FileTable::Sections as usize];
        let at = |v: &[SectionId], first: u32, count: u32| -> Vec<u32> {
            v.iter().filter(|s| s.0 >= first && s.0 - first < count).map(|s| s.0 - first).collect()
        };
        let was = at(&old, r.first, r.count);
        let count = u32::try_from(c.sections.len()).unwrap_or(u32::MAX);
        let is = at(&new, c.first_section.0, count);
        flips += was.iter().filter(|k| is.binary_search(k).is_err()).count();
        flips += is.iter().filter(|k| was.binary_search(k).is_err()).count();
    }
    changed::record(&format!(
        "gc-lift: live agrees with the front's mark: now_live {} now_dead {} drawn_in {}, {flips} flips",
        d.now_live.len(),
        d.now_dead.len(),
        d.drawn_in.len()
    ));
    Ok((d, flips))
}

/// The delta's placement of the changed files, judged ([`placement`]).
struct Placed {
    /// The changed sections the delta placed at another offset (or chunk)
    /// than the front's, as `name: update .., full ..` (empty: the full
    /// link's offsets, the identity case's requirement).
    differ: Vec<String>,
    /// The counts, for the findings file.
    summary: String,
    /// G-3 (WP-230): G-1's plan, when a changed file has debug sections.
    debug: Option<mortar_incr::update::debug::DebugPlan>,
}

/// U6-U7 for the changed files of `cf` (M2b-2b; IN-72): `layout_inputs`,
/// `scan_of`'s demands per file (its fall-back propagates by name), R-4a's
/// facts (`defined_facts`, packed into `delta.resolved`'s rows of the keys
/// the files define, and judged equal to the front's capture), then
/// `delta::layout` over them, judged against the front's placement
/// ([`judge`]). A disagreement is a FINDING: recorded with both answers
/// and the update falls back by name.
///
/// `live`: `live::live`'s delta, the update's one walk ([`live_judged`]'s,
/// judged against the front's mark; the empty delta without
/// `--gc-sections`). Under `--gc-sections` it gives each changed section's
/// `live` to `layout_inputs_live` and `scan_of` (F-8: the new side is the
/// walk's, never a stored bit). A delta that draws in an unchanged file
/// (a section of it flips, or its marked pieces change) is refused
/// `(layout)` by name: a drawn-in file's layout inputs (IN-72 item 2a; its
/// sections that die become holes, and its `.eh_frame` range loses the
/// dead FDEs) are not produced here (`layout_inputs` takes the files of
/// `cf` only), so the delta would keep its dead sections placed and its
/// stale records (WP-225 gc-3).
#[allow(clippy::too_many_arguments, reason = "the update's inputs, each read once")]
#[allow(clippy::too_many_lines, reason = "U6-U7's producers in order, then the judgement")]
fn placement(
    view: &StateView<'_>,
    stored: &mortar_core::incr::StoredLayout<'_>,
    f: &crate::front::Front<'_>,
    store: &Store,
    cwd: &Path,
    cf: &mortar_incr::update::view::ChangedFiles,
    mut delta: mortar_incr::update::resolve::ResolveDelta,
    live: &mortar_incr::update::live::LiveDelta,
    capture: &mortar_incr::update::testing::MemoryState,
    previous: &std::fs::File,
) -> Result<Placed, Why> {
    use mortar_core::incr::{FileDemands, LayoutInput};
    use mortar_core::resolution::SectionKind;
    use mortar_elf::SectionIndex;
    use mortar_merge::phase::ArchRelocs;
    use mortar_scan::ScanOptions;
    let options = mortar_resolve::tables::option_keys(&f.plan, &f.inputs.link.directives);
    if !live.drawn_in.is_empty() {
        changed::record(&format!(
            "M2b-2d: placement: live draws in {} unchanged files {:?} (now_live {} now_dead {}), \
             whose layout inputs are not produced: (layout)",
            live.drawn_in.len(),
            live.drawn_in,
            live.now_live.len(),
            live.now_dead.len()
        ));
        return Err(Why::Refused);
    }
    let mut cache = file_front::FrontCache::new(store, f.pool, cwd);
    let relocs = ArchRelocs(f.arch.get());
    let mut inputs = layout_in::layout_inputs_live(
        &mut cache,
        view,
        stored,
        &f.plan,
        cf,
        &delta,
        Some(live),
        &relocs,
        &f.pool,
    )
    .map_err(refused)?;
    let dynamic = crate::dynamic::arch_status(&f.plan, f.target.machine)
        .map_err(|_| refused(IncrReason::UpdateNotImplemented(Some(Unimplemented::Scan))))?
        == mortar_arch::Status::M2;
    let writable = crate::dynlink::OutWritable(&f.assignment);
    let opts = ScanOptions {
        output: crate::front::output_kind(f.plan.kind),
        dynamic,
        sections: if dynamic { Some(&writable) } else { None },
        proof: f.arch.proof(),
    };
    let scan = |e: file_scan::Fail| {
        changed::record(&format!("M2b-2b fallback: {} {}", e.0.report(), e.1));
        Why::Refused
    };
    let mut demands: Vec<FileDemands> = Vec::with_capacity(cf.files.len());
    let mut defined: Vec<(mortar_core::SymbolId, u32)> = Vec::new();
    for c in &cf.files {
        let ff = cache.get(c.file).ok_or_else(layout_in_lost).map_err(refused)?;
        let facts =
            file_split::changed_facts(ff, view, &f.plan, &delta.keepers).map_err(refused)?;
        let null = file_split::SectionFact {
            kind: SectionKind::Null,
            kept: false,
            out: None,
            reloc: SectionIndex(0),
        };
        let by_index = |i: SectionIndex| facts.get(i.0 as usize).copied().unwrap_or(null);
        let split = file_split::split_of(ff, view, &by_index, &relocs, &f.pool, Some(&delta))
            .map_err(refused)?;
        let d = file_scan::scan_of_why(
            f.arch.get(),
            opts,
            ff,
            Some(&facts),
            &split,
            view,
            cf,
            &options,
            &delta,
            live,
        )
        .map_err(scan)?;
        demands.push(d);
        let keys = file_scan::defined_facts(
            opts,
            ff,
            Some(&facts),
            &split,
            view,
            cf,
            &options,
            &delta,
            live,
        )
        .map_err(scan)?;
        for (id, r) in keys {
            defined.push((id, r.map_err(scan)?.pack()));
        }
    }
    demands.sort_by_key(|d| d.file);
    // R-4a: the facts of the keys the changed files define, into their
    // rows of the delta (the `facts: 0` of `defined_row` is never kept),
    // and judged against the front's capture (key by name).
    facts_into(view, capture, &mut delta, &defined).map_err(Why::Finding)?;
    let debug = cf.files.iter().any(|c| c.debug);
    debug_pieces(&inputs.changes, &mut inputs.pieces, debug);
    let input = LayoutInput {
        changes: &inputs.changes,
        eh: &inputs.eh,
        pieces: &inputs.pieces,
        demands: &demands,
        debug,
    };
    let mut d = mortar_layout::delta::layout(stored, &input, &f.target)
        .inspect_err(|e| {
            // With the front's answer for each change it does not place (the
            // other side of a refusal about a section the full link drops).
            let mut unplaced = String::new();
            for c in &inputs.changes {
                let fid = f.resolution.section_id(c.file, SectionIndex(c.index));
                let full = fid.and_then(|s| f.layout.placement.place.get(s)).and_then(|p| p.get());
                if let Some((fc, fo)) = full {
                    use std::fmt::Write as _;
                    let out = f.layout.placement.chunks[fc].kind.out();
                    let name = out.map(|o| String::from_utf8_lossy(f.assignment.outsecs[o].name));
                    let _ = write!(
                        unplaced,
                        " (placed {} size {} at {name:?}+{fo:#x})",
                        String::from_utf8_lossy(c.key.name),
                        c.size
                    );
                }
                if full.is_none() && c.index != 0 {
                    use std::fmt::Write as _;
                    let _ = write!(
                        unplaced,
                        " [{} size {} kept {} old {:?}]",
                        String::from_utf8_lossy(c.key.name),
                        c.size,
                        c.kept,
                        c.old
                    );
                }
            }
            // M2b2b-3: whether the stored layout has the changed files'
            // regions (no full link writes `REGIONS` yet).
            let regions: usize =
                cf.files.iter().map(|c| stored.regions_of(c.file).map_or(0, |r| r.len())).sum();
            changed::record(&format!(
                "M2b-2b fallback: delta::layout: {}; REGIONS rows of the changed files: \
                 {regions}; not placed by the front:{unplaced}",
                e.report()
            ));
        })
        .map_err(|_| Why::Refused)?;
    let debug = if input.debug {
        Some(debug_placed(view, stored, f, cf, previous, &mut d).map_err(Why::Finding)?)
    } else {
        None
    };
    let mut placed = judge(view, stored, f, &inputs, &d).map_err(|what| {
        Why::Finding(changed::finding_as(
            &format!("M2b-2b placement: {what}"),
            IncrReason::UpdateNotImplemented(Some(Unimplemented::Layout)),
        ))
    })?;
    placed.debug = debug;
    Ok(placed)
}

/// Why [`placement`] gave no placement: a producer refused (recorded by
/// name), or a FINDING (recorded with both answers).
enum Why {
    /// Its reason is in the findings file only (the identity check decides).
    Refused,
    Finding(IncrReason),
}

/// A producer's refusal, recorded.
#[allow(clippy::needless_pass_by_value, reason = "a `map_err` argument")]
fn refused(e: IncrReason) -> Why {
    changed::record(&format!("M2b-2b fallback: {}", e.report()));
    Why::Refused
}

fn layout_in_lost() -> IncrReason {
    IncrReason::StateCorrupt("the join: a changed file's front is not in the cache".into())
}

/// R-4a: each `(key, packed facts)` of `defined` written into the key's
/// row of `delta.resolved`; the front's capture's `RESOLVED.facts` of the
/// global of that name must be the same value (a FINDING otherwise), and
/// a key the files define must have a row in the delta.
fn facts_into(
    view: &StateView<'_>,
    capture: &mortar_incr::update::testing::MemoryState,
    delta: &mut mortar_incr::update::resolve::ResolveDelta,
    defined: &[(mortar_core::SymbolId, u32)],
) -> Result<(), IncrReason> {
    use mortar_incr::format::sym_kind;
    let corrupt = |what: String| IncrReason::StateCorrupt(format!("the join, R-4a: {what}"));
    let text = |s: mortar_incr::format::Str| -> Result<Vec<u8>, IncrReason> {
        let (a, b) = (s.off as usize, s.off as usize + s.len as usize);
        capture.strings.get(a..b).map(<[u8]>::to_vec).ok_or_else(|| corrupt("a string".into()))
    };
    let mut by_name: std::collections::HashMap<(Vec<u8>, Vec<u8>), usize> =
        std::collections::HashMap::new();
    for (k, s) in capture.symbols.iter().enumerate() {
        if s.kind != sym_kind::LOCAL {
            by_name.entry((text(s.name)?, text(s.version)?)).or_insert(k);
        }
    }
    let symbols = view.count(kind::SYMBOLS);
    for &(id, packed) in defined {
        let name = if u64::from(id.0) < symbols {
            let s = view.symbol(id)?;
            (view.string(s.name)?, view.string(s.version)?)
        } else {
            let n = delta
                .new_keys
                .get((u64::from(id.0) - symbols) as usize)
                .ok_or_else(|| corrupt(format!("key {} is no row of the update", id.0)))?;
            (n.name.clone(), n.version.clone())
        };
        let show = String::from_utf8_lossy(&name.0).into_owned();
        let front = by_name.get(&name).and_then(|&k| capture.resolved.get(k)).map(|r| r.facts);
        if front != Some(packed) {
            return Err(changed::finding_as(
                &format!("R-4a: key {show}: defined_facts {packed:#x}, the front's {front:x?}"),
                IncrReason::UpdateNotImplemented(Some(Unimplemented::Scan)),
            ));
        }
        let row = delta.resolved.iter_mut().find(|(k, _)| *k == id).ok_or_else(|| {
            changed::finding_as(
                &format!("R-4a: key {show} the files define has no row in the delta"),
                IncrReason::UpdateNotImplemented(Some(Unimplemented::Scan)),
            )
        })?;
        row.1.facts = packed;
    }
    Ok(())
}

/// M2b2a-2's properties (ruling M2b2a-3), per section of each changed file
/// (the update's and the front's matched by `(file, index)`: the front's
/// file of a changed id is the state's, `reference_of`'s U4 check):
/// placed by the delta exactly when the front places it; in an output
/// section of the same NAME; at an address its alignment admits;
/// overlapping no other placed section of its chunk (the stored places of
/// the other files' sections, the update's); every merged piece's
/// fragment in the stored `FRAGS` or the delta's new ones. `Err` is the
/// disagreement, both answers in the text.
#[allow(clippy::too_many_lines, reason = "M2b2a-2's properties, one after the other")]
fn judge(
    view: &StateView<'_>,
    stored: &mortar_core::incr::StoredLayout<'_>,
    f: &crate::front::Front<'_>,
    inputs: &layout_in::LayoutInputs<'_>,
    d: &mortar_core::incr::LayoutDelta,
) -> Result<Placed, String> {
    use mortar_core::incr::PlaceNow;
    use mortar_elf::SectionIndex;
    let e = |x: IncrReason| x.report();
    let outsecs = view.outsecs().map_err(e)?;
    let mut differ = Vec::new();
    let mut mine: Vec<(u32, u64, u64)> = Vec::new();
    let mut files: Vec<FileId> = Vec::new();
    for c in &inputs.changes {
        files.push(c.file);
        let name = String::from_utf8_lossy(c.key.name).into_owned();
        let fid = f
            .resolution
            .section_id(c.file, SectionIndex(c.index))
            .ok_or_else(|| format!("section {} {name}: no section of the front", c.index))?;
        let full = f.layout.placement.place.get(fid).and_then(|p| p.get());
        let now = match d.place_of(stored, c.id).map_err(e)? {
            PlaceNow::Here(p) => p.get(),
            PlaceNow::NotPlaced => None,
            PlaceNow::Stored(_) => {
                return Err(format!("{name}: a changed file's section is Stored"));
            }
        };
        let (Some((uc, uo)), Some((fc, fo))) = (now, full) else {
            if now.is_some() != full.is_some() {
                return Err(format!(
                    "section {} {name}: placed by the update {now:?}, by the full link {full:?}",
                    c.index
                ));
            }
            continue;
        };
        let uchunk =
            d.placement.chunks.get(uc).ok_or_else(|| format!("{name}: no chunk {uc:?}"))?;
        let fchunk = &f.layout.placement.chunks[fc];
        let uout = match uchunk.kind.out() {
            Some(o) => {
                let row = outsecs.get(o.0 as usize).ok_or_else(|| format!("{name}: OUTSECS"))?;
                Some(view.string(row.name).map_err(e)?)
            }
            None => None,
        };
        let fout = fchunk.kind.out().map(|o| f.assignment.outsecs[o].name.to_vec());
        if uout != fout {
            return Err(format!(
                "section {} {name}: output section {:?} in the update, {:?} in the full link",
                c.index,
                uout.map(|n| String::from_utf8_lossy(&n).into_owned()),
                fout.map(|n| String::from_utf8_lossy(&n).into_owned())
            ));
        }
        let addr = uchunk.addr.unwrap_or(0) + uo;
        if !addr.is_multiple_of(c.align.max(1)) {
            return Err(format!("{name}: at {addr:#x}, not aligned to {}", c.align));
        }
        mine.push((uc.0, uo, uo + c.size));
        if uo != fo || uchunk.addr != fchunk.addr {
            differ.push(format!("{name}: update {uo:#x}, full {fo:#x}"));
        }
    }
    files.sort_unstable();
    files.dedup();
    // No overlap within a chunk: the other files' stored places in the
    // chunks the update placed into.
    let chunks: std::collections::BTreeSet<u32> = mine.iter().map(|m| m.0).collect();
    let count = u32::try_from(view.count(kind::FILES)).map_err(|_| "FILES".to_string())?;
    let mut others: Vec<(u32, u64, u64)> = Vec::new();
    for g in (1..count).map(FileId).filter(|g| files.binary_search(g).is_err()) {
        for s in stored.sections_of(g).map_err(e)? {
            if let PlaceNow::Stored(p) = d.place_of(stored, s.id).map_err(e)?
                && let Some((ch, off)) = p.get()
                && chunks.contains(&ch.0)
            {
                others.push((ch.0, off, off + s.size));
            }
        }
    }
    others.sort_unstable();
    for (k, a) in mine.iter().enumerate() {
        let empty = |x: &(u32, u64, u64)| x.1 >= x.2;
        let hit = others
            .iter()
            .chain(&mine[k + 1..])
            .find(|b| a.0 == b.0 && a.1 < b.2 && b.1 < a.2 && !empty(a) && !empty(b));
        if let Some(b) = hit {
            return Err(format!(
                "chunk {} [{:#x}, {:#x}) overlaps [{:#x}, {:#x})",
                a.0, a.1, a.2, b.1, b.2
            ));
        }
    }
    let mut pieces = 0usize;
    for p in inputs.pieces.iter().flat_map(|p| &p.pieces) {
        let key = (p.out, p.class.word(), p.hash);
        let found = stored.frags_of(key.0, key.1, key.2).map_err(e)?.is_some()
            || d.frags.iter().any(|g| (g.out, g.class, g.hash) == key);
        if !found {
            return Err(format!("piece {p:?}: no fragment, stored or new"));
        }
        pieces += 1;
    }
    let summary = format!(
        "{} changes, {} placed, {} not at the full link's offset, {pieces} pieces, {} moved, {} growth steps",
        inputs.changes.len(),
        mine.len(),
        differ.len(),
        d.moved.len(),
        d.growth_steps.len()
    );
    Ok(Placed { differ, summary, debug: None })
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

/// IN-70's debug condition, at the end of U2 (G-3, WP-230): IN-55 and
/// IN-55a on each changed file, its old version by its stored `SECTIONS`
/// rows (`split::classify_rows`: names, flags, COMDAT membership) and its
/// new version by its bytes (`split::classify_object`: names and unit
/// headers): `debug-name-index` or `debug-unit-kind` by name, before
/// anything is read past the change detection. Every other debug
/// contribution goes on to the update: G-1's placement binds and is judged
/// against the front (`debug_placed`, `debug_judged`), and anything but the
/// identity case falls back by the identity check's name. A new version
/// that does not parse is left to the front, whose object
/// `debug_placed` classifies again.
fn debug_condition(view: &StateView<'_>, d: &Detected, cwd: &Path) -> Result<(), IncrReason> {
    use mortar_incr::update::split;
    let named = |f: FileId, e: IncrReason| {
        changed::record(&format!("G-3 fallback: file {}: {}", f.0, e.report()));
        e
    };
    for &f in &d.changed_files {
        let rows = view.sections_of(f)?;
        split::classify_rows(view, &mortar_incr::update::view::ChangedFiles::default(), &rows)
            .map_err(|e| named(f, e))?;
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
            split::classify_object(&object).map_err(|e| named(f, e))?;
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

// ---- G-3 (WP-230): the debug side of the update ------------------------------

/// The reason G-3 falls back with, recorded first (`G-3 fallback: …`).
fn debug_fallback(what: &str, e: IncrReason) -> IncrReason {
    changed::record(&format!("G-3 fallback: {what}: {}", e.report()));
    e
}

/// A disagreement of G-1 with the front: a FINDING, recorded with both
/// answers; the update falls back `update-not-implemented (debug)`.
fn debug_finding(what: &str) -> IncrReason {
    changed::finding_as(
        &format!("G-3 debug: {what}"),
        IncrReason::UpdateNotImplemented(Some(Unimplemented::Debug)),
    )
}

/// The previous output's bytes as `debug::strings` reads them: a buffer of
/// the output's offsets holding the merged debug string sections (the
/// stored chunks of `.debug_str` and `.debug_line_str`, read through U1's
/// descriptor `previous`) and zeros elsewhere; `debug::strings` reads
/// nothing else of it (IN-56, the string and its NUL at a hit's offset).
/// The zeros are an untouched zeroed allocation: the resident cost is the
/// two sections.
fn debug_string_bytes(
    stored: &mortar_core::incr::StoredLayout<'_>,
    previous: &std::fs::File,
) -> Result<Vec<u8>, IncrReason> {
    use mortar_incr::update::debug::str_class;
    let names: std::collections::BTreeMap<_, _> = stored.outsecs()?.into_iter().collect();
    let mut ranges = Vec::new();
    for (_, c) in stored.chunks()? {
        if c.kind.out().and_then(|o| names.get(&o)).is_some_and(|n| str_class(n).is_some()) {
            ranges.push((c.offset, c.size));
        }
    }
    let end = ranges.iter().map(|&(o, s)| o + s).max().unwrap_or(0);
    let mut bytes = vec![0u8; usize::try_from(end).map_err(|_| debug_reason())?];
    for (o, s) in ranges {
        let at = usize::try_from(o).map_err(|_| debug_reason())?;
        let len = usize::try_from(s).map_err(|_| debug_reason())?;
        std::os::unix::fs::FileExt::read_exact_at(previous, &mut bytes[at..at + len], o)
            .map_err(|e| IncrReason::OutputChanged(e.to_string()))?;
    }
    Ok(bytes)
}

fn debug_reason() -> IncrReason {
    IncrReason::UpdateNotImplemented(Some(Unimplemented::Debug))
}

/// G-3, U7 (WP-230; IN-50..IN-57): G-1's steps on the delta `d` that
/// `delta::layout` gave with `LayoutInput::debug`, in G-1's order:
/// IN-55/IN-55a on the front's object of each changed file with debug
/// sections (the bytes the update judges), `debug::strings` over their
/// `.debug_str`/`.debug_line_str` input sections against the previous
/// output, `debug::place` into `d`. Then the changed files'
/// `.debug_aranges` input sections (IN-54: rewritten whole, never placed by
/// `place`) get their place in the rewrite: the stored one when the size is
/// the stored size (the identity case); another size is `(debug)` by name
/// (the place in a whole rewrite with other sizes is the wiring's).
///
/// Every `Err` binds (the update falls back by its name): G-1 has not been
/// judged in a case it refuses.
fn debug_placed(
    view: &StateView<'_>,
    stored: &mortar_core::incr::StoredLayout<'_>,
    f: &crate::front::Front<'_>,
    cf: &mortar_incr::update::view::ChangedFiles,
    previous: &std::fs::File,
    d: &mut mortar_core::incr::LayoutDelta,
) -> Result<mortar_incr::update::debug::DebugPlan, IncrReason> {
    use mortar_core::inputs::Reader;
    use mortar_core::layout::Place;
    use mortar_elf::consts::SHF_ALLOC;
    use mortar_incr::update::{debug, split};
    let mut inputs: Vec<(FileId, u8, &[u8])> = Vec::new();
    for c in cf.files.iter().filter(|c| c.debug) {
        let Some(Reader::Object(o)) = f.inputs.files.get(c.file).map(|x| &x.reader) else {
            return Err(debug_fallback(
                &format!("file {}: no object of the front", c.file.0),
                debug_reason(),
            ));
        };
        split::classify_object(o).map_err(|e| debug_fallback(&format!("file {}", c.file.0), e))?;
        for i in 1..o.section_count() {
            let Ok(s) = o.section(mortar_elf::SectionIndex(i)) else { continue };
            let Ok(name) = s.name() else { continue };
            if s.flags() & SHF_ALLOC != 0 {
                continue;
            }
            if let Some(class) = debug::str_class(name) {
                let data = s.data().map_err(|_| debug_reason())?;
                inputs.push((c.file, class, data));
            }
        }
    }
    let prev = debug_string_bytes(stored, previous)?;
    let strings = debug::strings(view, stored, &inputs, &prev)
        .map_err(|e| debug_fallback("debug::strings", e))?;
    let plan = debug::place(view, stored, cf, strings, d)
        .map_err(|e| debug_fallback("debug::place", e))?;
    // IN-54's input sections: their place in the whole rewrite.
    for c in &cf.files {
        let olds: Vec<_> = stored
            .sections_of(c.file)?
            .into_iter()
            .filter(|s| s.name == b".debug_aranges" && s.sh_flags & SHF_ALLOC == 0)
            .collect();
        let mut nth = 0usize;
        for (k, s) in c.sections.iter().enumerate() {
            if s.sh_flags & SHF_ALLOC != 0 || cf.string(view, s.name)? != b".debug_aranges" {
                continue;
            }
            let old = olds.get(nth);
            nth += 1;
            let Some(old) = old.filter(|o| o.size == s.size && o.place.get().is_some()) else {
                return Err(debug_fallback(
                    &format!(
                        "file {}: a .debug_aranges contribution of {} bytes, stored {:?}",
                        c.file.0,
                        s.size,
                        old.map(|o| o.size)
                    ),
                    debug_reason(),
                ));
            };
            let id = mortar_core::SectionId(c.first_section.0 + k as u32);
            while d.placement.place.len() <= id.0 {
                d.placement.place.push(Place::NONE).map_err(|_| debug_reason())?;
            }
            d.placement.place[id] = old.place;
        }
    }
    changed::record(&format!(
        "G-3 debug: placed: {} contributions, {} strings found, {} appended, {} touched chunks",
        plan.contributions.len(),
        plan.strings.hits,
        plan.strings.misses,
        plan.chunks.len()
    ));
    Ok(plan)
}

/// G-3, U9 (WP-230): in the identity case the bytes the update writes are
/// the front's (the full link's of the new inputs with the stored
/// placement), so G-1's decisions are judged against them before W1:
/// every contribution in place and no hole made (IN-50 step 1); no string
/// appended and every string of a changed file at the front's offset
/// (IN-56); `debug::aranges` of the previous `.debug_aranges` with the
/// changed files' new sets equal, byte for byte, to the new output's
/// section (IN-54); each kept group copy left in place equal to the
/// previous output's bytes at its old place (R-5 a, else
/// `debug-comdat-keeper`). A disagreement is a FINDING (`debug_finding`).
/// A changed file with debug sections and no `DebugPlan` (G-1 did not
/// run: a producer before `delta::layout` refused, or U5 gave no delta)
/// is `update-not-implemented (debug)` by name.
#[allow(clippy::too_many_lines, reason = "IN-50, IN-56, IN-54, R-5 a in order")]
#[allow(clippy::many_single_char_names, reason = "the front, a range's ends, a piece")]
fn debug_judged(
    view: &StateView<'_>,
    stored: &mortar_core::incr::StoredLayout<'_>,
    f: &crate::front::Front<'_>,
    files: &[FileId],
    plan: Option<&mortar_incr::update::debug::DebugPlan>,
    refill: &mortar_image::update::Refill,
    previous: &std::fs::File,
) -> Result<(), IncrReason> {
    use mortar_elf::SectionIndex;
    use mortar_elf::consts::SHF_ALLOC;
    use mortar_incr::update::debug::{self, Taken};
    let mut has_debug = false;
    for &file in files {
        for r in view.sections_of(file)? {
            has_debug |=
                r.sh_flags & SHF_ALLOC == 0 && view.string(r.name)?.starts_with(b".debug_");
        }
    }
    if !has_debug {
        return Ok(());
    }
    let Some(plan) = plan else {
        return Err(debug_fallback(
            "a changed file with debug sections, and G-1 did not run (no delta placement)",
            debug_reason(),
        ));
    };
    // IN-50 step 1.
    for c in &plan.contributions {
        if c.taken != Taken::InPlace {
            return Err(debug_finding(&format!(
                "the identity case, but file {} chunk {} took {:?}",
                c.file.0, c.chunk.0, c.taken
            )));
        }
    }
    if let Some((chunk, holes)) = plan.chunks.first() {
        return Err(debug_finding(&format!(
            "the identity case, but chunk {} has new holes {holes:?}",
            chunk.0
        )));
    }
    // IN-56.
    if let Some((class, (_, at, bytes))) = plan.strings.appended.iter().next() {
        return Err(debug_finding(&format!(
            "the identity case, but {} bytes of class {class} strings appended at {at:#x}",
            bytes.len()
        )));
    }
    let split = &f.merge.split;
    let mut strings = 0usize;
    for &file in files {
        for (id, name, _) in front_sections(f, file) {
            let Some(class) = debug::str_class(name) else { continue };
            let Ok(row) = split.merge_of.binary_search_by_key(&id, |m| m.section) else {
                return Err(debug_finding(&format!(
                    "file {} class {class}: a string section the front did not split",
                    file.0
                )));
            };
            let span = split.merge_of[row].pieces;
            for p in span.start..span.start + span.len {
                let (off, frag) = (split.piece_off[p as usize], split.piece_frag[p as usize]);
                let full = f.layout.merge.offset(frag);
                let mine = plan.strings.output_offset(file, class, u64::from(off));
                if full != mine {
                    return Err(debug_finding(&format!(
                        "file {} class {class} string at input {off:#x}: G-1 {mine:?}, the front {full:?}",
                        file.0
                    )));
                }
                strings += 1;
            }
        }
    }
    // The new output's bytes of `range`: the previous output's with the
    // refill's ranges over them.
    let new_bytes = |lo: u64, len: u64| -> Result<Vec<u8>, IncrReason> {
        let mut b = vec![0u8; usize::try_from(len).map_err(|_| debug_reason())?];
        std::os::unix::fs::FileExt::read_exact_at(previous, &mut b, lo)
            .map_err(|e| IncrReason::OutputChanged(e.to_string()))?;
        let mut at = 0u64;
        for r in &refill.ranges {
            let (s, e) = (r.offset.max(lo), (r.offset + r.len).min(lo + len));
            if s < e {
                let from = usize::try_from(at + (s - r.offset)).map_err(|_| debug_reason())?;
                let to = usize::try_from(s - lo).map_err(|_| debug_reason())?;
                let n = usize::try_from(e - s).map_err(|_| debug_reason())?;
                b[to..to + n].copy_from_slice(&refill.bytes[from..from + n]);
            }
            at += r.len;
        }
        Ok(b)
    };
    // IN-54.
    let pl = &f.layout.placement;
    let chunk_named = |want: &[u8]| {
        pl.chunks.iter().find(|(_, c)| {
            !c.is_alloc() && c.kind.out().is_some_and(|o| f.assignment.outsecs[o].name == want)
        })
    };
    let mut aranges = 0usize;
    if !plan.aranges.is_empty() && chunk_named(b".debug_aranges").is_none() {
        // No `.debug_aranges` in the full link (DWARF 5 producers that
        // emit none, kiln's): G-1 still lists each changed file's
        // `.debug_info` range; its rewrite of nothing must be nothing
        // (the identity check has already equated the stored chunks).
        let replaced: Vec<(u64, u64, Vec<u8>)> =
            plan.aranges.iter().map(|&(_, s, e)| (s, e, Vec::new())).collect();
        let mine =
            debug::aranges(&[], &replaced).map_err(|e| debug_fallback("debug::aranges", e))?;
        if !mine.is_empty() {
            return Err(debug_finding(&format!(
                ".debug_aranges: none in the front, and G-1's rewrite is {} bytes",
                mine.len()
            )));
        }
    } else if !plan.aranges.is_empty() {
        let Some((ar, chunk)) = chunk_named(b".debug_aranges") else {
            return Err(debug_finding("sets to replace, and no .debug_aranges in the front"));
        };
        let old = stored
            .chunks()?
            .into_iter()
            .find(|(id, _)| *id == ar)
            .map(|(_, c)| c)
            .ok_or_else(|| debug_finding("no stored .debug_aranges chunk"))?;
        let mut prev = vec![0u8; usize::try_from(old.size).map_err(|_| debug_reason())?];
        std::os::unix::fs::FileExt::read_exact_at(previous, &mut prev, old.offset)
            .map_err(|e| IncrReason::OutputChanged(e.to_string()))?;
        let now = new_bytes(chunk.offset, chunk.size)?;
        let mut replaced = Vec::new();
        for &(file, s, e) in &plan.aranges {
            let mut sets = Vec::new();
            for (id, name, size) in front_sections(f, file) {
                if name != b".debug_aranges" {
                    continue;
                }
                if let Some((c, at)) = pl.place.get(id).and_then(|p| p.get())
                    && c == ar
                {
                    let at = usize::try_from(at).map_err(|_| debug_reason())?;
                    let size = usize::try_from(size).map_err(|_| debug_reason())?;
                    sets.extend_from_slice(now.get(at..at + size).unwrap_or_default());
                }
            }
            replaced.push((s, e, sets));
        }
        let mine =
            debug::aranges(&prev, &replaced).map_err(|e| debug_fallback("debug::aranges", e))?;
        if mine != now {
            let at = mine.iter().zip(&now).position(|(a, b)| a != b);
            return Err(debug_finding(&format!(
                ".debug_aranges: G-1's rewrite differs from the front's at {at:?} (sizes {} and {})",
                mine.len(),
                now.len()
            )));
        }
        aranges = now.len();
    }
    // R-5 (a).
    for &(new, old) in &plan.comdat_in_place {
        let Some(o) = stored.section(old)? else { continue };
        let (Some((oc, oo)), Some(fid)) =
            (o.place.get(), f.resolution.section_id(o.file, SectionIndex(o.index)))
        else {
            continue;
        };
        let Some((fc, fo)) = pl.place.get(fid).and_then(|p| p.get()) else { continue };
        let old_chunk = stored.chunks()?.into_iter().find(|(id, _)| *id == oc).map(|(_, c)| c);
        let Some(old_chunk) = old_chunk else { continue };
        let mut was = vec![0u8; usize::try_from(o.size).map_err(|_| debug_reason())?];
        std::os::unix::fs::FileExt::read_exact_at(previous, &mut was, old_chunk.offset + oo)
            .map_err(|e| IncrReason::OutputChanged(e.to_string()))?;
        let is = new_bytes(pl.chunks[fc].offset + fo, o.size)?;
        if was != is {
            return Err(debug_fallback(
                &format!("kept group copy {} (old {})", new.0, old.0),
                IncrReason::DebugComdatKeeper(format!(
                    "the kept copy's bytes at {:#x} change",
                    old_chunk.offset + oo
                )),
            ));
        }
    }
    changed::record(&format!(
        "G-3 debug: judged: {} contributions in place, {strings} strings at the front's offsets, \
         .debug_aranges {aranges} bytes equal, {} group copies",
        plan.contributions.len(),
        plan.comdat_in_place.len()
    ));
    Ok(())
}

/// The non-allocated sections of `file`'s object in the front: id, name,
/// size (nothing for a file that is not an object).
fn front_sections<'f>(
    f: &'f crate::front::Front<'_>,
    file: FileId,
) -> Vec<(mortar_core::SectionId, &'f [u8], u64)> {
    use mortar_core::inputs::Reader;
    use mortar_elf::SectionIndex;
    use mortar_elf::consts::SHF_ALLOC;
    let Some(Reader::Object(o)) = f.inputs.files.get(file).map(|x| &x.reader) else {
        return Vec::new();
    };
    let mut out = Vec::new();
    for i in 1..o.section_count() {
        let (Ok(s), Some(id)) =
            (o.section(SectionIndex(i)), f.resolution.section_id(file, SectionIndex(i)))
        else {
            continue;
        };
        let Ok(name) = s.name() else { continue };
        if s.flags() & SHF_ALLOC == 0 {
            out.push((id, name, s.size()));
        }
    }
    out
}

/// G-3 (WP-230): the merged pieces `delta::layout` places and `judge`
/// finds (IN-72 item 6), without those of a changed `.debug_*` section
/// when the debug placement runs: their strings are `debug::strings`'
/// (IN-52), judged by [`debug_judged`]; R-1: no `.debug_*` fragment exists
/// (item 6 refuses such a piece). Untouched without a debug section.
fn debug_pieces(
    changes: &[mortar_core::incr::SectionChange<'_>],
    pieces: &mut [mortar_core::incr::FilePieces],
    debug: bool,
) {
    if !debug {
        return;
    }
    let is_debug = |id: mortar_core::SectionId| {
        changes.binary_search_by_key(&id, |c| c.id).ok().is_some_and(|k| {
            let n = changes[k].key.name;
            n.starts_with(b".debug_") || n.starts_with(b".zdebug")
        })
    };
    for fp in pieces {
        fp.pieces.retain(|p| !is_debug(p.section));
    }
}
