//! Incremental mode in the facade (`spec/incr.md` §5, WP-061 stage A):
//! the checks before a link (IN-12's unsupported modes, the stamps of
//! IN-22a, the existing state read and validated, the fall-back reason of
//! §7), and after a successful full link the state written beside the
//! output and bound to it (IN-17, IN-18a, IN-21). There is no update yet:
//! every incremental link is a full link (`small-output` below its bound,
//! else `update-not-implemented`).
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
    Full(Session),
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
    /// The fall-back reason, with its detail (§7).
    reason: String,
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
}

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
    let (reason, recorded_target, old) = if mode == IncrementalMode::Full {
        // `--incremental=full` links from scratch whatever the state says.
        ("full requested".to_owned(), None, None)
    } else {
        check(&state_path, plan_hash, out_path)
    };
    // §7 `small-output`: a state that could be updated from, of an output
    // below the bound, is linked in full; `--incremental=update` updates
    // whatever the size (so far: fails, `update-not-implemented`).
    let reason = match &old {
        Some(s)
            if !update && reason == UPDATABLE && mortar_incr::small_output(s.meta.file_size) =>
        {
            SMALL_OUTPUT.to_owned()
        }
        _ => reason,
    };
    // The dry run (WP-063, WP-106) is computed only for a state an update
    // could start from, and only when the report asks for it.
    let old = old.filter(|_| report.is_some() && dry_run_wanted(&reason)).map(Box::new);
    if update {
        emit(diagnostics, 2, Diagnostic::error(Kind::IncrementalUpdateFailed { reason }));
        return Start::Failed;
    }
    Start::Full(Session {
        state_path,
        report,
        plan_hash,
        stamps,
        reason,
        recorded_target,
        old,
        start,
        times: Times::default(),
    })
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

/// The files incremental mode writes, with the option naming each.
fn own_files<'a>(
    state_path: &'a Path,
    report: Option<&'a Report>,
) -> Vec<(&'static str, &'a Path)> {
    let mut v = vec![("--incremental-state", state_path)];
    if let Some(Report::File(p)) = report {
        v.push(("--incremental-report", p.as_path()));
    }
    v
}

/// The fall-back reason of an existing state, in §7's order: the state
/// read and validated, the plan, the updating flag, the output; with the
/// state's target, which the link's is compared with once it is known
/// (`Session::finish`).
fn check(
    state_path: &Path,
    plan_hash: [u8; 16],
    out_path: &Path,
) -> (String, Option<(u16, u8)>, Option<State>) {
    // I-58b: a state is a regular file; anything else (a pipe through
    // /dev/stdout, which this process would read until it writes) is not
    // read.
    if std::fs::metadata(state_path).is_ok_and(|m| !m.is_file()) {
        return ("state-corrupt: not a regular file".to_owned(), None, None);
    }
    let bytes = match std::fs::read(state_path) {
        Ok(b) => b,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            return ("no-state".to_owned(), None, None);
        }
        Err(e) => return (format!("state-corrupt: cannot read the state: {e}"), None, None),
    };
    let state = match format::decode(&bytes, crate::VERSION.as_bytes()) {
        Ok(s) => s,
        Err(e) => return (format!("{}: {}", e.reason(), e.detail()), None, None),
    };
    let target = Some((state.meta.machine, state.meta.class));
    (check_state(&state, plan_hash, out_path), target, Some(state))
}

/// The reason of a state every check of §7 accepted: an update could start
/// from it, and the update does not exist yet (stage A).
const UPDATABLE: &str = "update-not-implemented";

/// §7's `small-output` (`mortar_incr::SMALL_OUTPUT`).
const SMALL_OUTPUT: &str = "small-output";

/// Whether a link whose reason is `reason` computes the dry run when the
/// report asks for it (`spec/incr.md` IN-10a): only for a state an update
/// could start from, an updatable one or one below `small-output`'s bound
/// (a policy decided from the size alone, which `--incremental=update`
/// does not apply). Every other reason says before the delta that no
/// update could run, and the report names it alone.
fn dry_run_wanted(reason: &str) -> bool {
    reason == UPDATABLE || reason == SMALL_OUTPUT
}

