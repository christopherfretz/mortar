//! Incremental mode in the facade (`spec/incr.md` §5, WP-061 stage A):
//! the checks before a link (IN-12's unsupported modes, the stamps of
//! IN-22a, the existing state read and validated, the fall-back reason of
//! §7), and after a successful full link the state written beside the
//! output and bound to it (IN-17, IN-18a, IN-21).
//!
//! **WP-225 (the join).** The state at `S` is format 5 (`MORTARSR`,
//! `mortar_incr::state5`); a format-4 state there is `state-version`.
//! [`start`] is IN-70's U1 on it (the lock, the updating flag before any
//! hash, `state5::open` with the plan hash, `META`'s mortar version, the
//! output's binding, `boot_check`, `small-output`); a state that passes
//! is [`Start::Update`], whose steps from U2 on are
//! `crate::update::run`'s. Until those are complete every update falls
//! back by name (`update-not-implemented (<word>)`) before its first
//! write. Format 4 is written only by a testhooks build
//! (`MORTAR_INCR_STATE4=1`, at `<S>.format4`) for the test floor and the
//! dry run of `--incremental-report` (until WP-225 M6).
//!
//! A plain link never reaches this module (IN-11): [`start`] returns
//! [`Start::Plain`] before it touches anything when `--incremental` is
//! not given.

use std::ffi::OsStr;
use std::fmt::Write as _;
use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::time::Instant;

use mortar_cli::Parsed;
use mortar_cli::table::{Lands, Status};
use mortar_core::id::FileId;
use mortar_core::incr::IncrReason;
use mortar_core::plan::{Compression, Icf, IncrementalMode, InputSpec, OutputKind, ScriptRole};
use mortar_diag::{Collector, Diagnostic, Key, Kind};
use mortar_digest::{L128, TreeFn};
use mortar_image::link::Written;
use mortar_incr::capture::{Keys, Link};
use mortar_incr::format::{self, Meta, StatRow, State, Strings, stat_tag};
use mortar_incr::stamp::{self, DeviceStamp, Tuple};
use mortar_input::store::Store;

use crate::front::Front;

/// The phase of the incremental checks (`spec/model.md` §8).
const PHASE: u16 = 15;

/// What [`start`] decided.
#[derive(Debug)]
pub enum Start {
    /// A plain link: nothing of incremental mode happens.
    Plain,
    /// A full link in incremental mode, which writes a state at the end.
    Full(Box<Session>),
    /// A format-5 state passed every check of U1 (IN-70): the update runs
    /// (`crate::update::run`); a fall-back turns the session into a full
    /// link ([`Session::fall_back`]) or, under `--incremental=update`, an
    /// error.
    Update(Box<Session>),
    /// `--incremental=update` cannot update: the error is emitted and
    /// nothing is linked or written.
    Failed,
}

/// Where `--incremental-report` goes.
#[derive(Debug)]
enum Report {
    Stderr,
    File(PathBuf),
}

/// A full link in incremental mode, between [`start`] and
/// [`Session::finish`].
#[derive(Debug)]
pub struct Session {
    state_path: PathBuf,
    report: Option<Report>,
    plan_hash: [u8; 16],
    stamps: Vec<DeviceStamp>,
    /// The fall-back reason, with its detail (§7); `None` for
    /// `--incremental=full`, printed `full requested` (WP-213: the one
    /// reason type, `mortar_core::incr::IncrReason`).
    reason: Option<IncrReason>,
    /// The state's target (`META` machine and class), when it was read.
    recorded_target: Option<(u16, u8)>,
    /// The state read, kept for the dry run of `--incremental-report`
    /// (WP-063) when it passed every check; `None` otherwise, so that a
    /// link without the report keeps nothing.
    old: Option<Box<State>>,
    start: Instant,
    /// WP-094: the parts of the state's work, timed (ms), for
    /// `--time-phases` lines.
    times: Times,
    /// IN-77b: `S`'s descriptor, locked, held to the state's write (its
    /// drop with the session releases the lock); `None` when no state was
    /// found or it could not be locked.
    lock: Option<std::fs::File>,
    /// IN-77b: `state-busy` or `lock-unavailable`: this link writes no
    /// state (and, for `lock-unavailable`, removes the one at `S`).
    writes_no_state: Option<&'static str>,
    /// WP-244's switch, kept for tests after the flip (WP-225): the
    /// format-5 state also written to its path, its rows to its sink.
    state5: Option<State5>,
    /// WP-225: the format-4 side state (`<state>.format4`) is also written,
    /// for the test floor and the dry run (`mortar_incr::testhook::state4`
    /// or [`crate::invoke_state4`]; deleted at M6).
    state4: bool,
    state4_path: Option<PathBuf>,
    /// WP-225: the pre-flip layout for the format-4 replay's tests
    /// ([`crate::replay_link`], [`crate::invoke_legacy4`]): no format-5
    /// data (so no IN-34a pads), only the format-4 side state written.
    legacy4: bool,
    /// `--incremental=update`: a fall-back is an error.
    update_mode: bool,
    /// IN-77a: the output `P`, opened once by U1 (the update's
    /// descriptor); `None` for a full link.
    pub(crate) out: Option<std::fs::File>,
}

/// WP-244's switch (until the join, U-10, writes format 5 as THE state):
/// the format-5 state of the link is also captured and written to
/// `path`; `collide` injects a `FRAGS` collision (`state-collision`).
/// Set from `mortar_incr::testhook::{state5_path, state5_collide}` (a
/// `testhooks` build) or by [`crate::invoke_state5`] (tests).
#[derive(Debug, Clone)]
pub struct State5 {
    pub path: PathBuf,
    pub collide: bool,
    /// Where the state's rows also go, normalized, before encoding (tests:
    /// seam 6's columns that format 5's bytes do not hold until U-2
    /// encodes them). `None`: nowhere.
    pub rows: Option<Rows5Sink>,
}

/// [`State5::rows`]: the last capture's normalized rows.
pub type Rows5Sink =
    std::sync::Arc<std::sync::Mutex<Option<mortar_incr::update::testing::MemoryState>>>;

