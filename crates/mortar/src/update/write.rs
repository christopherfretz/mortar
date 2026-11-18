//! The update's writes and the crash protocol (`spec/incr.md` IN-76,
//! IN-77, IN-77a..f, IN-45; IN-70 step U10). WP-213 (U-1): the types
//! [`Pair`], [`Written`] and the entry point [`write`]; filled by U-7
//! (WP-219). `WriteOps` lives in `mortar_incr::ops` (U-1 moved it there:
//! the state's patch in `mortar_incr` takes it too).
//!
//! What [`write`] does, in order (each file operation through `ops`;
//! reads and `stat`s are not operations and go to `std`):
//! 1. the checks that need nothing written (IN-13): `META` read, the
//!    ranges well formed, `P`'s descriptor still the size `META` gives,
//!    `S` hard-linked or a symbolic link only with a compacting plan;
//! 2. IN-77d's leftovers (`A`, `T`, the compaction's temporary) removed
//!    by their exact names, each only after a `try_lock` on it succeeds;
//! 3. the path (IN-76 and IN-45's conditions, cheapest first);
//! 4. W1..W6 (in place) or C1..C5 (copy), each failure ending the update
//!    at once with IN-77e's clean-up, no sync retried.
//!
//! A plain link never reaches this file (IN-11).

use std::ffi::OsString;
use std::fs::File;
use std::io;
use std::os::fd::{AsRawFd, RawFd};
use std::os::unix::fs::{FileExt, MetadataExt};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use mortar_core::incr::{DirtyRange, IncrReason, Unimplemented};
use mortar_incr::format::{self, StatRow, stat_tag};
use mortar_incr::ops::{LockMode, WriteFailure, WriteOps};
use mortar_incr::stamp::{self, Tuple};
use mortar_incr::state5::{self, PatchPlan};
use mortar_incr::testhook::{crash_point, dirty_ranges};
use mortar_incr::update::rows::MetaRow;
use mortar_incr::update::view::StateView;
use mortar_incr::{WritePath, WritePolicy};

/// The output and the state as U1 opened them (IN-77a: one descriptor per
/// file, never reopened by path; the paths are for the directory
/// operations: the aside name `A`, `T`, the renames).
#[derive(Debug, Clone, Copy)]
pub struct Pair<'a> {
    /// `P`'s descriptor (read-write in place, read-only for the copy) and
    /// its path.
    pub out: &'a File,
    pub out_path: &'a Path,
    /// `S`'s descriptor (read-write, locked, IN-77b) and its path.
    pub state: &'a File,
    pub state_path: &'a Path,
}

/// What the writes did (for the report: IN-76 "the report says which",
/// IN-46b `delta state compacted`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Written {
    /// The new output's tuple (W4's or C3's binding, IN-77a).
    pub tuple: Tuple,
    /// The path taken.
    pub path: WritePath,
    /// W5/C4 was the compaction (IN-18f).
    pub compacted: bool,
}

/// The aside name's suffix: `A = dir(P)/.<base(P)>.mortar-torn` (IN-77).
pub(crate) const ASIDE: &str = ".mortar-torn";
/// The copy path's temporary: `T = dir(P)/.<base(P)>.mortar-tmp` (image.md
/// I-59's `T0`; IN-77's C1).
pub(crate) const TEMPORARY: &str = ".mortar-tmp";
/// The compaction's temporary: `dir(S)/.<base(S)>.mortar-compact`
/// (IN-77, IN-77d).
pub(crate) const COMPACT: &str = ".mortar-compact";

/// `dir(path)/.<base(path)><suffix>`: the name of one of IN-77d's
/// temporaries.
#[must_use]
pub(crate) fn beside(path: &Path, suffix: &str) -> PathBuf {
    let mut name = OsString::from(".");
    name.push(path.file_name().unwrap_or_default());
    name.push(suffix);
    dir_of(path).join(name)
}

fn dir_of(path: &Path) -> PathBuf {
    match path.parent() {
        Some(p) if !p.as_os_str().is_empty() => p.to_path_buf(),
        _ => PathBuf::from("."),
    }
}

