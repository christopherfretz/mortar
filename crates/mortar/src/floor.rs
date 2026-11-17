//! The test-only update path of WP-061 stage B (`spec/incr.md` §1.5's
//! "Go/no-go rule", §5.2): the measured floor of an incremental update,
//! for a same-size edit of one input file, so that the previous layout
//! stands. It is not reachable from the command line (`--incremental=update`
//! still fails by name, `update-not-implemented`): tests and the
//! measurement (`examples/incr_floor.rs`) call [`update`].
//!
//! Every step of §5.2 runs with mortar's own code, in this order:
//!
//! 1. the state read and validated (IN-18a), checked against the plan and
//!    the output (IN-21; phase 15);
//! 2. a `stat` of every input the state lists, and new stamps (IN-22,
//!    IN-22a; phase 16);
//! 3. change detection: every input that is not clean is read and hashed
//!    (IN-22); exactly one may differ (phase 17). This is the one-file case
//!    of WP-063, which replaces it;
//! 4. re-resolution and the rest of the front over the edited inputs, with
//!    the layout taken from the state (the replay hook, IN-61; phases
//!    20-90). The replay translates the files of IN-28a's positional set
//!    by position (decided first: the `POSITIONAL` bit, unchanged, the
//!    same range, the section rows; identities are built for every other
//!    file), or every file by identity with `MORTAR_FLOOR_IDENTITY`. The
//!    front of today parses, splits and scans every input: the design
//!    restricts those to the changed file (§5.2, WP-064). After it, every
//!    file the front read is checked to be the file change detection
//!    judged (its descriptor's `fstat` after the read against the judged
//!    tuple: `input-changed`), and the changed file's contribution is
//!    compared with the state's rows (`mortar_incr::contribution`: a
//!    different section count, section header, merged-fragment position,
//!    global key, binding, type, visibility, definedness, section or
//!    resolution is `layout-changed` or `resolution-changed`);
//! 5. the dirty windows (image.md I-95) filled into memory through the
//!    image phase (phase 100): the windows of the changed file's placed
//!    sections, of the sections an update rewrites whole (`.symtab`,
//!    `.strtab`, `.symtab_shndx`, S-112; `.eh_frame_hdr`; `.debug_aranges`,
//!    IN-54), of `.eh_frame` (whole: per-file `.eh_frame` ranges are
//!    IN-34a's), the ELF header and the build id; the build id from the
//!    refilled leaves and the state's others (I-76); the update's map when
//!    `--mortar-map` is on its line (the records a full link writes);
//! 6. the writes of IN-46 (in place) or IN-47 (copy; phase 105), then the
//!    map;
//! 7. the state rewritten whole, bound to the output (IN-17, IN-21;
//!    phase 106), with the changed file's position rows recaptured from
//!    the bytes it hashed, or its `POSITIONAL` bit cleared (IN-28a, the
//!    writer's duty).
//!
//! What this path does NOT do (WP-064's): it refills the changed file's
//! own units only. A same-size edit CAN move a definition or change a
//! binding (a weak definition made global, the review's M4), change what
//! is merged where, or change the layout; step 4's comparison refuses
//! every such edit by name before anything is written, and the update
//! proper must handle them (delta resolution, IN-27's refill set). What
//! the comparison lets through keeps the changed file's sections, globals
//! and resolution as stored: its output is equivalent to the full link of
//! the edited inputs (`xtask plumb --equiv`), and byte-equal to it when
//! the edit leaves the file's merged contents and section order alone
//! (a permutation keeps the stored places; a merged string that changes
//! keeps its stored offset and leaves a dead fragment, the review's m2).

use std::ffi::{OsStr, OsString};
use std::fs::{File, OpenOptions};
use std::io::Write as _;
use std::ops::Range;
use std::os::unix::ffi::OsStrExt as _;
use std::os::unix::fs::{FileExt as _, MetadataExt as _};
use std::path::{Path, PathBuf};

use mortar_core::layout::ChunkKind;
use mortar_diag::Collector;
use mortar_digest::{Leaf, LeafColumn, TreeFn};
use mortar_incr::capture::{Keys, Positional, Translation};
use mortar_incr::format::{self, State, Str, stat_tag};
use mortar_incr::stamp::{self, Cleanliness, Tuple};
use mortar_input::store::{Store, StoreConfig, StorePolicy};

use crate::incremental;
use crate::timing::Timer;

/// The write path of an update (IN-45).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WritePath {
    /// IN-47: a copy of the output, written and renamed over it.
    Copy,
    /// IN-46: the output written in place.
    InPlace,
}