/// WP-094: timed parts (name, ms), shared with the caller.
pub type Times = std::rc::Rc<std::cell::RefCell<Vec<(&'static str, f64)>>>;

/// WP-094: `(name, ms since t)`, and `t` moved to now.
fn lap(times: &Times, t: &mut Instant, name: &'static str) {
    let now = Instant::now();
    times.borrow_mut().push((name, (now - *t).as_secs_f64() * 1e3));
    *t = now;
}

/// The option of IN-12 incremental mode does not support, if any, from
/// the plan (the output kind keys the dynamic outputs until WP-066).
fn unsupported(parsed: &Parsed) -> Option<String> {
    let plan = &parsed.plan;
    let s = &plan.shaping;
    let kind = match plan.kind {
        OutputKind::StaticExecutable => None,
        OutputKind::Executable => Some("a dynamic executable (no -static)"),
        OutputKind::PieExecutable => Some("-pie"),
        OutputKind::StaticPie => Some("-static-pie"),
        OutputKind::Shared => Some("-shared"),
        OutputKind::Relocatable => Some("-r"),
    };
    if let Some(k) = kind {
        return Some(k.to_owned());
    }
    if s.emit_relocs {
        return Some("--emit-relocs".to_owned());
    }
    // meta.md MT-32 (WP-076): the kept units are not part of the state.
    if plan.diagnostics.odr.keep_meta {
        return Some("--keep-mason-meta".to_owned());
    }
    if s.icf.is_some_and(|i| i != Icf::None) {
        return Some("--icf".to_owned());
    }
    if s.compress_debug_sections.is_some_and(|c| c != Compression::None) {
        return Some("--compress-debug-sections".to_owned());
    }
    if s.gdb_index == Some(true) {
        return Some("--gdb-index".to_owned());
    }
    // layout.md L-88a, L-53a (WP-111): the in-place update keeps each
    // input's place and does not re-sort; a sorted link is a plain one.
    if s.sort_common.is_some() {
        return Some("--sort-common".to_owned());
    }
    if s.sort_section.is_some() {
        return Some("--sort-section".to_owned());
    }
    // A script given by -T may hold SECTIONS; which it does is known only
    // once it is read, so every -T is unsupported here (conservative).
    if plan.scripts.iter().any(|x| x.role == ScriptRole::Script) {
        return Some("-T (a linker script)".to_owned());
    }
    // IN-71b (WP-212 F-9): a `--start-lib` region's lazy objects are not
    // archive members of the state.
    if plan.inputs.iter().any(|i| i.state.lib.is_some()) {
        return Some("--start-lib".to_owned());
    }
    None
}

/// The normalized plan's hash (IN-18b).
#[must_use]
pub fn plan_hash(parsed: &Parsed, cwd: &Path) -> [u8; 16] {
    let mut items: Vec<(u32, u8, Vec<Vec<u8>>)> = Vec::new();
    for u in &parsed.uses {
        let row = u.row();
        let excluded = matches!(
            row.lands,
            Lands::Map
                | Lands::PrintMap
                | Lands::MortarMap
                | Lands::TimePhases
                | Lands::Threads
                | Lands::ThreadCount
                | Lands::NoThreads
                | Lands::DependencyFile
                | Lands::Incremental
                | Lands::NoIncremental
                | Lands::IncrementalState
                | Lands::IncrementalReport
        ) || row.name().starts_with("--incremental")
            // IN-18b: a row the table accepts and ignores (`noop`) touches
            // no output byte (its audit links it to the same bytes), and
            // some carry a new value on every run: gcc's
            // `-plugin-opt=-fresolution=/tmp/ccXXXXXX.res`.
            || matches!(row.status, Status::Noop(_));
        if excluded {
            continue;
        }
        let value = u.value.as_deref().map_or(&[][..], OsStr::as_encoded_bytes);
        items.push((u.arg, b'O', vec![row.name().into_bytes(), value.to_vec()]));
    }
    for input in &parsed.plan.inputs {
        if let InputSpec::Path(p) = &input.spec {
            items.push((input.arg, b'I', vec![p.as_encoded_bytes().to_vec()]));
        }
    }
    items.sort_by_key(|i| i.0);
    let mut h = L128::new();
    let mut item = |tag: u8, fields: &[Vec<u8>]| {
        h.update(&[tag]);
        for f in fields {
            h.update(&(f.len() as u32).to_le_bytes());
            h.update(f);
        }
    };
    item(b'C', &[cwd.as_os_str().as_encoded_bytes().to_vec()]);
    for (_, tag, fields) in &items {
        item(*tag, fields);
    }
    h.finish()
}

fn emit(diagnostics: &Collector, seq: u64, d: Diagnostic) {
    diagnostics.emit(Key::new(PHASE, 0, seq), d);
}

/// WP-238 P0-7, the Rust oracle of the stored liveness rows, judging the
/// bytes the reader will read (P3-1a): `m` is a state as DECODED from its
/// bytes (`state5::read::decode`; the capture passes the reader's own
/// view of the bytes it is about to write). The stored-form walk
/// (`mortar_incr::update::live::walk_stored`) over it, with the link's
/// `OptionKeys`, compared with `f`'s full mark on sections AND live
/// fragments through the reader's numbering (a link section is its file's
/// first `SECTIONS` row plus its `sh_index`, as `state5::read` numbers a
/// file's rows), every section it marks checked KEPT (P0-2), and its hash
/// with the stored `META.live_hash` (S4); a zero stored hash with a live
/// section is a difference. `Ok`: the `incremental: live-check` line
/// (`agree`, or `refused <rule>: <detail>` for a v1 refusal by name).
/// `Err`: the first difference.
///
/// # Errors
/// The first section or fragment that differs, a marked section that is
/// not kept, a hash that differs, or a `state-corrupt` of the walk.
pub fn live_check(
    f: &Front<'_>,
    m: &mortar_incr::update::testing::MemoryState,
) -> Result<String, String> {
    live_check_view(f, &m.view())
}

/// [`live_check`] over a view of the state's rows.
#[allow(clippy::many_single_char_names, reason = "f, r, s, n: the front, the state, its rows")]
#[allow(clippy::too_many_lines, reason = "one oracle, its comparisons in order")]
fn live_check_view(
    f: &Front<'_>,
    view: &mortar_incr::update::view::StateView<'_>,
) -> Result<String, String> {
    use mortar_core::split::EVERY_PIECE;
    use mortar_incr::update::rows::FileTable;
    let options = mortar_resolve::tables::option_keys(&f.plan, &f.inputs.link.directives);
    let walk = match mortar_incr::update::live::walk_stored(view, &options) {
        Ok(w) => w,
        Err((IncrReason::UpdateNotImplemented(_), detail)) => {
            return Ok(format!("incremental: live-check refused {detail}"));
        }
        Err((_, detail)) => return Err(detail),
    };
    let r = &f.resolution;
    let n = r.sections.len() as usize;
    let name = |s: mortar_core::SectionId| -> String {
        let Some((file, index)) = r.sh_index(s) else { return format!("section {}", s.0) };
        let d = f.inputs.files[file].display();
        let path = match d.member() {
            Some(m) => format!("{}({m})", d.path()),
            None => d.path().to_owned(),
        };
        let sec = match f.inputs.files[file].reader {
            mortar_core::inputs::Reader::Object(o) => o
                .section(index)
                .ok()
                .and_then(|x| x.name().ok())
                .map(|x| String::from_utf8_lossy(x).into_owned())
                .unwrap_or_default(),
            _ => String::new(),
        };
        format!("section {} ({path}:{}:{sec})", s.0, index.0)
    };
    // The reader's numbering: link section -> stored `SECTIONS` row, and
    // back.
    let rows = walk.live.len();
    let mut back: Vec<Option<mortar_core::SectionId>> = vec![None; rows];
    let mut ranges = Vec::new();
    for i in 0..n {
        let s = mortar_core::SectionId(i as u32);
        let live = f.liveness.is_live(s);
        let Some((file, index)) = r.sh_index(s) else {
            if live {
                return Err(format!("{} is live in the full mark and has no row", name(s)));
            }
            continue;
        };
        if ranges.len() <= file.0 as usize {
            ranges.resize(file.0 as usize + 1, None);
        }
        let range = if let Some(x) = ranges[file.0 as usize] {
            x
        } else {
            let x =
                view.file_index(file).map_err(|e| e.report())?.ranges[FileTable::Sections as usize];
            ranges[file.0 as usize] = Some(x);
            x
        };
        if index.0 >= range.count {
            if live {
                return Err(format!(
                    "{} is live in the full mark and past its file's rows",
                    name(s)
                ));
            }
            continue;
        }
        let row = (range.first + index.0) as usize;
        let Some(w) = walk.live.get(row).copied() else {
            return Err(format!("{}: row {row} past the walk's {rows}", name(s)));
        };
        back[row] = Some(s);
        if w && !r.sections[s].is_kept() {
            return Err(format!("{} is marked by the walk but not kept (P0-2)", name(s)));
        }
        if w != live {
            return Err(format!("{}: walk live={w}, full mark live={live}", name(s)));
        }
    }
    if let Some(row) = (0..rows).find(|&i| walk.live[i] && back[i].is_none()) {
        return Err(format!("SECTIONS row {row} is live in the walk and is no link section"));
    }
    let split = &f.merge.split;
    let mut frag = vec![false; split.frags.len() as usize];
    for (row, offsets) in &walk.pieces {
        let Some(s) = back.get(row.0 as usize).copied().flatten() else {
            return Err(format!("SECTIONS row {}: pieces and no link section", row.0));
        };
        if offsets.as_slice() == [EVERY_PIECE] {
            for &fr in split
                .merge_pieces(s)
                .and_then(|span| split.piece_frag.get(span.slots()))
                .unwrap_or_default()
            {
                frag[fr.0 as usize] = true;
            }
            continue;
        }
        for &o in offsets {
            if let Some(p) = split.merge_piece_at(s, o)
                && let Some(&fr) = split.piece_frag.get(p.piece as usize)
            {
                frag[fr.0 as usize] = true;
            }
        }
    }
    let full = f.liveness.frag_live.as_slice();
    if let Some(i) = (0..frag.len()).find(|&i| frag[i] != full.get(i).copied().unwrap_or(false)) {
        let first = split.frags.as_slice().get(i).map(|x| x.section);
        let at = first.map(&name).unwrap_or_default();
        return Err(format!(
            "fragment {i} (first in {at}): walk live={}, full mark live={}",
            frag[i], !frag[i]
        ));
    }
    let live = walk.live.iter().filter(|b| **b).count();
    let stored = view.meta().map_err(|e| e.report())?.live_hash;
    if stored == [0; 16] && live > 0 {
        return Err(format!("META.live_hash is zero and {live} sections are live"));
    }
    let h = mortar_incr::update::live::live_hash(view, &walk.live, &walk.pieces)
        .map_err(|e| e.report())?;
    if h != stored {
        return Err("META.live_hash: the stored rows do not give the stored live set".into());
    }
    let frags = frag.iter().filter(|b| **b).count();
    Ok(format!(
        "incremental: live-check agree sections {live}/{n} fragments {frags}/{} rows {}",
        frag.len(),
        view.count(format::kind::EDGES)
    ))
}

/// The state's path: `--incremental-state`, else `<output>.mortar-state`.
#[must_use]
pub fn state_path(parsed: &Parsed, cwd: &Path, out_path: &Path) -> PathBuf {
    if let Some(p) = &parsed.plan.incremental.state {
        return cwd.join(p);
    }
    let mut s = out_path.as_os_str().to_owned();
    s.push(".mortar-state");
    PathBuf::from(s)
}

/// The checks before a link in incremental mode.
pub fn start(parsed: &Parsed, cwd: &Path, out_path: &Path, diagnostics: &Collector) -> Start {
    let plan = &parsed.plan;
    let Some(mode) = plan.incremental.mode else { return Start::Plain };
    let update = mode == IncrementalMode::Update;
    if let Some(option) = unsupported(parsed) {
        if update {
            let reason = format!("incremental-unsupported ({option})");
            emit(diagnostics, 0, Diagnostic::error(Kind::IncrementalUpdateFailed { reason }));
            return Start::Failed;
        }
        emit(diagnostics, 0, Diagnostic::warning(Kind::IncrementalUnsupported { option }));
        return Start::Plain;
    }
    if std::env::var_os("SOURCE_DATE_EPOCH").is_some() {
        emit(diagnostics, 1, Diagnostic::warning(Kind::IncrementalNotReproducible));
    }
    let start = Instant::now();
    let state_path = state_path(parsed, cwd, out_path);
    let report = plan.incremental.report.as_ref().map(|r| match r {
        None => Report::Stderr,
        Some(p) => Report::File(cwd.join(p)),
    });
    // image.md I-58b, IN-17: the state is a regular file the link may
    // replace, or absent in a directory where it may create one; the
    // report is appended where the link may write. Anything else (a link
    // through a descriptor table, a pipe, a device, a name in /dev) is
    // refused by name before anything is read or written.
    if let Some(d) = refuse_own_file(&state_path, true).or_else(|| match &report {
        Some(Report::File(p)) => refuse_own_file(p, false),
        _ => None,
    }) {
        emit(diagnostics, 3, d);
        return Start::Failed;
    }
    // IN-17: neither file is one the link reads or writes: the one rule of
    // cli.md §3.14 (`crate::written`), given them by `Session::written`.
    let plan_hash = plan_hash(parsed, cwd);
    // IN-22a: one stamp per device, before any input is read.
    let stamps = take_stamps(parsed, cwd, out_path, &state_path);
    // IN-77b: every mode that finds a state locks it before reading past
    // its magic, and holds the lock to its state write; IN-77d: the lock
    // holder sweeps the leftovers first.
    let mut ops = mortar_incr::ops::StdOps;
    let (lock, locked) = match crate::update::write::lock_state(&state_path, &mut ops) {
        Ok(lock) => (lock, Ok(())),
        Err(reason) => (None, Err(reason)),
    };
    if lock.is_some() {
        crate::update::write::remove_leftovers(out_path, &state_path, &mut ops);
    }
    let writes_no_state = match locked {
        Err(IncrReason::StateBusy(_)) => Some("state-busy"),
        Err(IncrReason::LockUnavailable(_)) => Some("lock-unavailable"),
        _ => None,
    };
    let mut out = None;
    let (reason, recorded_target, size) = match locked {
        Err(reason) if mode != IncrementalMode::Full => (Some(reason), None, None),
        // `--incremental=full` links from scratch whatever the state says.
        _ if mode == IncrementalMode::Full => (None, None, None),
        _ => {
            let c = check(lock.as_ref(), plan_hash, out_path);
            out = c.out;
            (Some(c.reason), c.target, c.size)
        }
    };
    // §7 `small-output`: a state that could be updated from, of an output
    // below the bound, is linked in full; `--incremental=update` updates
    // whatever the size. G-13: a testhooks build may move the bound.
    let bound = mortar_incr::testhook::small_output_bound().unwrap_or(mortar_incr::SMALL_OUTPUT);
    let reason = match size {
        Some(size) if !update && reason == Some(UPDATABLE) && size < bound => {
            Some(IncrReason::SmallOutput(String::new()))
        }
        _ => reason,
    };
    let updatable = reason == Some(UPDATABLE);
    if update && !updatable {
        let reason = words(reason.as_ref());
        emit(diagnostics, 2, Diagnostic::error(Kind::IncrementalUpdateFailed { reason }));
        return Start::Failed;
    }
    let state4 = mortar_incr::testhook::state4();
    let state4_path = state4.then(|| format4_path(&state_path));
    // The dry run (WP-063, WP-106) is computed only for a state an update
    // could start from, and only when the report asks for it; after the
    // flip (WP-225) from the format-4 side state of a testhooks build.
    let old = if report.is_some() && dry_run_wanted(reason.as_ref()) {
        old4(&state_path, plan_hash, out_path).map(Box::new)
    } else {
        None
    };
    let session = Box::new(Session {
        state_path,
        report,
        plan_hash,
        stamps,
        reason,
        recorded_target,
        old,
        start,
        times: Times::default(),
        lock,
        writes_no_state,
        state5: None,
        state4_path,
        state4,
        legacy4: false,
        update_mode: update,
        out: if updatable { out } else { None },
    });
    if updatable { Start::Update(session) } else { Start::Full(session) }
}

/// The format-4 side state at `<state>.format4` (WP-225's flip), for the
/// dry run: `None` unless it decodes and every check of format 4 accepts
/// it against this plan and output (so a side state an update left stale
/// is never used).
fn old4(state_path: &Path, plan_hash: [u8; 16], out_path: &Path) -> Option<State> {
    let bytes = std::fs::read(format4_path(state_path)).ok()?;
    let s = format::decode(&bytes, crate::VERSION.as_bytes()).ok()?;
    (check_state(&s, plan_hash, out_path) == UPDATABLE).then_some(s)
}

/// The format-4 side state's path (WP-225): `<state>.format4`.
#[must_use]
pub fn format4_path(state_path: &Path) -> PathBuf {
    let mut s = state_path.as_os_str().to_owned();
    s.push(".format4");
    PathBuf::from(s)
}

/// I-58b for incremental mode's own files: `cannot-write-output` naming
/// `path` when the link may not write it: the state (`replace`) only by
/// the replace protocol, the report also in place (appended).
fn refuse_own_file(path: &Path, state: bool) -> Option<Diagnostic> {
    use mortar_image::output::{Disposition, Use, disposition};
    let (reason, note) = match disposition(path, Use::Whole) {
        Disposition::Replace => return None,
        Disposition::InPlace { .. } if !state => return None,
        Disposition::InPlace { .. } => ("not a regular file".to_owned(), None),
        Disposition::Refused { reason, note } | Disposition::Forbidden { reason, note } => {
            (reason, note)
        }
    };
    let kind = Kind::CannotWriteOutput { path: path.display().to_string(), reason };
    let mut d = Diagnostic::error(kind);
    if let Some(n) = note {
        d = d.note(n);
    }
    if state {
        d = d.note("the incremental state is a regular file, replaced whole (spec/incr.md IN-17)");
    }
    Some(d)
}

/// IN-22a: one stamp per device of the directories a link reads and
/// writes, before any input is read.
pub(crate) fn take_stamps(
    parsed: &Parsed,
    cwd: &Path,
    out_path: &Path,
    state_path: &Path,
) -> Vec<DeviceStamp> {
    let plan = &parsed.plan;
    let mut dirs: Vec<PathBuf> = Vec::new();
    let parent = |p: &Path| p.parent().map_or_else(|| cwd.to_path_buf(), Path::to_path_buf);
    dirs.push(parent(out_path));
    dirs.push(parent(state_path));
    dirs.push(std::env::var_os("TMPDIR").map_or_else(|| PathBuf::from("/tmp"), PathBuf::from));
    dirs.push(cwd.to_path_buf());
    for input in &plan.inputs {
        if let InputSpec::Path(p) = &input.spec {
            dirs.push(parent(&cwd.join(p)));
        }
    }
    for l in &plan.library_paths {
        dirs.push(cwd.join(&l.path));
    }
    // I-58b: no stamp file in a directory where the link writes nothing.
    dirs.retain(|d| !mortar_image::output::system_dir(d));
    let mountinfo = std::fs::read_to_string("/proc/self/mountinfo").unwrap_or_default();
    stamp::stamps(&dirs, &mountinfo)
}

/// The link's target as `META` records it: `e_machine` and the ELF class.
pub(crate) fn target_of(f: &Front<'_>) -> (u16, u8) {
    (f.target.machine.e_machine(), if f.target.class == mortar_elf::Class::Elf64 { 2 } else { 1 })
}

/// WP-225 M2b-3 (D-8 1), a testhooks build only: the identity oracle of
/// `mortar_incr::update::materialize` on an identity hop of the update
/// (`update::reference_of`, after the identity check): the stored layout of
/// `view` with an empty delta, materialized against the front `f`, must
/// equal `f`'s own layout. One line per hop appended to the file
/// `MORTAR_UPDATE_FINDINGS` names (the lane's count): `M2b-3 materialize:
/// identity hop equal`, or `finding: M2b-3 materialize: identity hop: ..`
/// with the refusal or the first differing table and row. Never changes
/// the update.
#[cfg(feature = "testhooks")]
pub(crate) fn materialize_hop(view: &mortar_incr::update::view::StateView<'_>, f: &Front<'_>) {
    let front_link = Link {
        inputs: &f.inputs,
        resolution: &f.resolution,
        assignment: &f.assignment,
        split: &f.merge.split,
    };
    let classes: Vec<mortar_core::incr::MergeClass> = f
        .merge
        .meta
        .iter()
        .map(|m| mortar_core::incr::MergeClass {
            strings: m.strings,
            entsize: m.entsize,
            align: m.align,
        })
        .collect();
    let line = match (Keys::new(&front_link), f.incr5.as_ref().map(|i| i.eh.as_ref())) {
        (Ok(keys), Some(Ok(runs))) => {
            let eh: Vec<mortar_incr::capture::EhFile> = runs
                .iter()
                .map(|r| mortar_incr::capture::EhFile {
                    file: r.file,
                    offset: r.offset,
                    capacity: r.capacity,
                    records: r.records,
                    fdes: r.fdes,
                    movable: r.movable,
                })
                .collect();
            let hop = mortar_incr::update::materialize::identity_hop(
                view,
                &front_link,
                &keys,
                &f.layout,
                &classes,
                &eh,
            );
            match hop {
                Ok(()) => "M2b-3 materialize: identity hop equal".to_owned(),
                Err(e) => format!("finding: M2b-3 materialize: identity hop: {e}"),
            }
        }
        (Err(e), _) => format!("finding: M2b-3 materialize: identity hop: keys: {e}"),
        (_, Some(Err(why))) => {
            format!("finding: M2b-3 materialize: identity hop: no .eh_frame runs: {why}")
        }
        (_, None) => "finding: M2b-3 materialize: identity hop: no format-5 front data".to_owned(),
    };
    if let Some(path) = std::env::var_os("MORTAR_UPDATE_FINDINGS")
        && let Ok(mut file) = std::fs::OpenOptions::new().create(true).append(true).open(path)
    {
        let _ = writeln!(file, "{line}");
    }
}

/// The files incremental mode writes, with the option naming each.
fn own_files<'a>(
    state_path: &'a Path,
    state4: Option<&'a Path>,
    report: Option<&'a Report>,
) -> Vec<(&'static str, &'a Path)> {
    let mut v = vec![("--incremental-state", state_path)];
    if let Some(p) = state4 {
        v.push(("--incremental-state", p));
    }
    if let Some(Report::File(p)) = report {
        v.push(("--incremental-report", p.as_path()));
    }
    v
}