/// Write the update (WP-211 §C; IN-77's W1..W6 or C1..C5): `ranges` with
/// their `bytes` (concatenated in range order) into the output, then the
/// state's patch the join planned before W1 (`plan`, from
/// `mortar_incr::state5::plan_patch` in U9) at W5/C4:
/// `mortar_incr::state5::patch`, or `compact` when `plan.compacts()`,
/// with W4's or C3's binding; every call through `ops`. Before W3's first
/// write it hands `ranges` to `mortar_incr::testhook::dirty_ranges`, and it
/// calls `mortar_incr::testhook::crash_point` at each of IN-77's points. `policy` gives the default path (the join
/// passes `mortar_incr::WRITE_PATH`, or a policy of `Copy` on both sides
/// for `--incremental-write=copy`); IN-45's conditions are checked here
/// but one: "`P` is not one of the inputs" is image.md I-62's
/// `written-file-clash`, which the link decides before U10 (no input
/// list reaches this function).
///
/// **The new output's size** (W3 "truncate or extend"): no argument
/// carries it (WP-219's finding S-1); until one does, it is the larger of
/// `META`'s output size and the end of the last range, so an output that
/// grows is extended and one that would shrink is NOT truncated.
///
/// # Errors
/// [`WriteFailure`] (IN-77e): `Fallback` for a named reason found before
/// anything it forbids was written (`output-changed` before W2/C3 and
/// W2's foreign file; `state-corrupt` from `META`); `Failed` for an I/O
/// error, and for W4's `EEXIST` (the link fails with `output-changed`).
pub fn write(
    pair: &Pair<'_>,
    view: &StateView<'_>,
    ranges: &[DirtyRange],
    bytes: &[u8],
    plan: &PatchPlan,
    ops: &mut dyn WriteOps,
    policy: WritePolicy,
) -> Result<Written, WriteFailure> {
    let compacts = plan.compacts();
    let state_path = pair.state_path;
    write_with(pair, view, ranges, bytes, compacts, ops, policy, &mut |output, ops| {
        if compacts {
            state5::compact(view, plan, output, ops, state_path)
        } else {
            state5::patch(view, plan, output, ops)
        }
    })
}

#[doc(hidden)]
/// W5/C4's writer as [`write_with`] takes it: the binding W4 or C3 took,
/// and the operations (U-7 wraps them for the crash points inside W5).
pub type StateWriter<'w> =
    dyn FnMut(&StatRow, &mut dyn WriteOps) -> Result<(), WriteFailure> + 'w;

/// [`write`] with W5/C4's writer given (`w5`, called once with W4's or
/// C3's `STAT` output row) instead of `state5::patch`/`compact`, and the
/// plan's one question (`compacts`) as a value: U-7's tests drive the
/// protocol through it with a stub state while U-2's `patch` returns its
/// "until it lands" value (the supervisor's ruling for WP-219). The join
/// calls [`write`].
///
/// # Errors
/// As [`write`].
#[doc(hidden)]
#[allow(clippy::too_many_arguments)]
pub fn write_with(
    pair: &Pair<'_>,
    view: &StateView<'_>,
    ranges: &[DirtyRange],
    bytes: &[u8],
    compacts: bool,
    ops: &mut dyn WriteOps,
    policy: WritePolicy,
    w5: &mut StateWriter<'_>,
) -> Result<Written, WriteFailure> {
    // 1. What needs nothing written (IN-13: a fall-back is decided before
    //    W1).
    let meta = view.meta()?;
    let Some(new_size) = sized(meta.file_size, ranges, bytes) else {
        debug_assert!(false, "dirty ranges not sorted, disjoint and as long as their bytes");
        return Err(WriteFailure::Fallback(IncrReason::UpdateNotImplemented(Some(
            Unimplemented::Write,
        ))));
    };
    let size = pair.out.metadata().map_err(|e| failed("before W1", e))?.len();
    if size != meta.file_size {
        return Err(changed(format!(
            "{} is {size} bytes, the state says {} (IN-77a)",
            pair.out_path.display(),
            meta.file_size
        )));
    }
    if !compacts && state_shared(pair) {
        // IN-77b: such an `S` is never patched; `plan_patch` decides the
        // compaction for it. A plan that patches it is not written.
        return Err(WriteFailure::Fallback(IncrReason::UpdateNotImplemented(Some(
            Unimplemented::Write,
        ))));
    }

    // 2. IN-77d: the leftovers, by exact name, before any write of ours.
    let aside = beside(pair.out_path, ASIDE);
    let aside_clear = remove_leftover(ops, &aside);
    remove_leftover(ops, &beside(pair.out_path, TEMPORARY));
    remove_leftover(ops, &beside(pair.state_path, COMPACT));

    // The descriptor's tuple once the leftovers are gone (removing an `A`
    // that is a second name of `P`, a kill inside W4's leftover, changes
    // `P`'s link count and ctime): what IN-77a's checks compare with.
    let out_meta = pair.out.metadata().map_err(|e| failed("before W1", e))?;
    let start = Tuple::of(&out_meta);
    if start.size != meta.file_size {
        return Err(changed(format!("{} changed size (IN-77a)", pair.out_path.display())));
    }

    // 3. IN-76: the path.
    let path = if policy.default_for(meta.file_size) == WritePath::InPlace
        && in_place_blocked(pair, &out_meta, &meta, aside_clear).is_none()
    {
        WritePath::InPlace
    } else {
        WritePath::Copy
    };

    // 4. The protocol.
    let job = Job { pair, ranges, bytes, start, new_size, compacts };
    let tuple = match path {
        WritePath::InPlace => job.in_place(ops, &aside, w5)?,
        WritePath::Copy => job.by_copy(ops, w5)?,
    };
    Ok(Written { tuple, path, compacted: compacts })
}