/// What an update did.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Summary {
    /// The changed input, as the state names it.
    pub changed: String,
    /// Inputs stat'ed, and of them hashed (not clean).
    pub inputs: usize,
    pub hashed: usize,
    /// Dirty windows, the jobs filled for them and their bytes.
    pub windows: usize,
    pub jobs: usize,
    pub refilled: u64,
    /// WP-094: the bytes of the dirty ranges themselves (their union),
    /// which the windows round up to `IMAGE_BLOCK`.
    pub dirty: u64,
    /// WP-094: the bytes written into the output.
    pub written: u64,
    /// The output's size and the new state's.
    pub file_size: u64,
    pub state_size: u64,
    /// The dirty windows by kind (`header`, `file`: the changed file's
    /// sections, `symtab`, `eh_frame`, `aranges`, `build-id`), each kind's
    /// own count: `kind:count,...`.
    pub kinds: String,
}

/// How an update ended: done, or the reason it could not update (nothing
/// written when the reason comes before the writes, IN-13), with the
/// link's report and the phase lines.
#[derive(Debug)]
pub struct Update {
    pub outcome: Result<Summary, String>,
    pub report: Option<mortar_diag::Report>,
    pub phases: String,
}

/// The update of the link `args` in `cwd` (see the module's text). The
/// command line is the one of the full link that wrote the state
/// (`--incremental`'s rows aside, IN-18b).
#[must_use]
pub fn update(args: &[OsString], cwd: &Path, write: WritePath) -> Update {
    let mut timer = Timer::new(true);
    let mut report = None;
    let outcome = run(args, cwd, write, &mut timer, &mut report);
    Update { outcome, report, phases: timer.finish() }
}

fn ms(d: std::time::Duration) -> f64 {
    d.as_secs_f64() * 1000.0
}

fn path_of(bytes: &[u8]) -> PathBuf {
    PathBuf::from(OsStr::from_bytes(bytes))
}

/// A string appended to the state's pool.
fn add_str(state: &mut State, s: &[u8]) -> Str {
    if s.is_empty() {
        return Str::default();
    }
    let at = Str { off: state.strings.len() as u32, len: s.len() as u32 };
    state.strings.extend_from_slice(s);
    at
}