/// What U1's checks of an existing state found ([`check`]).
struct Checked {
    reason: IncrReason,
    /// The state's target (`META` machine and class), when read.
    target: Option<(u16, u8)>,
    /// `META`'s output size, when read (`small-output`).
    size: Option<u64>,
    /// IN-77a: the output's descriptor, opened once.
    out: Option<std::fs::File>,
}

impl Checked {
    fn no(reason: IncrReason) -> Self {
        Self { reason, target: None, size: None, out: None }
    }
}

/// U1 (IN-70) on the locked state, in §7's order (WP-225: format 5 is THE
/// state): the updating flag read first, before any hash (review-in77
/// m-1); the header and directory with the plan hash (`state5::open`); a
/// format-4 state is `state-version` (a full link replaces it); `META`;
/// the output's binding through its descriptor (IN-21); after a reboot
/// with `output-synced` 0, the leaves (IN-77c, `boot_check`).
fn check(state: Option<&std::fs::File>, plan_hash: [u8; 16], out_path: &Path) -> Checked {
    // I-58b: a state is a regular file; anything else (a pipe through
    // /dev/stdout) is not opened (`lock_state` refused it by name). IN-77b:
    // it is read through the locked descriptor, never reopened by path.
    let Some(file) = state else {
        return Checked::no(IncrReason::NoState(String::new()));
    };
    let mut head = [0u8; mortar_incr::state5::HEADER];
    let n = read_prefix(file, &mut head);
    if head.get(..8) == Some(&format::MAGIC[..]) {
        return Checked::no(IncrReason::StateVersion(
            "format version 4 (MORTARST), this mortar reads 5".to_owned(),
        ));
    }
    if n == head.len()
        && head[..8] == mortar_incr::state5::MAGIC
        && u32::from_le_bytes([head[12], head[13], head[14], head[15]]) & format::FLAG_UPDATING != 0
    {
        return Checked::no(IncrReason::OutputTorn(String::new()));
    }
    let view = match mortar_incr::state5::open(file, plan_hash) {
        Ok(v) => v,
        // The report's line as format 4's (`full (options-changed)`, IN-63;
        // L10's and the lane's expectations): the plan hash is the reason.
        Err(IncrReason::OptionsChanged(_)) => {
            return Checked::no(IncrReason::OptionsChanged(String::new()));
        }
        Err(r) => return Checked::no(r),
    };
    let meta = match view.meta() {
        Ok(m) => m,
        Err(r) => return Checked::no(r),
    };
    // IN-18a: a state of another build of mortar is `state-version` (as
    // format 4's reader says; WP-247 may give it its own reason).
    match view.string(meta.version) {
        Ok(v) if v == crate::VERSION.as_bytes() => {}
        Ok(v) => {
            return Checked::no(IncrReason::StateVersion(format!(
                "written by mortar {}, this is {}",
                String::from_utf8_lossy(&v),
                crate::VERSION
            )));
        }
        Err(r) => return Checked::no(r),
    }
    let target = Some((meta.machine, meta.class));
    let size = Some(meta.file_size);
    let fail = |reason| Checked { reason, target, size, out: None };
    // IN-77a: one descriptor, read-write when it can be (the in-place
    // path writes through it; IN-45's copy path when it cannot be: a
    // running program's `ETXTBSY`, no write permission).
    let opened = std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .open(out_path)
        .or_else(|_| std::fs::File::open(out_path));
    let out = match opened {
        Ok(f) => f,
        Err(e) => return fail(IncrReason::OutputChanged(format!("cannot open the output: {e}"))),
    };
    if let Err(r) = output_matches5(&view, &meta, &out) {
        return fail(r);
    }
    if let Err(r) = crate::update::write::boot_check(&view, &out, crate::update::write::boot_id()) {
        return fail(r);
    }
    Checked { reason: UPDATABLE, target, size, out: Some(out) }
}