fn failed(step: &'static str, error: io::Error) -> WriteFailure {
    WriteFailure::Failed { step, error }
}

fn changed(why: String) -> WriteFailure {
    WriteFailure::Fallback(IncrReason::OutputChanged(why))
}

/// The new output's size, or `None` when the ranges are not sorted,
/// disjoint and exactly as long as `bytes` (U-6's contract, IN-75).
fn sized(old: u64, ranges: &[DirtyRange], bytes: &[u8]) -> Option<u64> {
    let mut end = 0u64;
    let mut total = 0u64;
    for r in ranges {
        if r.offset < end {
            return None;
        }
        end = r.offset.checked_add(r.len)?;
        total = total.checked_add(r.len)?;
    }
    (total == u64::try_from(bytes.len()).ok()?).then_some(old.max(end))
}

/// IN-77b: `S` with other hard links or reached through a symbolic link.
fn state_shared(pair: &Pair<'_>) -> bool {
    let linked = pair.state.metadata().map_or(true, |m| m.nlink() != 1);
    let symbolic = std::fs::symlink_metadata(pair.state_path).map_or(true, |m| m.is_symlink());
    linked || symbolic
}

/// `(dev, ino)` of the file `path` names (not following a symbolic link).
fn identity(path: &Path) -> Option<(u64, u64)> {
    std::fs::symlink_metadata(path).ok().map(|m| (m.dev(), m.ino()))
}

/// IN-77d: removes the leftover `path` when it is a regular file whose
/// `try_lock` succeeds and which the name still names once locked. True
/// when no file is left at `path`.
fn remove_leftover(ops: &mut dyn WriteOps, path: &Path) -> bool {
    match std::fs::symlink_metadata(path) {
        Err(e) if e.kind() == io::ErrorKind::NotFound => return true,
        Ok(m) if m.is_file() => {}
        _ => return false,
    }
    let Ok(file) = File::open(path) else { return false };
    if !matches!(ops.flock(&file, LockMode::TryExclusive), Ok(true)) {
        return false; // held: a live link's
    }
    let locked = file.metadata().map(|m| (m.dev(), m.ino())).ok();
    if locked.is_none() || identity(path) != locked {
        return false;
    }
    ops.unlink(path).is_ok()
}

/// IN-45's conditions for in place, cheapest first; the first that fails
/// (the update then takes the copy path, IN-76: not a fall-back).
fn in_place_blocked(
    pair: &Pair<'_>,
    out: &std::fs::Metadata,
    meta: &MetaRow,
    aside_clear: bool,
) -> Option<&'static str> {
    if !std::fs::symlink_metadata(pair.out_path).is_ok_and(|m| m.file_type().is_file()) {
        return Some("the output is not a regular file (a symbolic link)");
    }
    if out.nlink() != 1 {
        return Some("the output has other hard links");
    }
    if meta.output_kind != 0 {
        return Some("the output is not an executable");
    }
    if !writable(pair.out) {
        return Some("the output could not be opened for writing");
    }
    if !aside_clear {
        return Some("a leftover aside name is held by a live link");
    }
    if seen_elsewhere(out.dev(), out.ino()) {
        return Some("another process maps the output or holds it open");
    }
    None
}