#[allow(
    clippy::too_many_lines,
    clippy::many_single_char_names,
    reason = "the steps of the module's text, in order"
)]
fn run(
    args: &[OsString],
    cwd: &Path,
    write: WritePath,
    timer: &mut Timer,
    report: &mut Option<mortar_diag::Report>,
) -> Result<Summary, String> {
    let parsed = mortar_cli::parse(args);
    if parsed.failed() || !parsed.refusals().is_empty() {
        return Err("the command line has errors or refused options".to_owned());
    }
    let plan = &parsed.plan;
    timer.phase(10, "command line");
    let out_path = cwd.join(plan.output.as_deref().unwrap_or_else(|| OsStr::new("a.out")));
    let state_path = incremental::state_path(&parsed, cwd, &out_path);

    // 1. The state, read and validated, against the plan and the output.
    let bytes = std::fs::read(&state_path).map_err(|e| format!("no-state: {e}"))?;
    let mut state = format::decode(&bytes, crate::VERSION.as_bytes())
        .map_err(|e| format!("{}: {}", e.reason(), e.detail()))?;
    drop(bytes);
    let reason = incremental::check_state(&state, incremental::plan_hash(&parsed, cwd), &out_path);
    if reason != "update-not-implemented" {
        return Err(reason);
    }
    timer.phase(15, "state");

    // 2. Stat of every input, and the stamps of this link (IN-22a).
    let stamps = incremental::take_stamps(&parsed, cwd, &out_path, &state_path);
    let trusted: Vec<(u64, i64)> = state
        .stat
        .iter()
        .filter(|r| r.tag == stat_tag::DEVICE && r.trust == 0)
        .map(|r| (r.dev, r.stamp_ns))
        .collect();
    let mut suspects: Vec<(usize, PathBuf, Tuple)> = Vec::new();
    // m3 (WP-064a): every input as change detection judged it.
    let mut judged: Vec<Tuple> = Vec::new();
    let mut inputs = 0;
    // WP-105: the `stat`s on the link's pool (8,241 of them cost 0.83 s
    // serially on virtiofs); the rows are judged in order below.
    let pool = crate::pool_of(plan).map_err(|_| "a malformed MORTAR_SCHED_SEED".to_owned())?;
    let metas = pool.map(state.stat.len(), |i| {
        let row = &state.stat[i];
        (row.tag == stat_tag::INPUT).then(|| {
            let path = cwd.join(path_of(state.str(row.path)));
            let meta = std::fs::metadata(&path)
                .map_err(|e| format!("input-changed: cannot stat {}: {e}", path.display()));
            (path, meta)
        })
    });
    for ((i, row), m) in state.stat.iter().enumerate().zip(metas) {
        let Some((path, meta)) = m else { continue };
        inputs += 1;
        let now = Tuple::of(&meta?);
        judged.push(now);
        let recorded = Tuple {
            dev: row.dev,
            ino: row.ino,
            size: row.size,
            mtime_ns: row.mtime_ns,
            ctime_ns: row.ctime_ns,
        };
        let s = trusted.iter().find(|t| t.0 == row.dev).map(|t| t.1);
        if row.trust != 0 || stamp::classify(&recorded, &now, s) != Cleanliness::Clean {
            suspects.push((i, path, now));
        }
    }
    timer.phase(16, "stat and stamps");

    // 3. Change detection: what is not clean is hashed (IN-22).
    let hashed = suspects.len();
    let mut changed = Vec::new();
    for (i, path, now) in suspects {
        let b = std::fs::read(&path).map_err(|e| format!("cannot read {}: {e}", path.display()))?;
        let h = incremental::hash_bytes(mortar_input::store::content_hash(&b));
        let row = &mut state.stat[i];
        if row.trust != 0 && row.hash == [0; 16] {
            return Err(format!("the state has no hash of {}", path.display()));
        }
        (row.dev, row.ino, row.size, row.mtime_ns, row.ctime_ns) =
            (now.dev, now.ino, now.size, now.mtime_ns, now.ctime_ns);
        if h != row.hash {
            changed.push((i, h));
        }
    }
    let [(changed_row, new_hash)] = changed[..] else {
        return Err(format!("the test path updates one changed file; {} changed", changed.len()));
    };
    let changed_path = state.str(state.stat[changed_row].path).to_vec();
    timer.phase(17, "change detection");

    // 4. The front over the edited inputs, the layout from the state.
    let diagnostics = Collector::new(crate::policy(plan));
    let config = StoreConfig {
        containers: mortar_odr::reads_containers(&plan.diagnostics.odr),
        ..StoreConfig::default()
    };
    let store =
        Store::with_config(StorePolicy::from_env(), config, mortar_input::store::Hooks::default());
    // The replay's two parts, timed apart (inside phase 90's wall time).
    let replay_ms = std::cell::Cell::new((0.0_f64, 0.0_f64));
    // IN-28a: by position unless MORTAR_FLOOR_IDENTITY is set.
    let by_position = std::env::var_os("MORTAR_FLOOR_IDENTITY").is_none();
    let laps = std::cell::RefCell::new(mortar_incr::capture::Laps::new());
    // What the replay translated (for the guard and the recapture), and
    // why it failed.
    let translated: std::cell::RefCell<Option<(Translation, Positional)>> =
        std::cell::RefCell::new(None);
    let failed: std::cell::RefCell<Option<String>> = std::cell::RefCell::new(None);
    let hook = |link: &mortar_incr::capture::Link<'_, '_>,
                computed: &mortar_core::layout::Layout| {
        let t0 = std::time::Instant::now();
        let r = (|| {
            let changed_id = link
                .inputs
                .link
                .opened
                .iter()
                .find_map(|o| match o.kind {
                    mortar_core::inputs::OpenedKind::Object(id) if o.path == &changed_path[..] => {
                        Some(id)
                    }
                    _ => None,
                })
                .ok_or("the changed file is not an object of this link")?;
            // IN-28a: the positional set first, from FILES, the new
            // link's files and change detection (step 3: every other file
            // is clean or hashed equal; m3 is checked after the front).
            let unchanged = |f: mortar_core::id::FileId| f != changed_id;
            let pos = mortar_incr::capture::positional_set(link, &state, &unchanged);
            let (layout, tr) = if by_position {
                // Identities for every file outside the set.
                let keys = Keys::partial(link, &|f| !pos.covers(f))?;
                let t1 = std::time::Instant::now();
                let r = mortar_incr::capture::replay_by_position(
                    link,
                    &keys,
                    &state,
                    computed,
                    &pos,
                    &mut laps.borrow_mut(),
                )?;
                replay_ms.set((ms(t1 - t0), ms(t1.elapsed())));
                r
            } else {
                let keys = Keys::new(link)?;
                let t1 = std::time::Instant::now();
                let r = mortar_incr::capture::replay_translated(
                    link,
                    &keys,
                    &state,
                    computed,
                    &mut laps.borrow_mut(),
                )?;
                replay_ms.set((ms(t1 - t0), ms(t1.elapsed())));
                r
            };
            *translated.borrow_mut() = Some((tr, pos));
            Ok::<_, String>(layout)
        })();
        if let Err(e) = &r {
            *failed.borrow_mut() = Some(e.clone());
        }
        r
    };
    // The state is an incremental-mode link's: its layout has IN-33's slack
    // and per-file CIEs (WP-062), and the replay reproduces that layout.
    let env = crate::front::Env { cwd, pool, replay: Some(&hook), slack: true };
    let front = crate::front::front(plan.clone(), env, &store, &diagnostics, timer);
    let Some(mut f) = front else {
        *report = Some(diagnostics.finish());
        return Err(match failed.borrow_mut().take() {
            Some(e) => format!("the replay failed: {e}"),
            None => "the front failed (a replay that does not fit is a changed layout)".to_owned(),
        });
    };
    f.keep_leaves = true;
    // The map's `arg` records (`spec/plumb.md` §3.2a), as a full link's.
    f.args = parsed.args.iter().map(|a| a.as_encoded_bytes().to_vec()).collect();
    let (keys_ms, replay_ms) = replay_ms.get();
    timer.line(format!("mortar: floor: replay keys: {keys_ms:.1} ms"));
    timer.line(format!("mortar: floor: replay translate: {replay_ms:.1} ms"));
    for (name, ms) in laps.borrow().iter() {
        timer.line(format!("mortar: floor: replay {name}: {ms:.1} ms"));
    }
    let (tr, pos) = translated.borrow_mut().take().ok_or("the replay did not run")?;
    timer.line(format!(
        "mortar: floor: positional files: {} of {}",
        if by_position { pos.len() } else { 0 },
        state.files.len()
    ));
    if f.dyn_tables.is_some() {
        return Err("incremental-unsupported (a dynamic executable)".to_owned());
    }
    let file = f
        .inputs
        .link
        .opened
        .iter()
        .find_map(|o| match o.kind {
            mortar_core::inputs::OpenedKind::Object(id) if o.path == &changed_path[..] => Some(id),
            _ => None,
        })
        .ok_or("the changed file is not an object of this link")?;
    // m3 (IN-28a condition 2): every file the front read is the file
    // change detection judged; the store's `fstat` after its read (§11.5)
    // equals the judged tuple.
    judged.sort_unstable_by_key(|t| (t.dev, t.ino));
    for (dev, ino) in store.inodes() {
        let Some(b) = store.blob_of_inode(dev, ino) else { continue };
        let m = store.meta(b);
        let at = judged.partition_point(|t| (t.dev, t.ino) < (dev, ino));
        let same = judged.get(at).is_some_and(|t| {
            (t.dev, t.ino, t.size) == (dev, ino, m.size)
                && t.mtime_ns == stamp::ns(m.mtime.0, m.mtime.1)
                && t.ctime_ns == stamp::ns(m.ctime.0, m.ctime.1)
        });
        if !same {
            return Err(format!(
                "input-changed: a file the update read (device {dev}, inode {ino}) is not the file change detection judged"
            ));
        }
    }
    let link = mortar_incr::capture::Link {
        inputs: &f.inputs,
        resolution: &f.resolution,
        assignment: &f.assignment,
        split: &f.merge.split,
    };
    let row = pos.row_of(file).ok_or("the changed file has no FILES row")?;
    // M4, m2 (WP-064a): the test path refills the changed file's units
    // only; anything it contributes that other units or the layout depend
    // on must be as the state stored it, decided before anything is
    // written.
    mortar_incr::contribution::compare(&link, &state, &pos, &tr, &f.layout, file, row)
        .map_err(|(reason, detail)| format!("{reason}: {detail}"))?;
    timer.phase(95, "changed file compared");

    // 5. The dirty windows, filled into memory.
    let pl = &f.layout.placement;
    // The ELF header first.
    let mut ranges: Vec<(&'static str, Range<u64>)> = Vec::new();
    ranges.push(("header", 0..64));
    for sid in f.resolution.section_base[file].iter() {
        let Some((c, off)) = pl.place.get(sid).and_then(|p| p.get()) else { continue };
        let chunk = &pl.chunks[c];
        if chunk.in_file() {
            let at = chunk.offset + off;
            ranges.push(("file", at..at + f.headers[sid].size));
        }
    }
    // WP-094 (b): the symbol tables and `.eh_frame` by the changed file's
    // ranges, unless MORTAR_FLOOR_WHOLE_TABLES is set.
    let per_file = std::env::var_os("MORTAR_FLOOR_WHOLE_TABLES").is_none();
    if per_file {
        per_file_ranges(&f, file, &mut ranges)?;
    }
    for (_, c) in pl.chunks.iter() {
        let whole = match c.kind {
            ChunkKind::Synthetic(s) => match s.name {
                b".symtab" | b".strtab" | b".symtab_shndx" if !per_file => Some("symtab"),
                b".eh_frame" if !per_file => Some("eh_frame"),
                b".eh_frame_hdr" => Some("eh_frame"),
                _ => None,
            },
            k => k
                .out()
                .is_some_and(|o| f.assignment.outsecs[o].name == b".debug_aranges")
                .then_some("aranges"),
        };
        if let Some(label) = whole.filter(|_| c.in_file()) {
            ranges.push((label, c.offset..c.offset + c.size));
        }
    }
    let m = &state.meta;
    if m.sealed_end > m.sealed_start {
        ranges.push(("build-id", m.sealed_start..m.sealed_end));
    }
    let w = mortar_image::IMAGE_BLOCK;
    let mut windows: Vec<u64> = Vec::new();
    // Each kind's windows (a window may hold several kinds).
    let mut by_kind: Vec<(&str, Vec<u64>)> = Vec::new();
    for (label, r) in ranges.iter().filter(|(_, r)| r.end > r.start) {
        let these = r.start / w..=(r.end - 1) / w;
        windows.extend(these.clone());
        match by_kind.iter_mut().find(|(l, _)| l == label) {
            Some((_, v)) => v.extend(these),
            None => by_kind.push((label, these.collect())),
        }
    }
    windows.sort_unstable();
    windows.dedup();
    // WP-094: the dirty bytes themselves (the ranges' union), per kind.
    let union = |mut rs: Vec<Range<u64>>| -> u64 {
        rs.sort_by_key(|r| r.start);
        let (mut total, mut end) = (0u64, 0u64);
        for r in rs {
            let start = r.start.max(end);
            if r.end > start {
                total += r.end - start;
                end = r.end;
            }
        }
        total
    };
    let dirty = union(ranges.iter().map(|(_, r)| r.clone()).collect());
    let kinds = by_kind
        .iter_mut()
        .map(|(l, v)| {
            v.sort_unstable();
            v.dedup();
            let bytes =
                union(ranges.iter().filter(|(k, _)| k == l).map(|(_, r)| r.clone()).collect());
            format!("{l}:{}/{bytes}", v.len())
        })
        .collect::<Vec<_>>()
        .join(",");
    let mut refilled = match crate::image::refill(&f, &store, &out_path, &windows) {
        Ok(r) => r,
        Err(e) => {
            for (k, d) in e.diagnostics {
                diagnostics.emit(k, d);
            }
            *report = Some(diagnostics.finish());
            return Err("the refill failed".to_owned());
        }
    };
    if refilled.file_size != state.meta.file_size {
        return Err("the output's size changed".to_owned());
    }
    let leaf_fn = refilled.leaf_fn;
    let stored_fn = match state.meta.leaf_fn {
        0 => TreeFn::L128,
        1 => TreeFn::Sha1,
        _ => TreeFn::Sha256,
    };
    if leaf_fn != stored_fn {
        return Err("the leaves' function changed".to_owned());
    }
    // I-76: the refilled leaves, the state's for the rest.
    let size = refilled.file_size;
    let mut column = LeafColumn::new(leaf_fn, size).map_err(|e| e.to_string())?;
    let mut fresh = vec![false; state.leaves.len()];
    for (k, leaf) in &refilled.leaves {
        column.set(*k, leaf).map_err(|e| e.to_string())?;
        if let Some(x) = fresh.get_mut(*k as usize) {
            *x = true;
        }
    }
    for (k, digest) in state.leaves.iter().enumerate() {
        if !fresh[k] {
            let leaf = Leaf::kept(leaf_fn, size, k as u64, digest)
                .ok_or("a stored leaf does not fit the output")?;
            column.set(k as u64, &leaf).map_err(|e| e.to_string())?;
        }
    }
    let build_id = match (refilled.build_id, &refilled.sealed) {
        (Some(style), Some(sealed)) => {
            let root = column.root().map_err(|e| e.to_string())?;
            let id = style.id(&root).map_err(|e| e.to_string())?.to_vec();
            let mut bytes = id.clone();
            bytes.resize((sealed.end - sealed.start) as usize, 0);
            let job = refilled
                .jobs
                .iter_mut()
                .find(|(r, _)| r.start <= sealed.start && sealed.end <= r.end)
                .ok_or("the build id's window was not refilled")?;
            let at = (sealed.start - job.0.start) as usize;
            job.1[at..at + bytes.len()].copy_from_slice(&bytes);
            id
        }
        _ => Vec::new(),
    };
    let summary_refilled: u64 = refilled.jobs.iter().map(|j| j.1.len() as u64).sum();
    // m4: the update's map, when its line asks for one: the records a full
    // link writes for the same output (made before anything is written).
    let machine_map = match plan.bookkeeping.mortar_map.as_deref() {
        Some(m) => {
            let map = crate::image::map_of(&f, &store)
                .map_err(|e| format!("the map: {e:?}"))?;
            Some((cwd.join(m), mortar_image::map::render(&map, &f.pool)))
        }
        None => None,
    };
    timer.phase(100, "image (dirty windows)");

    // 6. The writes (IN-46, IN-47), after everything was filled (IN-13).
    set_updating(&state_path).map_err(|e| format!("cannot set the updating flag: {e}"))?;
    // WP-094 (b): only the dirty ranges are written (the windows are
    // filled whole for their leaves), unless MORTAR_FLOOR_WRITE_WINDOWS.
    let writes: Vec<(Range<u64>, Vec<u8>)> = if std::env::var_os("MORTAR_FLOOR_WRITE_WINDOWS")
        .is_some()
    {
        refilled.jobs.clone()
    } else {
        let mut rs: Vec<Range<u64>> = ranges.iter().map(|(_, r)| r.clone()).collect();
        rs.sort_by_key(|r| r.start);
        let mut merged: Vec<Range<u64>> = Vec::new();
        for r in rs.into_iter().filter(|r| r.end > r.start) {
            match merged.last_mut() {
                Some(m) if r.start <= m.end => m.end = m.end.max(r.end),
                _ => merged.push(r),
            }
        }
        let mut out = Vec::new();
        for (jr, bytes) in &refilled.jobs {
            let first = merged.partition_point(|m| m.end <= jr.start);
            for m in merged[first..].iter().take_while(|m| m.start < jr.end) {
                let (a, b) = (m.start.max(jr.start), m.end.min(jr.end));
                out.push((a..b, bytes[(a - jr.start) as usize..(b - jr.start) as usize].to_vec()));
            }
        }
        out
    };
    let written_bytes: u64 = writes.iter().map(|w| w.0.end - w.0.start).sum();
    let written = match write {
        WritePath::InPlace => in_place(&out_path, &writes, size),
        WritePath::Copy => by_copy(&out_path, &writes, size),
    }
    .map_err(|e| format!("cannot write {}: {e}", out_path.display()))?;
    if let Some((path, text)) = &machine_map {
        mortar_image::output::write_whole(path, text, 0o666)
            .map_err(|_| format!("cannot write {}", path.display()))?;
    }
    timer.phase(105, "writes");

    // 7. The state, rewritten whole and bound to the output.
    for (k, leaf) in &refilled.leaves {
        if let Some(row) = state.leaves.get_mut(*k as usize) {
            *row = [0; 32];
            row[..leaf.as_bytes().len()].copy_from_slice(leaf.as_bytes());
        }
    }
    state.flags = 0;
    state.stat[changed_row].hash = new_hash;
    // FILES has one row per `FileId`, and a same-size edit keeps the ids.
    if state.files.len() != f.inputs.files.len() as usize {
        return Err("FILES does not have a row per file of this link".to_owned());
    }
    // IN-28a, the writer's duty: the changed file's position rows
    // recaptured from the bytes just hashed, or its bit cleared.
    if row != file.0 {
        return Err("FILES' row of the changed file is not its id's".to_owned());
    }
    state.files[file.0 as usize].hash = new_hash;
    let recaptured = mortar_incr::capture::recapture(&mut state, &link, &pos, &tr, file, row);
    timer.line(format!(
        "mortar: floor: the changed file's position rows {}",
        if recaptured { "recaptured" } else { "not recaptured: its POSITIONAL bit cleared" }
    ));
    // WP-105: the state's strings do not grow at every update. The build
    // id is overwritten in place when it is the pool's last string and of
    // the same length (no later string can share its bytes); a stamp's
    // directory and file-system type reuse an earlier device row's string.
    let old = state.meta.build_id;
    if old.len as usize == build_id.len()
        && old.off as usize + build_id.len() == state.strings.len()
        && !build_id.is_empty()
    {
        state.strings[old.off as usize..].copy_from_slice(&build_id);
    } else {
        state.meta.build_id = add_str(&mut state, &build_id);
    }
    let old_devices: Vec<(Str, Str)> = state
        .stat
        .iter()
        .filter(|r| r.tag == stat_tag::DEVICE)
        .map(|r| (r.path, r.fstype))
        .collect();
    let reuse = |state: &mut State, bytes: &[u8], pick: fn(&(Str, Str)) -> Str| {
        old_devices
            .iter()
            .map(pick)
            .find(|&x| state.str(x) == bytes && !bytes.is_empty())
            .unwrap_or_else(|| add_str(state, bytes))
    };
    let mut devices = Vec::new();
    for s in &stamps {
        let path = reuse(&mut state, s.dir.as_os_str().as_encoded_bytes(), |x| x.0);
        let fstype = reuse(&mut state, s.fs_type.as_bytes(), |x| x.1);
        devices.push(format::StatRow {
            tag: stat_tag::DEVICE,
            trust: u8::from(!s.trusted),
            path,
            dev: s.dev,
            stamp_ns: s.ctime_ns,
            fstype,
            ..format::StatRow::default()
        });
    }
    let (t, stamp_ns) = stamp::bind_output(&out_path)
        .map_err(|e| format!("cannot bind the output: {e}"))?
        .ok_or("the output changed while it was bound")?;
    if (t.dev, t.ino) != written || t.size != size {
        return Err("the output is no longer the file this update wrote".to_owned());
    }
    let mut stat: Vec<format::StatRow> = devices;
    stat.extend(state.stat.iter().filter(|r| r.tag == stat_tag::INPUT).cloned());
    stat.push(format::StatRow {
        tag: stat_tag::OUTPUT,
        trust: 0,
        dev: t.dev,
        ino: t.ino,
        size: t.size,
        mtime_ns: t.mtime_ns,
        ctime_ns: t.ctime_ns,
        stamp_ns,
        ..format::StatRow::default()
    });
    state.stat = stat;
    let encoded = format::encode(&state);
    incremental::write_atomically(&state_path, &encoded)
        .map_err(|e| format!("cannot write {}: {e}", state_path.display()))?;
    timer.phase(106, "state written");
    let diags = diagnostics.finish();
    if diags.failed() {
        *report = Some(diags);
        return Err("the link reported errors".to_owned());
    }
    Ok(Summary {
        changed: String::from_utf8_lossy(&changed_path).into_owned(),
        inputs,
        hashed,
        windows: windows.len(),
        jobs: refilled.jobs.len(),
        refilled: summary_refilled,
        dirty,
        written: written_bytes,
        file_size: size,
        state_size: encoded.len() as u64,
        kinds,
    })
}

/// WP-094 (b): the ranges of the symbol tables and `.eh_frame` that a
/// change of `file` can change: its `.symtab` block, its strings, the
/// entries and names of the globals it defines (S-88's per-file blocks),
/// and its `.eh_frame` range (IN-34a's per-file ranges in incremental
/// mode), with the padding after it.
fn per_file_ranges(
    f: &crate::front::Front<'_>,
    file: mortar_core::id::FileId,
    ranges: &mut Vec<(&'static str, Range<u64>)>,
) -> Result<(), String> {
    let pl = &f.layout.placement;
    let chunk = |name: &[u8]| {
        pl.chunks
            .iter()
            .find(|(_, c)| matches!(c.kind, ChunkKind::Synthetic(s) if s.name == name))
            .map(|(_, c)| c)
            .filter(|c| c.in_file())
    };
    if let Some(plan) = &f.synth.symtab {
        let (entries, strings) = f
            .with_sym_cx(|cx| {
                let defined: Vec<mortar_core::id::SymbolId> = f
                    .inputs
                    .sym_map_of(file)
                    .iter()
                    .copied()
                    .filter(|&id| cx.syms.global(id).is_some_and(|g| g.file == file))
                    .collect();
                plan.file_entries(cx, file, &defined)
            })
            .and_then(|r| r)
            .map_err(|e| format!("the symbol table's ranges: {e:?}"))?;
        let es = plan.entry_size();
        if let Some(c) = chunk(b".symtab") {
            for e in &entries {
                let r = c.offset + u64::from(e.start) * es..c.offset + u64::from(e.end) * es;
                ranges.push(("symtab", r));
            }
        }
        if let Some(c) = chunk(b".symtab_shndx") {
            for e in &entries {
                let r = c.offset + u64::from(e.start) * 4..c.offset + u64::from(e.end) * 4;
                ranges.push(("symtab", r));
            }
        }
        if let Some(c) = chunk(b".strtab") {
            for r in &strings {
                ranges.push(("strtab", c.offset + r.start..c.offset + r.end));
            }
        }
    }
    if let Some(c) = chunk(b".eh_frame") {
        let recs = f.eh.records();
        let mine: Vec<usize> = (0..recs.len())
            .filter(|&n| f.resolution.sh_index(recs[n].section).is_some_and(|(x, _)| x == file))
            .collect();
        if let (Some(&lo), Some(&hi)) = (mine.first(), mine.last()) {
            if hi - lo + 1 != mine.len() {
                return Err("the changed file's .eh_frame records are not one range".to_owned());
            }
            ranges.push((
                "eh_frame",
                c.offset + f.eh.out_offset(lo)..c.offset + f.eh.out_offset(hi + 1),
            ));
        }
    }
    Ok(())
}

/// IN-46 step 1 (and IN-47's): the state's updating flag, set by a
/// positioned write of its header's flags word, and synced.
fn set_updating(state_path: &Path) -> std::io::Result<()> {
    let f = OpenOptions::new().write(true).open(state_path)?;
    f.write_all_at(&format::FLAG_UPDATING.to_le_bytes(), format::FLAGS_OFFSET)?;
    f.sync_data()
}

fn sync_dir(path: &Path) -> std::io::Result<()> {
    let dir = match path.parent() {
        Some(p) if !p.as_os_str().is_empty() => p.to_path_buf(),
        _ => PathBuf::from("."),
    };
    File::open(dir)?.sync_all()
}

/// IN-46 steps 2-4: `e_ident` zeroed and synced, the windows written
/// and synced, the true header written and synced. The output's device
/// and inode.
fn in_place(
    out_path: &Path,
    jobs: &[(Range<u64>, Vec<u8>)],
    size: u64,
) -> std::io::Result<(u64, u64)> {
    const IDENT: usize = 16;
    let f = OpenOptions::new().read(true).write(true).open(out_path)?;
    let meta = f.metadata()?;
    if !meta.is_file() || meta.nlink() != 1 {
        return Err(std::io::Error::other("not a regular file with one link (IN-45)"));
    }
    let head = jobs
        .iter()
        .find(|(r, _)| r.start == 0)
        .map(|(_, b)| b)
        .filter(|b| b.len() >= IDENT)
        .ok_or_else(|| std::io::Error::other("the ELF header's window was not refilled"))?;
    f.write_all_at(&[0; IDENT], 0)?;
    f.sync_data()?;
    for (r, b) in jobs {
        if r.start == 0 {
            f.write_all_at(&b[IDENT..], IDENT as u64)?;
        } else {
            f.write_all_at(b, r.start)?;
        }
    }
    if meta.len() != size {
        f.set_len(size)?;
    }
    f.sync_data()?;
    f.write_all_at(&head[..IDENT], 0)?;
    f.sync_data()?;
    Ok((meta.dev(), meta.ino()))
}

/// IN-47: `P` copied to `T` beside it, the windows written into `T`,
/// `T` synced and renamed over `P`, the directory synced. `T`'s device
/// and inode.
fn by_copy(
    out_path: &Path,
    jobs: &[(Range<u64>, Vec<u8>)],
    size: u64,
) -> std::io::Result<(u64, u64)> {
    // I-58b: the output is replaced only where the link may replace it.
    mortar_image::output::may_replace(out_path).map_err(std::io::Error::other)?;
    let mut tmp = out_path.as_os_str().to_owned();
    tmp.push(format!(".mortar-update-{}", std::process::id()));
    let tmp = PathBuf::from(tmp);
    let result = (|| {
        let mut t = OpenOptions::new().read(true).write(true).create_new(true).open(&tmp)?;
        let mut p = File::open(out_path)?;
        t.set_permissions(p.metadata()?.permissions())?;
        std::io::copy(&mut p, &mut t)?;
        for (r, b) in jobs {
            t.write_all_at(b, r.start)?;
        }
        if t.metadata()?.len() != size {
            t.set_len(size)?;
        }
        t.flush()?;
        t.sync_data()?;
        let m = t.metadata()?;
        std::fs::rename(&tmp, out_path)?;
        sync_dir(out_path)?;
        Ok((m.dev(), m.ino()))
    })();
    if result.is_err() {
        let _ = std::fs::remove_file(&tmp);
    }
    result
}