/// Up to `buf.len()` bytes from the start of `file` (positioned reads):
/// how many were read.
fn read_prefix(file: &std::fs::File, buf: &mut [u8]) -> usize {
    use std::os::unix::fs::FileExt as _;
    let mut n = 0;
    while n < buf.len() {
        match file.read_at(&mut buf[n..], n as u64) {
            Ok(0) | Err(_) => break,
            Ok(k) => n += k,
        }
    }
    n
}

/// IN-21 on a format-5 state, through the output's descriptor `out`: the
/// `STAT` output row's tuple (the last row, else the first with its tag),
/// then the build id at `META.sealed_start`, and when racy every leaf.
fn output_matches5(
    view: &mortar_incr::update::view::StateView<'_>,
    meta: &mortar_incr::update::rows::MetaRow,
    out: &std::fs::File,
) -> Result<(), IncrReason> {
    use mortar_incr::format::kind;
    use std::os::unix::fs::FileExt as _;
    let changed = IncrReason::OutputChanged;
    let rows = u32::try_from(view.count(kind::STAT)).unwrap_or(u32::MAX);
    let mut row = None;
    if let Some(last) = rows.checked_sub(1) {
        let r = view.stat(last)?;
        if r.tag == stat_tag::OUTPUT {
            row = Some(r);
        } else {
            for i in 0..last {
                let r = view.stat(i)?;
                if r.tag == stat_tag::OUTPUT {
                    row = Some(r);
                    break;
                }
            }
        }
    }
    let row = row.ok_or_else(|| changed("the state has no output row".to_owned()))?;
    let m = out.metadata().map_err(|e| changed(format!("cannot stat the output: {e}")))?;
    let now = Tuple::of(&m);
    let recorded = Tuple {
        dev: row.dev,
        ino: row.ino,
        size: row.size,
        mtime_ns: row.mtime_ns,
        ctime_ns: row.ctime_ns,
    };
    match stamp::classify(&recorded, &now, Some(row.stamp_ns)) {
        stamp::Cleanliness::Changed => Err(changed("its stat tuple differs".to_owned())),
        stamp::Cleanliness::Clean => {
            let len = usize::from(meta.build_id_len);
            let want = meta
                .build_id
                .get(..len)
                .ok_or_else(|| IncrReason::StateCorrupt("META: the build id is too long".into()))?;
            if !want.is_empty() {
                let mut id = vec![0u8; len];
                out.read_exact_at(&mut id, meta.sealed_start)
                    .map_err(|e| changed(format!("cannot read the output: {e}")))?;
                if id != want {
                    return Err(changed("its build id differs".to_owned()));
                }
            }
            Ok(())
        }
        stamp::Cleanliness::Racy | stamp::Cleanliness::Hashed => leaves_match5(view, meta, out),
    }
}