/// Whether `file` was opened for writing (U1 opens `P` read-write for in
/// place; where the open failed, `ETXTBSY` included, it opens it
/// read-only): its access mode in `/proc/self/fdinfo`. Unknown: not.
fn writable(file: &File) -> bool {
    let fd: RawFd = file.as_raw_fd();
    let Ok(text) = std::fs::read_to_string(format!("/proc/self/fdinfo/{fd}")) else {
        return false;
    };
    text.lines()
        .find_map(|l| l.strip_prefix("flags:"))
        .and_then(|v| u32::from_str_radix(v.trim(), 8).ok())
        .is_some_and(|flags| matches!(flags & 3, 1 | 2))
}

/// IN-45: a process other than this one, visible in `/proc`, maps the
/// file `(dev, ino)` (`/proc/*/maps`) or holds it open (`/proc/*/fd`).
/// Without `/proc` nothing can be seen: treated as seen (the copy path).
fn seen_elsewhere(dev: u64, ino: u64) -> bool {
    let Ok(procs) = std::fs::read_dir("/proc") else { return true };
    let me = std::process::id().to_string();
    let (major, minor) = stamp::major_minor(dev);
    let want_dev = format!("{major:02x}:{minor:02x}");
    let want_ino = ino.to_string();
    for entry in procs.flatten() {
        let name = entry.file_name();
        let Some(pid) = name.to_str() else { continue };
        if pid == me || pid.is_empty() || !pid.bytes().all(|b| b.is_ascii_digit()) {
            continue;
        }
        let base = entry.path();
        if let Ok(maps) = std::fs::read_to_string(base.join("maps"))
            && maps.lines().any(|l| {
                let mut f = l.split_ascii_whitespace();
                f.nth(3).is_some_and(|d| d.eq_ignore_ascii_case(&want_dev))
                    && f.next() == Some(want_ino.as_str())
            })
        {
            return true;
        }
        if let Ok(fds) = std::fs::read_dir(base.join("fd")) {
            for fd in fds.flatten() {
                if std::fs::metadata(fd.path()).is_ok_and(|m| m.dev() == dev && m.ino() == ino) {
                    return true;
                }
            }
        }
    }
    false
}

/// The flag (IN-77: `S`'s updating word, one aligned 4-byte word at the
/// format's `FLAGS_OFFSET`, outside every hash), written and synced: W1,
/// C2 (set) and W6, C5 (clear).
fn flag(ops: &mut dyn WriteOps, state: &File, set: bool) -> io::Result<()> {
    let word = if set { format::FLAG_UPDATING } else { 0 };
    ops.pwrite(state, format::FLAGS_OFFSET, &word.to_le_bytes())?;
    ops.fdatasync(state)
}

/// `e_ident`'s length: what W2 zeroes and W4 writes back.
const IDENT: u64 = 16;

/// The binding of IN-21 through a descriptor (IN-77a; review-in77 M-8,
/// m-5): `fstat`, stamps in `dir` until one is later than the file's
/// ctime, `fstat` again. `None` when the two differ (something wrote the
/// file meanwhile) or no later stamp came within 2 s.
fn bind(file: &File, dir: &Path) -> io::Result<Option<StatRow>> {
    let first = Tuple::of(&file.metadata()?);
    let begun = Instant::now();
    let stamp_ns = loop {
        let (dev, ctime) = stamp::take(dir)?;
        if dev != first.dev {
            return Ok(None);
        }
        if ctime > first.ctime_ns {
            break ctime;
        }
        if begun.elapsed() > Duration::from_secs(2) {
            return Ok(None);
        }
        std::thread::sleep(Duration::from_micros(20));
    };
    let second = Tuple::of(&file.metadata()?);
    Ok((first == second).then(|| StatRow {
        tag: stat_tag::OUTPUT,
        trust: 0,
        dev: second.dev,
        ino: second.ino,
        size: second.size,
        mtime_ns: second.mtime_ns,
        ctime_ns: second.ctime_ns,
        stamp_ns,
        ..StatRow::default()
    }))
}

