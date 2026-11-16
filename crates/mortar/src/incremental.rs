//! Incremental mode in the facade (`spec/incr.md` §5, WP-061 stage A):
//! the checks before a link (IN-12's unsupported modes, the stamps of
//! IN-22a, the existing state read and validated, the fall-back reason of
//! §7), and after a successful full link the state written beside the
//! output and bound to it (IN-17, IN-18a, IN-21). There is no update yet:
//! every incremental link is a full link (`update-not-implemented`).
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
    start: Instant,
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
    if s.icf.is_some_and(|i| i != Icf::None) {
        return Some("--icf".to_owned());
    }
    if s.compress_debug_sections.is_some_and(|c| c != Compression::None) {
        return Some("--compress-debug-sections".to_owned());
    }
    if s.gdb_index == Some(true) {
        return Some("--gdb-index".to_owned());
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
    // IN-17: neither file is one the link reads or writes: the one rule of
    // cli.md §3.14 (`crate::written`), given them by `Session::written`.
    let plan_hash = plan_hash(parsed, cwd);
    // IN-22a: one stamp per device, before any input is read.
    let stamps = take_stamps(parsed, cwd, out_path, &state_path);
    let (reason, recorded_target) = if mode == IncrementalMode::Full {
        // `--incremental=full` links from scratch whatever the state says.
        ("full requested".to_owned(), None)
    } else {
        check(&state_path, plan_hash, out_path)
    };
    if update {
        emit(diagnostics, 2, Diagnostic::error(Kind::IncrementalUpdateFailed { reason }));
        return Start::Failed;
    }
    Start::Full(Session { state_path, report, plan_hash, stamps, reason, recorded_target, start })
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
fn check(state_path: &Path, plan_hash: [u8; 16], out_path: &Path) -> (String, Option<(u16, u8)>) {
    let bytes = match std::fs::read(state_path) {
        Ok(b) => b,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return ("no-state".to_owned(), None),
        Err(e) => return (format!("state-corrupt: cannot read the state: {e}"), None),
    };
    let state = match format::decode(&bytes, crate::VERSION.as_bytes()) {
        Ok(s) => s,
        Err(e) => return (format!("{}: {}", e.reason(), e.detail()), None),
    };
    let target = Some((state.meta.machine, state.meta.class));
    (check_state(&state, plan_hash, out_path), target)
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
    "update-not-implemented".to_owned()
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
        // §7: the target is part of the options; a state of another
        // target is `options-changed`, ahead of the output's reasons.
        if let Some(recorded) = self.recorded_target
            && recorded != target_of(f)
            && self.reason != "options-changed"
        {
            "options-changed".clone_into(&mut self.reason);
        }
        if f.dyn_tables.is_some() {
            emit(
                diagnostics,
                3,
                Diagnostic::warning(Kind::IncrementalUnsupported {
                    option: "a shared object input (a dynamic executable)".to_owned(),
                }),
            );
        } else if let Err(why) = self.write_state(f, store, written, cwd, out_path) {
            let _ = writeln!(stderr, "incremental: no state written: {why}");
        }
        let line = format!(
            "incremental: full ({}) changed={} refilled=0 bytes={} time={:.3}\n",
            self.reason,
            f.inputs.files.len().saturating_sub(1),
            written.file_size,
            self.start.elapsed().as_secs_f64() * 1e3
        );
        match &self.report {
            Some(Report::Stderr) => stderr.push_str(&line),
            Some(Report::File(path)) => {
                let appended = std::fs::OpenOptions::new()
                    .create(true)
                    .append(true)
                    .open(path)
                    .and_then(|mut file| file.write_all(line.as_bytes()));
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

    fn write_state(
        &self,
        f: &Front<'_>,
        store: &Store,
        written: &Written,
        cwd: &Path,
        out_path: &Path,
    ) -> Result<(), String> {
        let result = self.state(f, store, written, cwd).and_then(|state| {
            let state = bind(state, out_path, written)?;
            write_atomically(&self.state_path, &format::encode(&state))
                .map_err(|e| format!("cannot write {}: {e}", self.state_path.display()))
        });
        if result.is_err() {
            // No state is left that the next link could accept: an older
            // one describes an output this link has replaced.
            let _ = std::fs::remove_file(&self.state_path);
        }
        result
    }

    /// The state of the link `f`, all but its output row.
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
        let keys = Keys::new(&link)?;
        let mut strings = Strings::default();
        let file_hash = |id: FileId| -> Option<[u8; 16]> {
            let b = f.inputs.files.get(id)?.bytes?;
            let blob = store.blob_of_model(b.blob)?;
            let h = if b.base == 0 && b.len == store.len(blob) {
                store.hash(blob).ok()?
            } else {
                store.hash_range(blob, b.base..b.base + b.len).ok()?
            };
            Some(hash_bytes(h))
        };
        let mut state =
            mortar_incr::capture::capture(&link, &keys, &f.layout, &mut strings, &file_hash)?;
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