/// A racy output's bytes against the state's leaves (IN-21; the build id's
/// descriptor hashed as zeros, I-73), through its descriptor.
fn leaves_match5(
    view: &mortar_incr::update::view::StateView<'_>,
    m: &mortar_incr::update::rows::MetaRow,
    out: &std::fs::File,
) -> Result<(), IncrReason> {
    use std::os::unix::fs::FileExt as _;
    let changed = IncrReason::OutputChanged;
    let f = match m.leaf_fn {
        0 => TreeFn::L128,
        1 => TreeFn::Sha1,
        _ => TreeFn::Sha256,
    };
    let size = out.metadata().map_err(|e| changed(format!("cannot stat the output: {e}")))?.len();
    if size != m.file_size {
        return Err(changed("its size differs".to_owned()));
    }
    let leaves = view.leaves()?;
    let mut window = Vec::new();
    for (i, want) in leaves.iter().enumerate() {
        let r = mortar_digest::leaf_range(m.file_size, i as u64);
        window.clear();
        window.resize(usize::try_from(r.end - r.start).unwrap_or(0), 0);
        out.read_exact_at(&mut window, r.start)
            .map_err(|e| changed(format!("cannot read the output: {e}")))?;
        let lo = m.sealed_start.max(r.start);
        let hi = m.sealed_end.min(r.end);
        if lo < hi {
            window[(lo - r.start) as usize..(hi - r.start) as usize].fill(0);
        }
        let leaf = mortar_digest::leaf(f, &window);
        if leaf.as_bytes() != &want[..f.leaf_len()] {
            return Err(changed(format!("window {i} differs from its leaf")));
        }
    }
    Ok(())
}

/// The reason of a state every check of §7 accepted: an update could start
/// from it, and the update does not exist yet (stage A; printed with no
/// detail word).
const UPDATABLE: IncrReason = IncrReason::UpdateNotImplemented(None);

/// The words a reason prints (`incremental: full (<words>)`,
/// `incremental-update-failed`): `full requested` for `--incremental=full`.
fn words(reason: Option<&IncrReason>) -> String {
    reason.map_or_else(|| "full requested".to_owned(), IncrReason::report)
}

/// Whether a link whose reason is `reason` computes the dry run when the
/// report asks for it (`spec/incr.md` IN-10a): only for a state an update
/// could start from, an updatable one or one below `small-output`'s bound
/// (a policy decided from the size alone, which `--incremental=update`
/// does not apply). Every other reason says before the delta that no
/// update could run, and the report names it alone.
fn dry_run_wanted(reason: Option<&IncrReason>) -> bool {
    // WP-225: an update's own fall-back by its word (`update-not-implemented
    // (<word>)`) started from such a state too.
    matches!(reason, Some(IncrReason::UpdateNotImplemented(_) | IncrReason::SmallOutput(_)))
}

pub(crate) fn check_state(state: &State, plan_hash: [u8; 16], out_path: &Path) -> IncrReason {
    if state.plan_hash != plan_hash {
        return IncrReason::OptionsChanged(String::new());
    }
    if state.flags & format::FLAG_UPDATING != 0 {
        return IncrReason::OutputTorn(String::new());
    }
    if let Err(why) = output_matches(state, out_path) {
        return IncrReason::OutputChanged(why);
    }
    UPDATABLE
}

/// IN-21's check of the output against the state: its tuple, then its
/// build id, and when racy its bytes against the stored leaves.
pub fn output_matches(state: &State, out_path: &Path) -> Result<(), String> {
    let row = state
        .stat
        .iter()
        .find(|r| r.tag == stat_tag::OUTPUT)
        .ok_or("the state has no output row")?;
    let meta = std::fs::metadata(out_path).map_err(|e| format!("cannot stat the output: {e}"))?;
    let now = Tuple::of(&meta);
    let recorded = Tuple {
        dev: row.dev,
        ino: row.ino,
        size: row.size,
        mtime_ns: row.mtime_ns,
        ctime_ns: row.ctime_ns,
    };
    match stamp::classify(&recorded, &now, Some(row.stamp_ns)) {
        stamp::Cleanliness::Changed => Err("its stat tuple differs".to_owned()),
        stamp::Cleanliness::Clean => {
            let m = &state.meta;
            let want = state.str(m.build_id);
            if !want.is_empty() {
                // The reader holds the descriptor to the id's length, at
                // most MAX_BUILD_ID bytes: only that many are read.
                let mut buf = [0u8; format::MAX_BUILD_ID as usize];
                let id = buf.get_mut(..want.len()).ok_or("the build id is too long")?;
                let file = std::fs::File::open(out_path).map_err(|e| e.to_string())?;
                std::os::unix::fs::FileExt::read_exact_at(&file, id, m.sealed_start)
                    .map_err(|e| e.to_string())?;
                if id != want {
                    return Err("its build id differs".to_owned());
                }
            }
            Ok(())
        }
        stamp::Cleanliness::Racy | stamp::Cleanliness::Hashed => leaves_match(state, out_path),
    }
}

/// A racy output's bytes against the state's leaves (IN-21; the build id's
/// descriptor hashed as zeros, I-73).
fn leaves_match(state: &State, out_path: &Path) -> Result<(), String> {
    let m = &state.meta;
    let f = match m.leaf_fn {
        0 => TreeFn::L128,
        1 => TreeFn::Sha1,
        _ => TreeFn::Sha256,
    };
    let bytes = std::fs::read(out_path).map_err(|e| format!("cannot read the output: {e}"))?;
    if bytes.len() as u64 != m.file_size {
        return Err("its size differs".to_owned());
    }
    for (i, want) in state.leaves.iter().enumerate() {
        let r = mortar_digest::leaf_range(m.file_size, i as u64);
        let mut window = bytes[r.start as usize..r.end as usize].to_vec();
        let lo = m.sealed_start.max(r.start);
        let hi = m.sealed_end.min(r.end);
        if lo < hi {
            window[(lo - r.start) as usize..(hi - r.start) as usize].fill(0);
        }
        let leaf = mortar_digest::leaf(f, &window);
        if leaf.as_bytes() != &want[..f.leaf_len()] {
            return Err(format!("window {i} differs from its leaf"));
        }
    }
    Ok(())
}

pub(crate) fn hash_bytes(h: mortar_core::inputs::ContentHash) -> [u8; 16] {
    h.0.to_le_bytes()
}

impl Session {
    /// The files incremental mode writes, with the option naming each
    /// (IN-17): entries of the registry of cli.md §3.14.
    #[must_use]
    pub fn written(&self) -> Vec<(&'static str, &Path)> {
        own_files(&self.state_path, self.state4_path.as_deref(), self.report.as_ref())
    }

    /// WP-244: also write the format-5 state to `s.path` (tests; after the
    /// flip the state at `S` is format 5 whatever this says).
    pub fn set_state5(&mut self, s: Option<State5>) {
        self.state5 = s;
    }

    /// WP-225: also write the format-4 side state ([`format4_path`]);
    /// `legacy`: the pre-flip layout and ONLY that side state (the
    /// format-4 replay's tests, until M6).
    pub fn set_state4(&mut self, on: bool, legacy: bool) {
        self.state4 |= on || legacy;
        self.legacy4 |= legacy;
        self.state4_path = self.state4.then(|| format4_path(&self.state_path));
    }

    /// Whether the front computes format 5's data (`front::Env::state5`):
    /// always since the flip (WP-225), format 5 being THE state, except
    /// the legacy tests' links ([`Session::set_state4`]).
    #[must_use]
    pub fn state5(&self) -> bool {
        !self.legacy4
    }

    /// `--incremental=update`: a fall-back is an error, not a full link.
    #[must_use]
    pub fn update_mode(&self) -> bool {
        self.update_mode
    }