/// The `ops` W5/C4 writes through: every call forwarded, with the crash
/// points that fall inside U-2's `patch` and `compact` placed by the call
/// they follow (IN-77 "Crash hooks"): `W5-written` before the patch's
/// first sync of `S`; `compact-written` before the compaction's rename
/// (its temporary written and synced), `compact-renamed` after it.
struct Hooked<'o> {
    inner: &'o mut dyn WriteOps,
    state: RawFd,
    compacts: bool,
    synced: bool,
}

impl WriteOps for Hooked<'_> {
    fn pwrite(&mut self, file: &File, offset: u64, bytes: &[u8]) -> io::Result<()> {
        self.inner.pwrite(file, offset, bytes)
    }
    fn set_len(&mut self, file: &File, len: u64) -> io::Result<()> {
        self.inner.set_len(file, len)
    }
    fn fdatasync(&mut self, file: &File) -> io::Result<()> {
        if !self.compacts && !self.synced && file.as_raw_fd() == self.state {
            self.synced = true;
            crash_point("W5-written");
        }
        self.inner.fdatasync(file)
    }
    fn fsync(&mut self, file: &File) -> io::Result<()> {
        if !self.compacts && !self.synced && file.as_raw_fd() == self.state {
            self.synced = true;
            crash_point("W5-written");
        }
        self.inner.fsync(file)
    }
    fn rename(&mut self, from: &Path, to: &Path) -> io::Result<()> {
        if self.compacts {
            crash_point("compact-written");
        }
        self.inner.rename(from, to)?;
        if self.compacts {
            crash_point("compact-renamed");
        }
        Ok(())
    }
    fn hard_link(&mut self, from: &Path, to: &Path) -> io::Result<()> {
        self.inner.hard_link(from, to)
    }
    fn unlink(&mut self, path: &Path) -> io::Result<()> {
        self.inner.unlink(path)
    }
    fn fsync_dir(&mut self, dir: &Path) -> io::Result<()> {
        self.inner.fsync_dir(dir)
    }
    fn flock(&mut self, file: &File, mode: LockMode) -> io::Result<bool> {
        self.inner.flock(file, mode)
    }
    fn create_new(&mut self, path: &Path) -> io::Result<File> {
        self.inner.create_new(path)
    }
    fn copy_range(&mut self, from: &File, to: &File, len: u64) -> io::Result<()> {
        self.inner.copy_range(from, to, len)
    }
}

/// One update's writes.
struct Job<'p, 'r> {
    pair: &'p Pair<'p>,
    ranges: &'r [DirtyRange],
    bytes: &'r [u8],
    /// `P`'s descriptor's tuple at the start (U1 bound it to the state).
    start: Tuple,
    new_size: u64,
    compacts: bool,
}

/// Where the in-place update is, for IN-77e's clean-up.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Stage {
    /// Only `S` written (W1): nothing to remove.
    Flagged,
    /// `P` is at `A` (W2..W4 before the link).
    Aside,
    /// `P` and `A` name our file (W4 between the link and the unlink).
    Linked,
    /// `P` names our file (from W4's unlink on).
    Back,
}

impl Job<'_, '_> {
    /// Each range with its bytes.
    fn pieces(&self) -> impl Iterator<Item = (u64, &[u8])> {
        let mut at = 0usize;
        self.ranges.iter().map(move |r| {
            let n = usize::try_from(r.len).unwrap_or(usize::MAX);
            let piece = &self.bytes[at..at + n];
            at += n;
            (r.offset, piece)
        })
    }

    /// The file's length after the ranges are written over `start.size`
    /// bytes: when it is not the new size, W3/C1 truncate or extend.
    fn written_len(&self) -> u64 {
        self.ranges.last().map_or(self.start.size, |r| self.start.size.max(r.offset + r.len))
    }

    /// The true `e_ident`: the output's own, under the ranges' bytes.
    fn true_ident(&self) -> io::Result<[u8; 16]> {
        let mut ident = [0u8; 16];
        self.pair.out.read_exact_at(&mut ident, 0)?;
        for (offset, piece) in self.pieces().take_while(|(o, _)| *o < IDENT) {
            for (i, b) in piece.iter().enumerate() {
                let at = offset + i as u64;
                if at >= IDENT {
                    break;
                }
                ident[at as usize] = *b;
            }
        }
        Ok(ident)
    }