pub(crate) fn check_state(state: &State, plan_hash: [u8; 16], out_path: &Path) -> String {
    if state.plan_hash != plan_hash {
        return "options-changed".to_owned();
    }
    if state.flags & format::FLAG_UPDATING != 0 {
        return "output-torn".to_owned();
    }
    if let Err(why) = output_matches(state, out_path) {
        return format!("output-changed: {why}");
    }
    UPDATABLE.to_owned()
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
        own_files(&self.state_path, self.report.as_ref())
    }

    /// The fall-back reason (for tests).
    #[must_use]
    pub fn reason(&self) -> &str {
        &self.reason
    }

    /// WP-094: the timed parts of the state's work, filled by
    /// [`Session::finish`].
    #[must_use]
    pub fn times_handle(&self) -> Times {
        Times::clone(&self.times)
    }

    /// After a successful full link: the state written and bound to the
    /// output, and the lines for standard error (the fall-back's, or the
    /// report's when it goes there).
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
            && self.reason != "options-changed"
        {
            "options-changed".clone_into(&mut self.reason);
        }
        // No update could start from a state of another target: no dry run
        // (WP-106; it ran on one before).
        if !dry_run_wanted(&self.reason) {
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
        } else {
            match self.state(f, store, written, cwd) {
                Ok(state) => {
                    if let Some(old) = self.old.take() {
                        dry = Some(dry_run(&old, &state, f));
                    }
                    let mut t = Instant::now();
                    let r = self.write_state(state, out_path, written);
                    lap(&self.times, &mut t, "bind, encode and write");
                    if let Err(why) = r {
                        let _ = writeln!(stderr, "incremental: no state written: {why}");
                    }
                }
                Err(why) => {
                    let _ = mortar_image::output::remove_if_replaceable(&self.state_path);
                    let _ = writeln!(stderr, "incremental: no state written: {why}");
                }
            }
        }
        let (changed, refilled) =
            dry.as_ref().map_or((f.inputs.files.len().saturating_sub(1) as usize, 0), |d| {
                (d.changed, d.refilled)
            });
        let mut line = format!(
            "incremental: full ({}) changed={changed} refilled={refilled} bytes={} time={:.3}\n",
            self.reason,
            written.file_size,
            self.start.elapsed().as_secs_f64() * 1e3
        );
        if let Some(d) = &dry {
            line.push_str(&d.lines);
        }
        match &self.report {
            Some(Report::Stderr) => stderr.push_str(&line),
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
            None if self.reason != "no-state" && self.reason != "full requested" => {
                let _ = writeln!(stderr, "incremental: full ({})", self.reason);
            }
            None => {}
        }
        stderr
    }

    fn write_state(&self, state: State, out_path: &Path, written: &Written) -> Result<(), String> {
        let mut t = Instant::now();
        let result = bind(state, out_path, written).and_then(|state| {
            lap(&self.times, &mut t, "bind");
            let bytes = format::encode(&state);
            lap(&self.times, &mut t, "encode");
            let r = write_atomically(&self.state_path, &bytes)
                .map_err(|e| format!("cannot write {}: {e}", self.state_path.display()));
            lap(&self.times, &mut t, "write and sync");
            r
        });
        if result.is_err() {
            // No state is left that the next link could accept: an older
            // one describes an output this link has replaced.
            let _ = mortar_image::output::remove_if_replaceable(&self.state_path);
        }
        result
    }

    /// The state of the link `f`, all but its output row.
    #[allow(clippy::too_many_lines)] // the state's parts, each timed (WP-094)
    fn state(
        &self,
        f: &Front<'_>,
        store: &Store,
        written: &Written,
        cwd: &Path,
    ) -> Result<State, String> {
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
        let mut state = mortar_incr::capture::capture_timed(
            &link,
            &keys,
            &f.layout,
            &mut strings,
            &file_hash,
            f.arch.get(),
            &f.pool,
            &mut laps,
        )?;
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
        for o in &f.inputs.link.opened {
            if !seen.insert(o.path) {
                continue;
            }
            let path = PathBuf::from(<OsStr as std::os::unix::ffi::OsStrExt>::from_bytes(o.path));
            let mut row = StatRow {
                tag: stat_tag::INPUT,
                trust: 1,
                path: strings.add(o.path),
                ..StatRow::default()
            };
            if let Ok(m) = std::fs::metadata(cwd.join(&path)) {
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
        Ok(state)
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
/// state, and the directory synced (IN-17).
pub(crate) fn write_atomically(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    let dir = match path.parent() {
        Some(p) if !p.as_os_str().is_empty() => p.to_path_buf(),
        _ => PathBuf::from("."),
    };
    let mut tmp = path.as_os_str().to_owned();
    tmp.push(format!(".tmp-{}", std::process::id()));
    let tmp = PathBuf::from(tmp);
    // I-58b: the state is replaced only where the link may replace a file.
    mortar_image::output::may_replace(path).map_err(std::io::Error::other)?;
    let result = (|| {
        let mut file = std::fs::File::create(&tmp)?;
        file.write_all(bytes)?;
        file.sync_data()?;
        drop(file);
        std::fs::rename(&tmp, path)?;
        std::fs::File::open(&dir)?.sync_all()
    })();
    if result.is_err() {
        let _ = std::fs::remove_file(&tmp);
    }
    result
}