    /// The update fell back by `reason` before its first write (IN-70):
    /// the session becomes the full link's, with that reason; the output's
    /// descriptor is closed. Under `--incremental=update` the caller makes
    /// it an error instead ([`Session::update_mode`]).
    pub fn fall_back(&mut self, reason: IncrReason) {
        self.out = None;
        if self.report.is_none() || !dry_run_wanted(Some(&reason)) {
            self.old = None;
        }
        self.reason = Some(reason);
    }

    /// The state's path (`S`).
    #[must_use]
    pub fn state_path(&self) -> &Path {
        &self.state_path
    }

    /// The plan hash (IN-18b).
    #[must_use]
    pub fn plan_hash(&self) -> [u8; 16] {
        self.plan_hash
    }

    /// The locked state's descriptor (IN-77b), when one was found.
    #[must_use]
    pub fn lock(&self) -> Option<&std::fs::File> {
        self.lock.as_ref()
    }

    /// The stamps this link took (IN-22a).
    #[must_use]
    pub fn stamps(&self) -> &[DeviceStamp] {
        &self.stamps
    }

    /// The fall-back reason (for tests); `None` for `--incremental=full`.
    #[must_use]
    pub fn reason(&self) -> Option<&IncrReason> {
        self.reason.as_ref()
    }

    /// WP-094: the timed parts of the state's work, filled by
    /// [`Session::finish`].
    #[must_use]
    pub fn times_handle(&self) -> Times {
        Times::clone(&self.times)
    }

    /// The `incremental:` line (and its `delta` lines) where
    /// `--incremental-report` sends it: appended to the report file
    /// (I-58b: in place, never created where the link may create no
    /// file), or onto `stderr`. A full link's and an update's (WP-225).
    pub fn emit_report(&self, line: &str, stderr: &mut String) {
        match &self.report {
            Some(Report::Stderr) => stderr.push_str(line),
            Some(Report::File(path)) => {
                // I-58b: appended in place, never created where the link
                // may create no file.
                let refused =
                    match mortar_image::output::disposition(path, mortar_image::output::Use::Whole)
                    {
                        mortar_image::output::Disposition::Refused { reason, .. }
                        | mortar_image::output::Disposition::Forbidden { reason, .. } => {
                            Some(reason)
                        }
                        _ => None,
                    };
                let appended = refused.map_or_else(
                    || {
                        std::fs::OpenOptions::new()
                            .create(true)
                            .append(true)
                            .open(path)
                            .and_then(|mut file| file.write_all(line.as_bytes()))
                    },
                    |r| Err(std::io::Error::other(r)),
                );
                if let Err(e) = appended {
                    let _ = writeln!(stderr, "incremental: cannot write the report: {e}");
                }
            }
            None if !matches!(self.reason, None | Some(IncrReason::NoState(_))) => {
                let _ = writeln!(stderr, "incremental: full ({})", words(self.reason.as_ref()));
            }
            None => {}
        }
    }

    /// After a successful full link: the state written and bound to the
    /// output, and the lines for standard error (the fall-back's, or the
    /// report's when it goes there).
    #[allow(clippy::too_many_lines, reason = "the state's write, then the lines, in order")]
    pub fn finish(
        mut self,
        f: &Front<'_>,
        store: &Store,
        written: &Written,
        cwd: &Path,
        out_path: &Path,
        diagnostics: &Collector,
    ) -> String {
        let mut stderr = String::new();
        let mut dry = None;
        // §7: the target is part of the options; a state of another
        // target is `options-changed`, ahead of the output's reasons.
        if let Some(recorded) = self.recorded_target
            && recorded != target_of(f)
            && !matches!(self.reason, Some(IncrReason::OptionsChanged(_)))
        {
            self.reason = Some(IncrReason::OptionsChanged(String::new()));
        }
        // No update could start from a state of another target: no dry run
        // (WP-106; it ran on one before).
        if !dry_run_wanted(self.reason.as_ref()) {
            self.old = None;
        }
        if f.dyn_tables.is_some() {
            emit(
                diagnostics,
                3,
                Diagnostic::warning(Kind::IncrementalUnsupported {
                    option: "a shared object input (a dynamic executable)".to_owned(),
                }),
            );
        } else if let Some(why) = self.writes_no_state {
            // IN-77b: another link keeps replacing the state (`state-busy`:
            // it is left), or `flock` is not enforced there
            // (`lock-unavailable`: no later link could use it; removed by
            // its exact name).
            if why == "lock-unavailable" {
                let _ = mortar_image::output::remove_if_replaceable(&self.state_path);
            }
            let _ = writeln!(stderr, "incremental: no state written: {why}");
        } else {
            match self.state(f, store, written, cwd) {
                Ok((state, rows5)) => {
                    if let Some(old) = self.old.take() {
                        dry = Some(dry_run(&old, &state, f));
                    }
                    let mut t = Instant::now();
                    let mut failed = None;
                    let r = self.write_state(
                        f,
                        state,
                        rows5,
                        out_path,
                        written,
                        &mut stderr,
                        &mut failed,
                    );
                    lap(&self.times, &mut t, "bind, encode and write");
                    if let Err(why) = r {
                        let _ = writeln!(stderr, "incremental: no state written: {why}");
                    }
                    // WP-238 P0-7: the oracle's disagreement fails the link.
                    if let Some(what) = failed {
                        emit(diagnostics, 4, Diagnostic::error(Kind::InternalError { what }));
                    }
                }
                Err(why) => {
                    let _ = mortar_image::output::remove_if_replaceable(&self.state_path);
                    if let Some(s5) = &self.state5 {
                        let _ = mortar_image::output::remove_if_replaceable(&s5.path);
                    }
                    let _ = writeln!(stderr, "incremental: no state written: {why}");
                }
            }
        }
        // IN-77b: the lock is held to the state's last write.
        drop(self.lock.take());
        let (changed, refilled) =
            dry.as_ref().map_or((f.inputs.files.len().saturating_sub(1) as usize, 0), |d| {
                (d.changed, d.refilled)
            });
        let mut line = format!(
            "incremental: full ({}) changed={changed} refilled={refilled} bytes={} time={:.3}\n",
            words(self.reason.as_ref()),
            written.file_size,
            self.start.elapsed().as_secs_f64() * 1e3
        );
        if let Some(d) = &dry {
            line.push_str(&d.lines);
        }
        self.emit_report(&line, &mut stderr);
        stderr
    }

    /// The state written at `S` (WP-225's flip: FORMAT 5, `state5_rows`
    /// of the bound format-4 capture and `rows5`, encoded), through
    /// `write_state_whole` (IN-17, IN-77d). With the test switches also
    /// the format-4 side state and WP-244's copy. On any failure no state
    /// is left at `S` (an older one describes an output this link
    /// replaced), except `state-busy` (another link's temporary: `S` is
    /// left as it is).
    #[allow(clippy::too_many_arguments, reason = "the parts of the state's write")]
    fn write_state(
        &self,
        f: &Front<'_>,
        state: State,
        rows5: Option<mortar_incr::capture::Rows5>,
        out_path: &Path,
        written: &Written,
        stderr: &mut String,
        failed: &mut Option<String>,
    ) -> Result<(), String> {
        let mut t = Instant::now();
        let result = bind(state, out_path, written).and_then(|state| {
            lap(&self.times, &mut t, "bind");
            if self.legacy4 {
                let p = format4_path(&self.state_path);
                return write_atomically(&p, &format::encode(&state))
                    .map(|()| true)
                    .map_err(|e| format!("cannot write {}: {e}", p.display()));
            }
            let bytes = self.state5_bytes(f, &state, rows5, &mut t, stderr, failed)?;
            if let Some(p) = &self.state4_path {
                // The side state of a testhooks build (the floor, the dry
                // run): best effort, removed when it cannot be written.
                if write_atomically(p, &format::encode(&state)).is_err() {
                    let _ = mortar_image::output::remove_if_replaceable(p);
                }
                lap(&self.times, &mut t, "format 4 (side): encode and write");
            }
            // I-58b: the state is replaced only where the link may replace
            // a file.
            let r = mortar_image::output::may_replace(&self.state_path)
                .map_err(std::io::Error::other)
                .and_then(|()| {
                    crate::update::write::write_state_whole(
                        &self.state_path,
                        &bytes,
                        &mut mortar_incr::ops::StdOps,
                    )
                });
            lap(&self.times, &mut t, "write and sync");
            r.map_err(|e| format!("cannot write {}: {e}", self.state_path.display()))
        });
        let busy = matches!(result, Ok(false));
        let result = result.and_then(|written| {
            // IN-77d: the temporary is another live link's: no state, and
            // `S` is left as it is.
            if written {
                Ok(())
            } else {
                Err("state-busy (its temporary is another link's)".to_owned())
            }
        });
        if result.is_err() && !busy {
            // No state is left that the next link could accept: an older
            // one describes an output this link has replaced.
            let _ = mortar_image::output::remove_if_replaceable(&self.state_path);
            if let Some(p) = &self.state4_path {
                let _ = mortar_image::output::remove_if_replaceable(p);
            }
        }
        result
    }