    /// IN-77a, immediately before W2 or C3: `P` by path is still the file
    /// U1 checked (its `(dev, ino)` and size, and the times of the
    /// descriptor's `fstat` at the start: nothing wrote it since).
    fn replaced(&self) -> Option<String> {
        let p = self.pair.out_path.display();
        match std::fs::metadata(self.pair.out_path) {
            Err(e) => Some(format!("{p}: {e}")),
            Ok(m) if Tuple::of(&m) != self.start => {
                Some(format!("{p} was replaced or written after the update checked it (IN-77a)"))
            }
            Ok(_) => None,
        }
    }

    /// IN-77e in place: from W2 on, our file at `A` (and at `P` after
    /// W4's link) is removed; a name that names another file is left.
    fn clean(&self, ops: &mut dyn WriteOps, aside: &Path, stage: Stage) {
        let ours = Some((self.start.dev, self.start.ino));
        let mut remove = |path: &Path| {
            if identity(path) == ours {
                let _ = ops.unlink(path);
            }
        };
        match stage {
            Stage::Flagged => {}
            Stage::Aside => remove(aside),
            Stage::Linked => {
                remove(self.pair.out_path);
                remove(aside);
            }
            Stage::Back => remove(self.pair.out_path),
        }
    }

    /// IN-77's W1..W6.
    fn in_place(
        &self,
        ops: &mut dyn WriteOps,
        aside: &Path,
        w5: &mut StateWriter<'_>,
    ) -> Result<Tuple, WriteFailure> {
        let (out, out_path, state) = (self.pair.out, self.pair.out_path, self.pair.state);
        let dir = dir_of(out_path);
        let ident = self.true_ident().map_err(|e| failed("before W1", e))?;

        // W1
        flag(ops, state, true).map_err(|e| failed("W1", e))?;
        crash_point("W1");

        // IN-77a's check, then `A`'s creator's lock (IN-77d).
        if let Some(why) = self.replaced() {
            return Err(changed(why));
        }
        match ops.flock(out, LockMode::TryExclusive) {
            Ok(true) => {}
            Ok(false) => {
                return Err(changed(format!("{} is locked by another link", out_path.display())));
            }
            Err(e) => return Err(failed("W2", e)),
        }

        // W2
        ops.rename(out_path, aside).map_err(|e| failed("W2", e))?;
        crash_point("W2-renamed");
        if identity(aside) != Some((self.start.dev, self.start.ino)) {
            // A foreign file was at `P`: put back with `link`, never
            // `rename` (W4's form); on `EEXIST` a newer file is at `P`.
            if identity(aside).is_some() {
                match ops.hard_link(aside, out_path) {
                    Ok(()) => {
                        let _ = ops.unlink(aside);
                    }
                    Err(e) if e.kind() == io::ErrorKind::AlreadyExists => {
                        let _ = ops.unlink(aside);
                    }
                    Err(_) => {} // left at `A`: not ours to remove
                }
                let _ = ops.fsync_dir(&dir);
            }
            return Err(changed(format!(
                "{} named another file when it was set aside (IN-77 W2)",
                out_path.display()
            )));
        }
        let step = |ops: &mut dyn WriteOps, r: io::Result<()>, name, stage| {
            r.map_err(|e| {
                self.clean(ops, aside, stage);
                failed(name, e)
            })
        };
        let r = ops.fsync_dir(&dir);
        step(ops, r, "W2", Stage::Aside)?;
        let r = ops.pwrite(out, 0, &[0; IDENT as usize]);
        step(ops, r, "W2", Stage::Aside)?;
        crash_point("W2-written");
        let r = ops.fdatasync(out);
        step(ops, r, "W2", Stage::Aside)?;
        crash_point("W2");

        // W3
        dirty_ranges(self.ranges);
        for (offset, piece) in self.pieces() {
            let skip = usize::try_from(IDENT.saturating_sub(offset)).unwrap_or(usize::MAX);
            if skip >= piece.len() {
                continue; // inside `e_ident`: W4's
            }
            let r = ops.pwrite(out, offset.max(IDENT), &piece[skip..]);
            step(ops, r, "W3", Stage::Aside)?;
        }
        if self.written_len() != self.new_size {
            let r = ops.set_len(out, self.new_size);
            step(ops, r, "W3", Stage::Aside)?;
        }
        crash_point("W3-written");
        let r = ops.fdatasync(out);
        step(ops, r, "W3", Stage::Aside)?;
        crash_point("W3");

        // W4
        let r = ops.pwrite(out, 0, &ident);
        step(ops, r, "W4", Stage::Aside)?;
        crash_point("W4-written");
        let r = ops.fsync(out);
        step(ops, r, "W4", Stage::Aside)?;
        match ops.hard_link(aside, out_path) {
            Ok(()) => {}
            Err(e) if e.kind() == io::ErrorKind::AlreadyExists => {
                // Another link put a file at `P` while it named nothing:
                // it stays; `A` is removed; the flag stays set; the link
                // fails and writes nothing more.
                self.clean(ops, aside, Stage::Aside);
                let _ = ops.fsync_dir(&dir);
                return Err(failed(
                    "W4",
                    io::Error::new(
                        io::ErrorKind::AlreadyExists,
                        format!(
                            "output-changed: {} was written by another link while the update \
                             had it aside (IN-77 W4)",
                            out_path.display()
                        ),
                    ),
                ));
            }
            Err(e) => {
                self.clean(ops, aside, Stage::Aside);
                return Err(failed("W4", e));
            }
        }
        crash_point("W4-linked");
        let r = ops.fsync_dir(&dir);
        step(ops, r, "W4", Stage::Linked)?;
        let r = ops.unlink(aside);
        step(ops, r, "W4", Stage::Linked)?;
        crash_point("W4-unlinked");
        let r = ops.fsync_dir(&dir);
        step(ops, r, "W4", Stage::Back)?;
        let row = match bind(out, &dir) {
            Ok(Some(row)) => row,
            Ok(None) => {
                return Err(changed(format!(
                    "{} changed while the update bound it (IN-77a)",
                    out_path.display()
                )));
            }
            Err(e) => {
                self.clean(ops, aside, Stage::Back);
                return Err(failed("W4", e));
            }
        };
        crash_point("W4");

        // W5, W6
        self.state_steps(ops, &row, w5, ("W5", "W6"), |ops| self.clean(ops, aside, Stage::Back))?;
        Ok(tuple_of(&row))
    }