    /// Format 5's bytes of `state` (bound) and `rows5` (WP-244's
    /// `state5_rows` + `state5_encode`); WP-244's test copy to `s5.path`
    /// and its rows sink when set. `state-collision`, `state-limit`
    /// (WP-239 R-2, S4-3) and every other failure: the error, no state.
    /// Under `--gc-sections` (WP-238 S4) the capture's live-set hash goes
    /// into `META` first, from the full mark's sets (P0-3); with
    /// `MORTAR_INCR_LIVE_CHECK` P0-7's oracle then judges the stored
    /// rows: a disagreement fills `failed` (the link fails) and no state
    /// is written.
    #[allow(clippy::too_many_arguments, reason = "the parts of the state's write")]
    fn state5_bytes(
        &self,
        f: &Front<'_>,
        state: &State,
        rows5: Option<mortar_incr::capture::Rows5>,
        t: &mut Instant,
        stderr: &mut String,
        failed: &mut Option<String>,
    ) -> Result<Vec<u8>, String> {
        let s5 = self.state5.as_ref();
        let rows = rows5.ok_or_else(|| "the link computed no format-5 data".to_owned())?;
        if s5.is_some() {
            for n in &rows.notes {
                let _ = writeln!(stderr, "incremental: state5 {n}");
            }
        }
        let collide = s5.is_some_and(|s| s.collide) || mortar_incr::testhook::state5_collide();
        let made = mortar_incr::capture::state5_rows(state, rows, collide).and_then(|mut m| {
            // IN-77c (WP-264): the boot this link wrote the output on; an
            // update on the same boot trusts `P` without re-hashing it.
            // Unreadable: zero, which no boot id matches (re-hash).
            m.meta.boot_id = crate::update::write::boot_id().unwrap_or_default();
            // WP-238 S4: the capture's live-set hash, from the full mark's
            // sets (P0-3), over the rows as the encoder numbers them (Part
            // 3b: `normalize` leaves the `SECTIONS` ranges to the encoder;
            // S4's sum is by file and position in the file, which the
            // encoder keeps). Timed on its own: `t` skips it.
            let gc = f.incr5.as_ref().and_then(|i| i.live_rows.as_ref());
            if let Some(Ok((_, at))) = gc {
                let start = Instant::now();
                let mut th = start;
                m.index_sections().map_err(|e| e.report())?;
                let h =
                    mortar_incr::update::live::live_hash(&m.view(), f.liveness.live.as_slice(), at)
                        .map_err(|e| e.report())?;
                m.meta.live_hash = mortar_incr::testhook::live_hash_mutated(h);
                lap(&self.times, &mut th, "format 5: live-set hash");
                *t += start.elapsed();
            }
            let bytes = mortar_incr::capture::state5_encode(&m)?;
            // P0-7's oracle when asked for, on the bytes the reader will
            // read (P3-1a): the reader's view of `bytes`. Its own clock.
            if matches!(gc, Some(Ok(_))) && mortar_incr::testhook::live_check() {
                let start = Instant::now();
                let mut th = start;
                let checked = mortar_incr::state5::read::open_bytes(bytes.clone())
                    .map_err(|e| e.report())
                    .and_then(|v| live_check_view(f, &v));
                lap(&self.times, &mut th, "format 5: live check");
                *t += start.elapsed();
                match checked {
                    Ok(line) => {
                        let _ = writeln!(stderr, "{line}");
                    }
                    Err(what) => {
                        let _ = writeln!(stderr, "incremental: live-check DISAGREE {what}");
                        *failed = Some(format!("live-check: {what}"));
                        return Err(format!("live-check: {what}"));
                    }
                }
            }
            Ok((m, bytes))
        });
        let (m, bytes) = match made {
            Ok(x) => x,
            Err(e) => {
                if let Some(s5) = s5 {
                    let _ = mortar_image::output::remove_if_replaceable(&s5.path);
                }
                return Err(e);
            }
        };
        if let Some(sink) = s5.and_then(|s| s.rows.as_ref())
            && let Ok(mut g) = sink.lock()
        {
            *g = Some(m);
        }
        lap(&self.times, t, "format 5: normalize and encode");
        if let Some(s5) = s5 {
            let _ = writeln!(stderr, "incremental: state5 bytes={}", bytes.len());
            if let Err(e) = write_atomically(&s5.path, &bytes) {
                let _ = mortar_image::output::remove_if_replaceable(&s5.path);
                let _ = writeln!(
                    stderr,
                    "incremental: no state written: cannot write {}: {e}",
                    s5.path.display()
                );
            }
            lap(&self.times, t, "format 5: the test copy");
        }
        Ok(bytes)
    }

    /// The state of the link `f`, all but its output row; with the
    /// format-5 switch, format 5's own rows beside it (WP-244).
    #[allow(clippy::too_many_lines)] // the state's parts, each timed (WP-094)
    pub(crate) fn state(
        &self,
        f: &Front<'_>,
        store: &Store,
        written: &Written,
        cwd: &Path,
    ) -> Result<(State, Option<mortar_incr::capture::Rows5>), String> {
        let link = Link {
            inputs: &f.inputs,
            resolution: &f.resolution,
            assignment: &f.assignment,
            split: &f.merge.split,
        };
        let mut t = Instant::now();
        // WP-094 (c): every file's keys in parallel; no global lookup.
        let keys = Keys::for_capture(&link, &f.pool)?;
        lap(&self.times, &mut t, "keys");
        let mut strings = Strings::default();
        // IN-24: the files' and members' hashes, one task per file on the
        // link's pool (WP-063; a member is a range of its archive).
        let hash_of = |id: FileId| -> Option<[u8; 16]> {
            let b = f.inputs.files.get(id)?.bytes?;
            let blob = store.blob_of_model(b.blob)?;
            let h = if b.base == 0 && b.len == store.len(blob) {
                store.hash(blob).ok()?
            } else {
                store.hash_range(blob, b.base..b.base + b.len).ok()?
            };
            Some(hash_bytes(h))
        };
        let hashes = f.pool.map(f.inputs.files.len() as usize, |i| hash_of(FileId(i as u32)));
        lap(&self.times, &mut t, "file hashes");
        let file_hash = |id: FileId| hashes.get(id.0 as usize).copied().flatten();
        let mut laps = mortar_incr::capture::Laps::new();
        let (mut state, rows5) = match &f.incr5 {
            Some(i5) => {
                let classes: Vec<mortar_core::incr::MergeClass> = f
                    .merge
                    .meta
                    .iter()
                    .map(|m| mortar_core::incr::MergeClass {
                        strings: m.strings,
                        entsize: m.entsize,
                        align: m.align,
                    })
                    .collect();
                let eh: Result<Vec<mortar_incr::capture::EhFile>, &str> =
                    i5.eh.as_ref().map_err(String::as_str).map(|runs| {
                        runs.iter()
                            .map(|r| mortar_incr::capture::EhFile {
                                file: r.file,
                                offset: r.offset,
                                capacity: r.capacity,
                                records: r.records,
                                fdes: r.fdes,
                                movable: r.movable,
                            })
                            .collect()
                    });
                let stack: Vec<(FileId, u8)> = i5
                    .stack
                    .iter()
                    .map(|&(file, n)| {
                        use mortar_incr::update::rows::file_stack;
                        use mortar_layout::stack::StackNote;
                        let b = match n {
                            StackNote::Absent => file_stack::ABSENT,
                            StackNote::NonExec => file_stack::NON_EXEC,
                            StackNote::Exec => file_stack::EXEC,
                        };
                        (file, b)
                    })
                    .collect();
                // SYMTAB_KEYS (WP-262): the symbol table plan's walk of its
                // global area, in incremental mode only (this branch).
                let symtab_keys = match &f.synth.symtab {
                    None => None,
                    Some(plan) => {
                        let area = plan.incr_global_area().ok_or(
                            "SYMTAB_KEYS: an incremental-mode .symtab without IN-49a's global area",
                        )?;
                        let keys = f
                            .with_sym_cx(|cx| plan.incr_global_keys(cx, &f.pool))
                            .map_err(|e| format!("SYMTAB_KEYS: {e:?}"))?
                            .ok_or("SYMTAB_KEYS: a .symtab plan without IN-49a's form")?
                            .map_err(|e| format!("SYMTAB_KEYS: {e:?}"))?;
                        // SYMTAB and META.symtab (WP-263): the plan's own
                        // blocks and sizes.
                        let blocks = plan
                            .incr_blocks()
                            .ok_or("SYMTAB: an incremental-mode .symtab without IN-49a's blocks")?;
                        let tables = plan.incr_tables().ok_or(
                            "META.symtab: an incremental-mode .symtab without IN-49a's tail",
                        )?;
                        Some((area, keys, blocks, tables))
                    }
                };
                lap(&self.times, &mut t, "symtab keys");
                let five = mortar_incr::capture::Link5 {
                    symtab: symtab_keys
                        .as_ref()
                        .map(|(a, k, b, t)| (*a, k.as_slice(), b.as_slice(), *t)),
                    stack: &stack,
                    properties: &i5.properties,
                    option_keys: &i5.option_keys,
                    liveness: &f.liveness,
                    classes: &classes,
                    eh: eh.as_deref().map_err(|e| *e),
                    demands: &i5.demands,
                    facts: &i5.facts,
                    slot_demand: crate::front::slot_demand,
                    copies: f.plan.diagnostics.odr.mode != mortar_core::plan::OdrMode::Off,
                    // WP-238 Part 2: IN-29's rows under --gc-sections.
                    edges: match &i5.live_rows {
                        None => None,
                        Some(Ok((edges, _))) => Some(edges),
                        Some(Err(why)) => return Err(why.clone()),
                    },
                };
                let (s, r) = mortar_incr::capture::capture5_timed(
                    &link,
                    &keys,
                    &f.layout,
                    &mut strings,
                    &file_hash,
                    f.arch.get(),
                    &f.pool,
                    &mut laps,
                    &five,
                )?;
                (s, Some(r))
            }
            None => (
                mortar_incr::capture::capture_timed(
                    &link,
                    &keys,
                    &f.layout,
                    &mut strings,
                    &file_hash,
                    f.arch.get(),
                    &f.pool,
                    &mut laps,
                )?,
                None,
            ),
        };
        lap(&self.times, &mut t, "capture");
        self.times.borrow_mut().extend(laps);
        state.plan_hash = self.plan_hash;
        for s in &self.stamps {
            state.stat.push(StatRow {
                tag: stat_tag::DEVICE,
                trust: u8::from(!s.trusted),
                path: strings.add(s.dir.as_os_str().as_encoded_bytes()),
                dev: s.dev,
                stamp_ns: s.ctime_ns,
                fstype: strings.add(s.fs_type.as_bytes()),
                ..StatRow::default()
            });
        }
        // One row per path: a path the link opens twice (`-lgfortran`
        // named twice on gfortran's line) is one file to watch.
        let mut seen: std::collections::HashSet<&[u8]> = std::collections::HashSet::new();
        let paths: Vec<&[u8]> =
            f.inputs.link.opened.iter().map(|o| o.path).filter(|p| seen.insert(p)).collect();
        // WP-105: the `stat`s on the link's pool (8,241 of them cost 0.9 s
        // serially on virtiofs); the rows are made in order below.
        let metas = f.pool.map(paths.len(), |i| {
            let path = PathBuf::from(<OsStr as std::os::unix::ffi::OsStrExt>::from_bytes(paths[i]));
            std::fs::metadata(cwd.join(&path)).ok()
        });
        for (&p, meta) in paths.iter().zip(metas) {
            let mut row = StatRow {
                tag: stat_tag::INPUT,
                trust: 1,
                path: strings.add(p),
                ..StatRow::default()
            };
            if let Some(m) = meta {
                use std::os::unix::fs::MetadataExt as _;
                let t = Tuple::of(&m);
                (row.dev, row.ino, row.size, row.mtime_ns, row.ctime_ns) =
                    (t.dev, t.ino, t.size, t.mtime_ns, t.ctime_ns);
                if let Some(blob) = store.blob_of_inode(m.dev(), m.ino())
                    && let Ok(h) = store.hash(blob)
                {
                    let read = store.meta(blob);
                    row.size = read.size;
                    row.mtime_ns = stamp::ns(read.mtime.0, read.mtime.1);
                    row.ctime_ns = stamp::ns(read.ctime.0, read.ctime.1);
                    row.hash = hash_bytes(h);
                    row.trust = 0;
                }
            }
            state.stat.push(row);
        }
        lap(&self.times, &mut t, "stat rows");
        let (leaf_fn, leaves) = written.leaves.as_ref().ok_or("the image kept no leaves")?;
        let sealed = written.sealed.clone().unwrap_or(0..0);
        state.meta = Meta {
            version: strings.add(crate::VERSION.as_bytes()),
            machine: target_of(f).0,
            class: target_of(f).1,
            output_kind: 0,
            leaf_fn: match leaf_fn {
                TreeFn::L128 => 0,
                TreeFn::Sha1 => 1,
                TreeFn::Sha256 => 2,
            },
            file_size: written.file_size,
            sealed_start: sealed.start,
            sealed_end: sealed.end,
            build_id: strings.add(written.build_id.as_deref().unwrap_or_default()),
        };
        for leaf in leaves {
            let mut row = [0u8; 32];
            let b = leaf.as_bytes();
            row[..b.len()].copy_from_slice(b);
            state.leaves.push(row);
        }
        state.strings = strings.into_bytes();
        Ok((state, rows5))
    }
}

/// The dry run's outcome (WP-063).
struct DryRun {
    changed: usize,
    refilled: usize,
    lines: String,
}

/// Change detection, the delta and the refill set from the state read
/// (`old`) to the full link's (`new`), as `delta` lines (`docs/cli.md`
/// §3.13).
fn dry_run(old: &State, new: &State, f: &Front<'_>) -> DryRun {
    let t0 = Instant::now();
    let changes = mortar_incr::detect::detect(old, new);
    let t1 = Instant::now();
    let link = Link {
        inputs: &f.inputs,
        resolution: &f.resolution,
        assignment: &f.assignment,
        split: &f.merge.split,
    };
    // Nothing changed: nothing to resolve, and nothing of the new link
    // beyond its state is asked for.
    let extra = if mortar_incr::delta::nothing_changed(old, new, &changes) {
        mortar_incr::delta::Extra::default()
    } else {
        mortar_incr::capture::extra(&link)
    };
    let delta =
        mortar_incr::delta::compute(old, new, &changes, &extra, mortar_incr::delta::Drop::NONE);
    let t2 = Instant::now();
    let mut lines = mortar_incr::delta::report(old, new, &changes, &delta, &extra);
    lines.truncate(lines.len().saturating_sub("delta end\n".len()));
    let _ = writeln!(
        lines,
        "delta time detect={:.3} delta={:.3}",
        (t1 - t0).as_secs_f64() * 1e3,
        (t2 - t1).as_secs_f64() * 1e3
    );
    lines.push_str("delta end\n");
    DryRun {
        changed: changes.new_files.iter().filter(|x| x.1.refills_all()).count(),
        refilled: delta.units.len(),
        lines,
    }
}

/// IN-21: the output's tuple and reference stamp, after its last write.
fn bind(mut state: State, out_path: &Path, written: &Written) -> Result<State, String> {
    let (t, stamp) = stamp::bind_output(out_path)
        .map_err(|e| format!("cannot bind the output: {e}"))?
        .ok_or("the output changed while it was bound (or no stamp could be taken)")?;
    // IN-21: the state describes the file this link wrote, which the path
    // may no longer name (another link renamed its own output over it).
    let ours = written.file.ok_or("the written output's identity is unknown")?;
    if (t.dev, t.ino) != ours || t.size != written.file_size {
        return Err(format!(
            "{} is no longer the file this link wrote (another link replaced it)",
            out_path.display()
        ));
    }
    state.stat.push(StatRow {
        tag: stat_tag::OUTPUT,
        trust: 0,
        dev: t.dev,
        ino: t.ino,
        size: t.size,
        mtime_ns: t.mtime_ns,
        ctime_ns: t.ctime_ns,
        stamp_ns: stamp,
        ..StatRow::default()
    });
    Ok(state)
}

/// A temporary file in the state's directory, synced, renamed over the
/// state, and the directory synced (IN-17), with IN-77d's protocol for the
/// temporary (`crate::update::write::write_state_whole`; a temporary held
/// by another link is an error here, `state-busy`).
pub(crate) fn write_atomically(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    mortar_image::output::may_replace(path).map_err(std::io::Error::other)?;
    let mut ops = mortar_incr::ops::StdOps;
    if crate::update::write::write_state_whole(path, bytes, &mut ops)? {
        Ok(())
    } else {
        Err(std::io::Error::other("state-busy (its temporary is another link's)"))
    }
}