    /// W5/C4 (U-2's writer, hooked) and W6/C5 (the flag cleared, unless
    /// the compaction's rename replaced it). `clean` is IN-77e's clean-up
    /// for a failure here.
    fn state_steps(
        &self,
        ops: &mut dyn WriteOps,
        row: &StatRow,
        w5: &mut StateWriter<'_>,
        (five, six): (&'static str, &'static str),
        clean: impl Fn(&mut dyn WriteOps),
    ) -> Result<(), WriteFailure> {
        let r = {
            let mut hooked = Hooked {
                inner: &mut *ops,
                state: self.pair.state.as_raw_fd(),
                compacts: self.compacts,
                synced: false,
            };
            w5(row, &mut hooked)
        };
        if let Err(e) = r {
            clean(ops);
            return Err(e);
        }
        crash_point(five);
        if !self.compacts
            && let Err(e) = flag(ops, self.pair.state, false)
        {
            clean(ops);
            return Err(failed(six, e));
        }
        crash_point(six);
        Ok(())
    }

    /// Creates `T` (I-59's `T0`, else `T0.<pid>` when `T0` exists: a live
    /// link's), locked by this link and still named by its path once
    /// locked. Any other failure ends C1 (IN-77e: no retry); a file this
    /// call created and could not lock is removed.
    fn temporary(&self, ops: &mut dyn WriteOps) -> io::Result<(PathBuf, File)> {
        let t0 = beside(self.pair.out_path, TEMPORARY);
        let mut with_pid = t0.clone().into_os_string();
        with_pid.push(format!(".{}", std::process::id()));
        let mut last = io::Error::other("no temporary");
        for path in [t0, PathBuf::from(with_pid)] {
            let t = match ops.create_new(&path) {
                Ok(t) => t,
                Err(e) if e.kind() == io::ErrorKind::AlreadyExists => {
                    last = e;
                    continue;
                }
                Err(e) => return Err(e),
            };
            let id = t.metadata().map(|m| (m.dev(), m.ino()))?;
            match ops.flock(&t, LockMode::TryExclusive) {
                Ok(true) if identity(&path) == Some(id) => return Ok((path, t)),
                // Another link removed or took it between the create and
                // the lock: not ours any more.
                Ok(_) => {
                    last = io::Error::other(format!("{} was taken by another link", path.display()));
                }
                Err(e) => {
                    if identity(&path) == Some(id) {
                        let _ = ops.unlink(&path);
                    }
                    return Err(e);
                }
            }
        }
        Err(last)
    }

    /// IN-77's C1..C5.
    fn by_copy(
        &self,
        ops: &mut dyn WriteOps,
        w5: &mut StateWriter<'_>,
    ) -> Result<Tuple, WriteFailure> {
        let (out, out_path, state) = (self.pair.out, self.pair.out_path, self.pair.state);
        let dir = dir_of(out_path);

        // C1
        let (t_path, t) = self.temporary(ops).map_err(|e| failed("C1", e))?;
        let drop_t = |ops: &mut dyn WriteOps| {
            if identity(&t_path) == t.metadata().ok().map(|m| (m.dev(), m.ino())) {
                let _ = ops.unlink(&t_path);
            }
        };
        let step = |ops: &mut dyn WriteOps, r: io::Result<()>, name| {
            r.map_err(|e| {
                drop_t(ops);
                failed(name, e)
            })
        };
        let r = out.metadata().and_then(|m| t.set_permissions(m.permissions()));
        step(ops, r, "C1")?;
        dirty_ranges(self.ranges);
        let r = ops.copy_range(out, &t, self.start.size);
        step(ops, r, "C1")?;
        for (offset, piece) in self.pieces() {
            let r = ops.pwrite(&t, offset, piece);
            step(ops, r, "C1")?;
        }
        if self.written_len() != self.new_size {
            let r = ops.set_len(&t, self.new_size);
            step(ops, r, "C1")?;
        }
        crash_point("C1-written");
        let r = ops.fdatasync(&t);
        step(ops, r, "C1")?;
        crash_point("C1");

        // C2
        let r = flag(ops, state, true);
        step(ops, r, "C2")?;
        crash_point("C2");

        // C3
        if let Some(why) = self.replaced() {
            drop_t(ops);
            return Err(changed(why));
        }
        let r = ops.rename(&t_path, out_path);
        step(ops, r, "C3")?;
        ops.fsync_dir(&dir).map_err(|e| failed("C3", e))?;
        let row = match bind(&t, &dir) {
            Ok(Some(row)) => row,
            Ok(None) => {
                return Err(changed(format!(
                    "{} changed while the update bound it (IN-77a)",
                    out_path.display()
                )));
            }
            Err(e) => return Err(failed("C3", e)),
        };
        crash_point("C3");

        // C4, C5: `P` is the old or the new file and is kept (IN-77e).
        self.state_steps(ops, &row, w5, ("C4", "C5"), |_| {})?;
        Ok(tuple_of(&row))
    }
}

fn tuple_of(row: &StatRow) -> Tuple {
    Tuple {
        dev: row.dev,
        ino: row.ino,
        size: row.size,
        mtime_ns: row.mtime_ns,
        ctime_ns: row.ctime_ns,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // spec: incr.md IN-77 (the aside name), IN-77d (the temporaries' names)
    #[test]
    fn the_temporaries_names() {
        assert_eq!(beside(Path::new("/d/out"), ASIDE), Path::new("/d/.out.mortar-torn"));
        assert_eq!(beside(Path::new("out"), TEMPORARY), Path::new("./.out.mortar-tmp"));
        assert_eq!(
            beside(Path::new("/d/out.mortar-state"), COMPACT),
            Path::new("/d/.out.mortar-state.mortar-compact")
        );
    }

    // spec: incr.md IN-75 (the ranges' contract), IN-77 W3 (the size)
    #[test]
    fn ranges_and_the_new_size() {
        let r = |offset, len| DirtyRange { offset, len };
        assert_eq!(sized(100, &[], &[]), Some(100));
        assert_eq!(sized(100, &[r(0, 4), r(96, 8)], &[0; 12]), Some(104));
        assert_eq!(sized(100, &[r(0, 4), r(2, 4)], &[0; 8]), None, "overlap");
        assert_eq!(sized(100, &[r(8, 4), r(0, 4)], &[0; 8]), None, "unsorted");
        assert_eq!(sized(100, &[r(0, 4)], &[0; 5]), None, "bytes too long");
    }
}
