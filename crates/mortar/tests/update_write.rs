//! U-7's tests (WP-219; `spec/incr.md` IN-76, IN-77, IN-77a..e, IN-45):
//! the update's writes and the crash protocol, tested alone against a
//! stub state (U-2's `state5::patch` and `compact` still return their
//! "until it lands" value): `write_with` drives the protocol with a stub
//! W5 that writes a small checksummed state the way U-2's patch will
//! (blocks, then the header past the flag word, then one sync).
//!
//! Every test works in its own directory under the system temporary
//! directory (`TMPDIR`; never the workspace: `flock`, hard links,
//! `EEXIST`, timestamps and kills), removed on drop. The kill tests
//! (`MORTAR_INCR_CRASH`) need `--features testhooks`.

use std::fs::File;
use std::io;
use std::os::unix::fs::{FileExt, MetadataExt, PermissionsExt};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::{PoisonError, RwLock, RwLockReadGuard, RwLockWriteGuard};

use mortar::update::write::{
    Pair, Written, boot_check, boot_id, lock_state, write, write_state_whole, write_with,
};
use mortar_core::incr::{DirtyRange, IncrReason, Unimplemented};
use mortar_incr::format::{FLAG_UPDATING, FLAGS_OFFSET, StatRow};
use mortar_incr::ops::{Disk, LockMode, Op, RecordingOps, StdOps, WriteFailure, WriteOps};
use mortar_incr::stamp::{self, Cleanliness, Tuple};
use mortar_incr::state5::PatchPlan;
use mortar_incr::update::testing::MemoryState;
use mortar_incr::{SMALL_OUTPUT, WRITE_PATH, WritePath, WritePolicy};

// ---- the scratch directory --------------------------------------------------

/// A directory of the system temporary directory, removed on drop.
struct Tmp(PathBuf);

impl Tmp {
    fn new(label: &str) -> Self {
        static N: AtomicU32 = AtomicU32::new(0);
        let n = N.fetch_add(1, Ordering::Relaxed);
        let path =
            std::env::temp_dir().join(format!("mortar-wp219-{label}-{}-{n}", std::process::id()));
        std::fs::create_dir_all(&path).unwrap();
        Self(path)
    }
}

impl Drop for Tmp {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

// ---- children and updates in one test process ------------------------------------

/// Between its fork and its exec a child holds a copy of every descriptor
/// of this process, `P`'s among them, and IN-45's `/proc` scan then sees
/// `P` held by another process: the update rightly takes the copy path.
/// So a child is spawned only while no update runs here (an update holds
/// [`updating`]; a spawn holds [`forking`] until the child has exec'd,
/// or, for `output`/`status`, until it ends).
static FORKS: RwLock<()> = RwLock::new(());

fn updating() -> RwLockReadGuard<'static, ()> {
    FORKS.read().unwrap_or_else(PoisonError::into_inner)
}

fn forking() -> RwLockWriteGuard<'static, ()> {
    FORKS.write().unwrap_or_else(PoisonError::into_inner)
}

/// `Command`'s spawns, each under [`forking`].
trait Fork {
    fn fork_spawn(&mut self) -> io::Result<std::process::Child>;
    fn fork_output(&mut self) -> io::Result<std::process::Output>;
    fn fork_status(&mut self) -> io::Result<std::process::ExitStatus>;
}

impl Fork for std::process::Command {
    fn fork_spawn(&mut self) -> io::Result<std::process::Child> {
        let _g = forking();
        self.spawn()
    }

    fn fork_output(&mut self) -> io::Result<std::process::Output> {
        let _g = forking();
        self.output()
    }

    fn fork_status(&mut self) -> io::Result<std::process::ExitStatus> {
        let _g = forking();
        self.status()
    }
}

// ---- the world: an output, its edit, a stub state ---------------------------

const P_LEN: usize = 8192;

/// The previous output: an "ELF" magic, then a pattern.
fn p_old() -> Vec<u8> {
    let mut v: Vec<u8> = (0..P_LEN).map(|i| (i * 7 % 251) as u8).collect();
    v[..4].copy_from_slice(b"\x7fELF");
    v
}

/// How the edit changes the output's size.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Size {
    Same,
    Grow,
    /// W3/C1 truncate (image.md I-97; WP-219 S-1).
    Shrink,
}

impl From<bool> for Size {
    fn from(grow: bool) -> Self {
        if grow { Self::Grow } else { Self::Same }
    }
}

/// The size a shrinking edit leaves.
const SHRUNK: usize = 7000;

/// The update's dirty ranges and their bytes: the header (a new
/// `e_ident` past the magic), a window, and the end (grown, the same, or
/// cut), with the new output's size.
fn edit(size: Size) -> (Vec<DirtyRange>, Vec<u8>, u64) {
    let mut ranges = Vec::new();
    let mut bytes = Vec::new();
    let mut add = |offset: u64, len: u64, fill: u8| {
        ranges.push(DirtyRange { offset, len });
        bytes.extend(std::iter::repeat_n(fill, len as usize));
    };
    add(0, 64, 0x11);
    add(4096, 100, 0x22);
    let len = match size {
        Size::Grow => {
            add(P_LEN as u64 - 50, 350, 0x33);
            P_LEN as u64 + 300
        }
        Size::Same => {
            add(8000, 100, 0x33);
            P_LEN as u64
        }
        Size::Shrink => {
            add(6000, 100, 0x33);
            SHRUNK as u64
        }
    };
    bytes[..4].copy_from_slice(b"\x7fELF");
    (ranges, bytes, len)
}

fn apply(old: &[u8], ranges: &[DirtyRange], bytes: &[u8], len: u64) -> Vec<u8> {
    let mut v = old.to_vec();
    let mut at = 0;
    for r in ranges {
        let (o, n) = (r.offset as usize, r.len as usize);
        if v.len() < o + n {
            v.resize(o + n, 0);
        }
        v[o..o + n].copy_from_slice(&bytes[at..at + n]);
        at += n;
    }
    v.resize(len as usize, 0);
    v
}

/// The stub state (U-2's format stands in): `MORTARSR`, version, the flag
/// word at `FLAGS_OFFSET`, the binding `(dev, ino, size)`, a body, and a
/// checksum over everything past the flag word.
const S_LEN: usize = 112;
const OLD_BODY: u8 = 0xaa;
const NEW_BODY: u8 = 0xbb;
/// The body of a state another (full) link wrote.
const OTHER_BODY: u8 = 0xcc;

fn fnv(bytes: &[u8]) -> u64 {
    bytes
        .iter()
        .fold(0xcbf2_9ce4_8422_2325u64, |h, b| (h ^ u64::from(*b)).wrapping_mul(0x100_0000_01b3))
}

fn s_image(dev: u64, ino: u64, size: u64, body: u8, flag: u32) -> Vec<u8> {
    let mut s = vec![0u8; S_LEN];
    s[..8].copy_from_slice(b"MORTARSR");
    s[8..12].copy_from_slice(&5u32.to_le_bytes());
    s[12..16].copy_from_slice(&flag.to_le_bytes());
    s[16..24].copy_from_slice(&dev.to_le_bytes());
    s[24..32].copy_from_slice(&ino.to_le_bytes());
    s[32..40].copy_from_slice(&size.to_le_bytes());
    s[40..104].fill(body);
    let sum = fnv(&s[16..104]);
    s[104..112].copy_from_slice(&sum.to_le_bytes());
    s
}

fn u64_at(s: &[u8], at: usize) -> u64 {
    u64::from_le_bytes(s[at..at + 8].try_into().unwrap())
}

fn w5_failed(e: io::Error) -> WriteFailure {
    WriteFailure::Failed { step: "W5", error: e }
}

/// W5's stub patch: the body's two blocks, the header past the flag, the
/// checksum, one sync (U-2's patch: blocks, block list, header 16.., sync).
fn stub_patch(state: &File, row: &StatRow, ops: &mut dyn WriteOps) -> Result<(), WriteFailure> {
    let img = s_image(row.dev, row.ino, row.size, NEW_BODY, 0);
    ops.pwrite(state, 40, &img[40..72]).map_err(w5_failed)?;
    ops.pwrite(state, 72, &img[72..104]).map_err(w5_failed)?;
    ops.pwrite(state, 16, &img[16..40]).map_err(w5_failed)?;
    ops.pwrite(state, 104, &img[104..112]).map_err(w5_failed)?;
    ops.fdatasync(state).map_err(w5_failed)
}

/// The compaction's stub: the whole new state, flag clear, to
/// `.<base(S)>.mortar-compact`, synced, renamed over `S`, the directory
/// synced (IN-77, IN-18f).
fn stub_compact(s: &Path, row: &StatRow, ops: &mut dyn WriteOps) -> Result<(), WriteFailure> {
    let tmp = beside(s, ".mortar-compact");
    let fail = |e| WriteFailure::Failed { step: "compact", error: e };
    let file = ops.create_new(&tmp).map_err(fail)?;
    let id = file.metadata().map(|m| (m.dev(), m.ino())).map_err(fail)?;
    // IN-77d (WP-219 F-8): the name must still name the locked file.
    let ours = |p: &Path| std::fs::metadata(p).ok().map(|m| (m.dev(), m.ino())) == Some(id);
    match ops.flock(&file, LockMode::TryExclusive) {
        Ok(true) if ours(&tmp) => {}
        Ok(_) => return Err(fail(io::Error::other("the temporary is another link's (IN-77d)"))),
        Err(e) => {
            if ours(&tmp) {
                let _ = ops.unlink(&tmp);
            }
            return Err(fail(e));
        }
    }
    let img = s_image(row.dev, row.ino, row.size, NEW_BODY, 0);
    let res = ops.pwrite(&file, 0, &img).and_then(|()| ops.fdatasync(&file));
    if let Err(e) = res {
        if ours(&tmp) {
            let _ = ops.unlink(&tmp);
        }
        return Err(fail(e));
    }
    if let Err(e) = ops.rename(&tmp, s) {
        if ours(&tmp) {
            let _ = ops.unlink(&tmp);
        }
        return Err(fail(e));
    }
    ops.fsync_dir(s.parent().unwrap()).map_err(fail)
}

fn beside(path: &Path, suffix: &str) -> PathBuf {
    let mut name = std::ffi::OsString::from(".");
    name.push(path.file_name().unwrap());
    name.push(suffix);
    path.parent().unwrap().join(name)
}

/// One output and its state in a directory, and the edit to write.
struct World {
    dir: PathBuf,
    p: PathBuf,
    s: PathBuf,
    a: PathBuf,
    t: PathBuf,
    c: PathBuf,
    p_old: Vec<u8>,
    s_old: Vec<u8>,
    ranges: Vec<DirtyRange>,
    bytes: Vec<u8>,
    p_new: Vec<u8>,
    /// The new output's size (`Pair::size`).
    size: u64,
}

impl World {
    /// The names in `dir` (the child of a kill test sees the same).
    fn names(dir: &Path, size: impl Into<Size>) -> Self {
        let p = dir.join("out");
        let s = dir.join("out.mortar-state");
        let (ranges, bytes, size) = edit(size.into());
        let p_old = p_old();
        let p_new = apply(&p_old, &ranges, &bytes, size);
        Self {
            dir: dir.to_path_buf(),
            a: beside(&p, ".mortar-torn"),
            t: beside(&p, ".mortar-tmp"),
            c: beside(&s, ".mortar-compact"),
            p,
            s,
            p_old,
            s_old: Vec::new(),
            ranges,
            bytes,
            p_new,
            size,
        }
    }

    /// Creates `P` (mode 755) and its state bound to it.
    fn create(dir: &Path, size: impl Into<Size>) -> Self {
        let mut w = Self::names(dir, size);
        std::fs::write(&w.p, &w.p_old).unwrap();
        std::fs::set_permissions(&w.p, std::fs::Permissions::from_mode(0o755)).unwrap();
        let m = std::fs::metadata(&w.p).unwrap();
        w.s_old = s_image(m.dev(), m.ino(), m.size(), OLD_BODY, 0);
        std::fs::write(&w.s, &w.s_old).unwrap();
        w
    }

    /// U1's descriptors: `P` read-write (in place) or read-only, `S`
    /// read-write.
    fn open(&self, writable: bool) -> (File, File) {
        let out = File::options().read(true).write(writable).open(&self.p).unwrap();
        let state = File::options().read(true).write(true).open(&self.s).unwrap();
        (out, state)
    }

    /// The pair as U1 hands it to `write`: the descriptors, U1's `fstat`
    /// of `P` now, the new size.
    fn pair<'a>(&'a self, out: &'a File, state: &'a File) -> Pair<'a> {
        let checked = Tuple::of(&out.metadata().unwrap());
        Pair { out, out_path: &self.p, state, state_path: &self.s, checked, size: self.size }
    }

    fn view(&self) -> MemoryState {
        let mut m = MemoryState::default();
        m.meta.file_size = self.p_old.len() as u64;
        m.meta.output_kind = 0;
        m
    }
}

fn policy(path: WritePath) -> WritePolicy {
    WritePolicy { in_place_from: 0, at_or_above: path, below: path }
}

/// What the next link makes of a pair (U1's order: the flag first, then
/// the state's checksum, then the binding through `P`), and whether `P`
/// at its path is a file the update could leave there.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Verdict {
    /// Flag clear, checksum good, bound to `P`: the old pair.
    OldPair,
    /// The same, the new pair.
    NewPair,
    /// A named reason: a full link.
    Named(&'static str),
    /// Another link's pair (its output `foreign`, its state bound to it
    /// with `OTHER_BODY`): consistent, and not the update's.
    OtherPair,
}

/// The real files: `P` or `S` by path.
fn real(p: &Path) -> Option<((u64, u64), Vec<u8>)> {
    let m = std::fs::metadata(p).ok()?;
    Some(((m.dev(), m.ino()), std::fs::read(p).ok()?))
}

type Reader<'r> = dyn Fn(&Path) -> Option<((u64, u64), Vec<u8>)> + 'r;

/// The judge of IN-77 "Why never a silent mix": `Err` for a trusted mix
/// (a state trusted with an output it does not describe) or a torn `P`
/// at its path.
fn judge(w: &World, read: &Reader<'_>, foreign: Option<&[u8]>) -> Result<Verdict, String> {
    let p = read(&w.p);
    if let Some((_, bytes)) = &p
        && *bytes != w.p_old
        && *bytes != w.p_new
        && Some(bytes.as_slice()) != foreign
    {
        return Err(format!("a torn output at P ({} bytes)", bytes.len()));
    }
    let Some((_, s)) = read(&w.s) else { return Ok(Verdict::Named("no-state")) };
    if s.len() != S_LEN || &s[..8] != b"MORTARSR" {
        return Ok(Verdict::Named("state-corrupt"));
    }
    if u32::from_le_bytes(s[12..16].try_into().unwrap()) != 0 {
        return Ok(Verdict::Named("output-torn"));
    }
    if u64_at(&s, 104) != fnv(&s[16..104]) {
        return Ok(Verdict::Named("state-corrupt"));
    }
    let Some((id, bytes)) = p else { return Ok(Verdict::Named("output-changed")) };
    if id != (u64_at(&s, 16), u64_at(&s, 24)) || bytes.len() as u64 != u64_at(&s, 32) {
        return Ok(Verdict::Named("output-changed"));
    }
    match (s[40], bytes == w.p_old, bytes == w.p_new) {
        (OLD_BODY, true, _) => Ok(Verdict::OldPair),
        (NEW_BODY, _, true) => Ok(Verdict::NewPair),
        (OTHER_BODY, _, _) if Some(bytes.as_slice()) == foreign => Ok(Verdict::OtherPair),
        (body, old, new) => {
            Err(format!("a trusted mix: state body {body:#x} with an output old {old}, new {new}"))
        }
    }
}

/// Runs the update with `ops` and the stub W5; `Ok` with what it wrote.
fn run(
    w: &World,
    out: &File,
    state: &File,
    path: WritePath,
    compacts: bool,
    ops: &mut dyn WriteOps,
    rows: &mut Vec<StatRow>,
) -> Result<Written, WriteFailure> {
    let pair = w.pair(out, state);
    let m = w.view();
    let s = w.s.clone();
    let _u = updating();
    write_with(
        &pair,
        &m.view(),
        &w.ranges,
        &w.bytes,
        compacts,
        ops,
        policy(path),
        &mut |row, ops| {
            rows.push(row.clone());
            if compacts { stub_compact(&s, row, ops) } else { stub_patch(state, row, ops) }
        },
    )
}

// ---- the entry point --------------------------------------------------------

// spec: incr.md IN-77e (the empty plan is refused before W1: W5 would
// write no binding and W6 would clear the flag over a state still bound
// to the old output; nothing is written, the old pair stays)
#[test]
fn write_refuses_the_empty_plan_before_w1() {
    let tmp = Tmp::new("entry");
    let w = World::create(&tmp.0, false);
    let (out, state) = w.open(true);
    let pair = w.pair(&out, &state);
    let m = w.view();
    let mut ops = RecordingOps::default();
    let r = write(
        &pair,
        &m.view(),
        &w.ranges,
        &w.bytes,
        &PatchPlan::default(),
        &mut ops,
        policy(WritePath::InPlace),
    );
    assert!(matches!(r, Err(WriteFailure::Fallback(IncrReason::StateCorrupt(_)))), "{r:?}");
    assert!(ops.log.is_empty(), "no operation before the refusal: {:?}", ops.log);
    assert_eq!(judge(&w, &real, None), Ok(Verdict::OldPair));
}

// ---- power loss: every prefix and subset ------------------------------------

/// Records one update, then replays every power loss onto byte buffers
/// and judges each; the verdicts seen, by kind.
fn power_loss(path: WritePath, compacts: bool, size: Size) -> Vec<(usize, Verdict)> {
    let tmp = Tmp::new("power");
    let w = World::create(&tmp.0, size);
    let (out, state) = w.open(path == WritePath::InPlace);
    let mut ops = RecordingOps::default();
    let mut rows = Vec::new();
    let r = run(&w, &out, &state, path, compacts, &mut ops, &mut rows).unwrap();
    assert_eq!((r.path, r.compacted), (path, compacts));
    assert_eq!(judge(&w, &real, None), Ok(Verdict::NewPair), "the real files after it");
    let (p_id, s_id) = (ops.file_of(&out).unwrap(), ops.file_of(&state).unwrap());
    let mut before = Disk::default();
    before.names.insert(w.p.clone(), p_id);
    before.names.insert(w.s.clone(), s_id);
    before.data.insert(p_id, w.p_old.clone());
    before.data.insert(s_id, w.s_old.clone());
    let files = ops.files.clone();
    let mut seen = Vec::new();
    let mut bad = Vec::new();
    let n = ops
        .power_loss(&before, 12, |k, d: &Disk| {
            let read = |p: &Path| {
                let id = *d.names.get(p)?;
                Some((files[id as usize], d.data.get(&id)?.clone()))
            };
            match judge(&w, &read, None) {
                Ok(v) => seen.push((k, v)),
                Err(e) => {
                    bad.push(format!("prefix {k} ({:?}): {e}", ops.log.get(k.wrapping_sub(1))));
                }
            }
            // IN-77 "Why never a silent mix": a torn state has its flag set.
            if let Some(s) = d.read(&w.s) {
                let torn =
                    s[40..104].iter().any(|b| *b != s[40]) || fnv(&s[16..104]) != u64_at(s, 104);
                let flag = u32::from_le_bytes(s[12..16].try_into().unwrap());
                if torn && flag == 0 {
                    bad.push(format!("prefix {k}: a torn state with its flag clear"));
                }
            }
        })
        .unwrap();
    assert!(bad.is_empty(), "{} of {n} outcomes:\n{}", bad.len(), bad.join("\n"));
    eprintln!(
        "power loss {path:?} compacts {compacts} size {size:?}: {} calls, {n} outcomes",
        ops.log.len()
    );
    assert!(seen.iter().any(|(_, v)| *v == Verdict::OldPair), "the old pair is reachable");
    assert!(seen.iter().any(|(_, v)| *v == Verdict::NewPair), "the new pair is reachable");
    assert_eq!(seen.last().map(|x| x.1), Some(Verdict::NewPair), "the whole log: the new pair");
    seen
}

// spec: incr.md IN-77 (power loss after every prefix with every subset of
// the unsynced calls: the old pair, the new pair or a named reason, never
// a trusted mix and never a torn output at P), in place and by copy, with
// and without the compaction, a same-size, a growing and a shrinking edit
// (W3/C1 truncate: WP-219 S-1); IN-77e/F-4 (the prefix ending at W6's or
// C5's flag write, with and without that write durable: a failed final
// sync); IN-77
// "Why never a silent mix" (a torn state has its flag set: the next link
// says `output-torn` before any hash)
#[test]
fn power_loss_every_prefix_and_subset() {
    for path in [WritePath::InPlace, WritePath::Copy] {
        for compacts in [false, true] {
            for size in [Size::Same, Size::Grow, Size::Shrink] {
                let seen = power_loss(path, compacts, size);
                let torn = seen.iter().filter(|(_, v)| *v == Verdict::Named("output-torn")).count();
                assert!(torn > 0, "{path:?} {compacts} {size:?}: the flag is seen set");
            }
        }
    }
}

// spec: incr.md IN-77 "Why never a silent mix", IN-18c (a state whose
// blocks are torn has its flag set at every point of W5, so the next link
// reads `output-torn` first)
#[test]
fn torn_state_with_flag_is_output_torn() {
    let seen = power_loss(WritePath::InPlace, false, Size::Same);
    assert!(
        !seen.iter().any(|(_, v)| *v == Verdict::Named("state-corrupt")),
        "a torn state is never judged by its checksum: its flag is set"
    );
}

// ---- the binding (W4, C3) ----------------------------------------------------

// spec: incr.md IN-77a (W4's and C3's tuple through the descriptor, by
// IN-21's binding: the stamp later than the output's ctime), review-in77
// M-8, m-5: the next link finds the output clean, not racy
#[test]
fn output_not_racy_after_update() {
    for path in [WritePath::InPlace, WritePath::Copy] {
        let tmp = Tmp::new("racy");
        let w = World::create(&tmp.0, true);
        let (out, state) = w.open(path == WritePath::InPlace);
        let mut rows = Vec::new();
        let r = run(&w, &out, &state, path, false, &mut StdOps, &mut rows).unwrap();
        let row = &rows[0];
        let now = Tuple::of(&std::fs::metadata(&w.p).unwrap());
        assert_eq!(r.tuple, now, "{path:?}: the tuple returned is P's");
        assert_eq!(
            (row.dev, row.ino, row.size, row.mtime_ns, row.ctime_ns),
            (now.dev, now.ino, now.size, now.mtime_ns, now.ctime_ns)
        );
        assert!(row.stamp_ns > row.ctime_ns, "{path:?}: the stamp is later than the ctime");
        assert_eq!(stamp::classify(&r.tuple, &now, Some(row.stamp_ns)), Cleanliness::Clean);
        assert_eq!(r.path == WritePath::InPlace, now.ino == u64_at(&w.s_old, 24), "{path:?}");
        assert_eq!(std::fs::read(&w.p).unwrap(), w.p_new);
    }
}

// ---- a recording that fails on demand ----------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Kind {
    Pwrite,
    SetLen,
    Fdatasync,
    Fsync,
    Rename,
    HardLink,
    Unlink,
    FsyncDir,
    Flock,
    CreateNew,
    CopyRange,
}

/// [`RecordingOps`] with every attempt counted, a failure injected at
/// attempt `fail` (with `errno`), and `after` called after each successful
/// call (attempt, kind).
struct Faulty<'f> {
    rec: RecordingOps,
    calls: Vec<Kind>,
    fail: Option<(usize, i32)>,
    after: Box<dyn FnMut(usize, Kind) + 'f>,
    /// Every `try_lock` succeeds: a file system that does not enforce
    /// `flock` (image.md I-59's probe sees it).
    unenforced: bool,
}

impl<'f> Faulty<'f> {
    fn new(fail: Option<(usize, i32)>, after: impl FnMut(usize, Kind) + 'f) -> Self {
        Self {
            rec: RecordingOps::default(),
            calls: Vec::new(),
            fail,
            after: Box::new(after),
            unenforced: false,
        }
    }

    fn go<T>(
        &mut self,
        kind: Kind,
        f: impl FnOnce(&mut RecordingOps) -> io::Result<T>,
    ) -> io::Result<T> {
        let n = self.calls.len();
        self.calls.push(kind);
        if let Some((at, errno)) = self.fail
            && at == n
        {
            return Err(io::Error::from_raw_os_error(errno));
        }
        let r = f(&mut self.rec)?;
        (self.after)(n, kind);
        Ok(r)
    }
}

impl WriteOps for Faulty<'_> {
    fn pwrite(&mut self, file: &File, offset: u64, bytes: &[u8]) -> io::Result<()> {
        self.go(Kind::Pwrite, |r| r.pwrite(file, offset, bytes))
    }
    fn set_len(&mut self, file: &File, len: u64) -> io::Result<()> {
        self.go(Kind::SetLen, |r| r.set_len(file, len))
    }
    fn fdatasync(&mut self, file: &File) -> io::Result<()> {
        self.go(Kind::Fdatasync, |r| r.fdatasync(file))
    }
    fn fsync(&mut self, file: &File) -> io::Result<()> {
        self.go(Kind::Fsync, |r| r.fsync(file))
    }
    fn rename(&mut self, from: &Path, to: &Path) -> io::Result<()> {
        self.go(Kind::Rename, |r| r.rename(from, to))
    }
    fn hard_link(&mut self, from: &Path, to: &Path) -> io::Result<()> {
        self.go(Kind::HardLink, |r| r.hard_link(from, to))
    }
    fn unlink(&mut self, path: &Path) -> io::Result<()> {
        self.go(Kind::Unlink, |r| r.unlink(path))
    }
    fn fsync_dir(&mut self, dir: &Path) -> io::Result<()> {
        self.go(Kind::FsyncDir, |r| r.fsync_dir(dir))
    }
    fn flock(&mut self, file: &File, mode: LockMode) -> io::Result<bool> {
        let fake = self.unenforced && matches!(mode, LockMode::TryExclusive);
        self.go(Kind::Flock, |r| if fake { Ok(true) } else { r.flock(file, mode) })
    }
    fn create_new(&mut self, path: &Path) -> io::Result<File> {
        self.go(Kind::CreateNew, |r| r.create_new(path))
    }
    fn copy_range(&mut self, from: &File, to: &File, len: u64) -> io::Result<()> {
        self.go(Kind::CopyRange, |r| r.copy_range(from, to, len))
    }
}

/// The calls of a clean update (to inject at each).
fn clean_calls(path: WritePath, compacts: bool) -> Vec<Kind> {
    let tmp = Tmp::new("calls");
    let w = World::create(&tmp.0, true);
    let (out, state) = w.open(path == WritePath::InPlace);
    let mut ops = Faulty::new(None, |_, _| {});
    run(&w, &out, &state, path, compacts, &mut ops, &mut Vec::new()).unwrap();
    ops.calls
}

const EIO: i32 = 5;
const ENOSPC: i32 = 28;

/// IN-77e's outcome of a failure at attempt `at`: the update failed by
/// name of its step, nothing but removals after it, and the pair as the
/// rule leaves it. The verdict of the next link.
fn failure_at(path: WritePath, compacts: bool, at: usize, errno: i32) -> (Verdict, &'static str) {
    let tmp = Tmp::new("fail");
    let w = World::create(&tmp.0, true);
    let (out, state) = w.open(path == WritePath::InPlace);
    let mut ops = Faulty::new(Some((at, errno)), |_, _| {});
    let r = run(&w, &out, &state, path, compacts, &mut ops, &mut Vec::new());
    let Err(WriteFailure::Failed { step, error }) = r else {
        panic!("{path:?} {compacts} at {at} ({:?}): {r:?}", ops.calls.get(at))
    };
    assert_eq!(error.raw_os_error(), Some(errno), "{path:?} at {at}: the error itself");
    let after: Vec<Kind> = ops.calls[at + 1..].to_vec();
    assert!(
        after.iter().all(|k| *k == Kind::Unlink),
        "{path:?} {compacts} at {at} ({step}): IN-77e, nothing but removals after a failure: {after:?}"
    );
    let v = judge(&w, &real, None)
        .unwrap_or_else(|e| panic!("{path:?} {compacts} at {at} ({step}): {e}"));
    // W6's and C5's sync failing, or the compaction's directory sync after
    // its rename: the cleared word (the new state) is already live (WP-219
    // finding F-4): the next link sees it clear, and finds the new pair
    // (copy) or `P` removed (`output-changed`).
    let six = (matches!(step, "W6" | "C5") && ops.calls[at] == Kind::Fdatasync)
        || (step == "compact" && ops.calls[at] == Kind::FsyncDir);
    assert!(
        matches!(v, Verdict::OldPair | Verdict::Named("output-torn"))
            || (six && matches!(v, Verdict::NewPair | Verdict::Named("output-changed"))),
        "{path:?} {compacts} at {at} ({step}): {v:?}"
    );
    // The temporaries: `A` removed (in place), `T` removed (copy), the
    // compaction's removed.
    assert!(!w.a.exists(), "{path:?} at {at} ({step}): A is removed");
    let t_pid = {
        let mut t = w.t.clone().into_os_string();
        t.push(format!(".{}", std::process::id()));
        PathBuf::from(t)
    };
    assert!(!w.t.exists() && !t_pid.exists(), "{path:?} at {at} ({step}): T is removed");
    assert!(!w.c.exists(), "{path:?} at {at} ({step}): the compaction's temporary is removed");
    match (path, v) {
        // In place, a failure from W2 on removes our file: P is gone
        // after W4's link (W4..W6), or still the old one before W2's rename.
        (WritePath::InPlace, Verdict::Named(_)) => {
            if let Ok(b) = std::fs::read(&w.p) {
                assert_eq!(b, w.p_old, "{path:?} at {at} ({step}): P is the old file or none");
            }
        }
        // By copy P is the old or the new file and is kept.
        (WritePath::Copy, _) => assert!(w.p.exists(), "{path:?} at {at} ({step}): P is kept"),
        _ => {}
    }
    drop(tmp);
    (v, step)
}

// spec: incr.md IN-77e (a failed sync ends the update at once: it is not
// retried, and nothing but IN-77e's removals follows it), every sync of
// both paths, with and without the compaction
#[test]
fn failed_sync_is_not_retried() {
    for path in [WritePath::InPlace, WritePath::Copy] {
        for compacts in [false, true] {
            let calls = clean_calls(path, compacts);
            let syncs: Vec<usize> = (0..calls.len())
                .filter(|i| matches!(calls[*i], Kind::Fdatasync | Kind::Fsync | Kind::FsyncDir))
                .collect();
            assert!(syncs.len() >= 4, "{path:?}: {calls:?}");
            for at in syncs {
                failure_at(path, compacts, at, EIO);
            }
        }
    }
}

// spec: incr.md IN-77e (any failure of a write, a sync, a rename or a
// `set_len`: before W1/C2 the old pair, from W1/C2 on the flag set and the
// next link a full link; in place the torn file removed, by copy P kept),
// at every call of both paths
#[test]
fn enospc_at_each_step_then_full_link() {
    for path in [WritePath::InPlace, WritePath::Copy] {
        for compacts in [false, true] {
            let calls = clean_calls(path, compacts);
            for (at, kind) in calls.iter().enumerate() {
                let errno = match kind {
                    Kind::Pwrite | Kind::SetLen | Kind::CopyRange | Kind::CreateNew => ENOSPC,
                    _ => EIO,
                };
                let (v, step) = failure_at(path, compacts, at, errno);
                // Before W1/C2 the old pair stands (W1's and C2's write of
                // the flag failing leaves it clear); after, the flag is set.
                let old = step == "C1" || (matches!(step, "W1" | "C2") && *kind == Kind::Pwrite);
                assert_eq!(v == Verdict::OldPair, old, "{path:?} {compacts} at {at} {step}: {v:?}");
            }
        }
    }
}

// ---- the output replaced or written by another --------------------------------

/// Replaces `P` by another file (`rename` over it), or writes into it in
/// place, after the `n`-th successful call of `kind`.
fn interfere(w: &World, new_inode: bool) {
    // Let the clock move past P's timestamps (overlay ticks at 1 ms).
    std::thread::sleep(std::time::Duration::from_millis(20));
    if new_inode {
        let other = w.dir.join("other");
        std::fs::write(&other, b"another link's output").unwrap();
        std::fs::rename(&other, &w.p).unwrap();
    } else {
        let f = File::options().write(true).open(&w.p).unwrap();
        f.write_all_at(b"XX", 100).unwrap();
    }
}

// spec: incr.md IN-77a (immediately before W2 and C3, `stat(P)` by path
// must give the descriptor's file as U1 checked it: else
// `output-changed`, nothing written to `P`, `T` removed), in place and by
// copy, a new inode and the same inode written
#[test]
fn output_replaced_between_check_and_write() {
    for path in [WritePath::InPlace, WritePath::Copy] {
        for new_inode in [true, false] {
            let tmp = Tmp::new("replaced");
            let w = World::create(&tmp.0, false);
            let (out, state) = w.open(path == WritePath::InPlace);
            // The call after which U1's check is stale: W1's sync (in
            // place), C2's (copy).
            let mut syncs = 0;
            let want = if path == WritePath::InPlace { 1 } else { 2 };
            let mut ops = Faulty::new(None, |_, k| {
                if k == Kind::Fdatasync {
                    syncs += 1;
                    if syncs == want {
                        interfere(&w, new_inode);
                    }
                }
            });
            let r = run(&w, &out, &state, path, false, &mut ops, &mut Vec::new());
            assert!(
                matches!(r, Err(WriteFailure::Fallback(IncrReason::OutputChanged(_)))),
                "{path:?} {new_inode}: {r:?}"
            );
            let calls = ops.calls.clone();
            drop(ops);
            let p = std::fs::read(&w.p).unwrap();
            if new_inode {
                assert_eq!(p, b"another link's output", "{path:?}: P is the other file");
            } else {
                assert_eq!(&p[100..102], b"XX", "{path:?}: P is as the other writer left it");
                assert_eq!(p.len(), P_LEN);
            }
            assert!(!w.a.exists() && !w.t.exists(), "{path:?}: nothing set aside, T removed");
            assert!(
                !calls.contains(&Kind::Rename) && !calls.contains(&Kind::HardLink),
                "{path:?}: no rename of P: {calls:?}"
            );
            assert_eq!(judge(&w, &real, Some(&p)), Ok(Verdict::Named("output-torn")));
        }
    }
}

// spec: incr.md IN-77 W4, WP-211 (review-wp207 M-4): another link writes
// `P` while the update has it aside (between W2 and W4): W4's `link`
// gives `EEXIST`, `P` is the other link's, `A` is removed, the flag stays
// set (the next link is a full link) and the update fails with
// `output-changed`, writing nothing more
#[test]
fn foreign_output_during_aside() {
    let tmp = Tmp::new("foreign");
    let w = World::create(&tmp.0, false);
    let (out, state) = w.open(true);
    let mut syncs = 0;
    let mut ops = Faulty::new(None, |_, k| {
        if k == Kind::Fdatasync {
            syncs += 1;
            if syncs == 3 {
                // W3's sync: P names nothing; a plain link renames its
                // output to P.
                interfere(&w, true);
            }
        }
    });
    let r = run(&w, &out, &state, WritePath::InPlace, false, &mut ops, &mut Vec::new());
    let Err(WriteFailure::Failed { step: "W4", error }) = r else { panic!("{r:?}") };
    assert_eq!(error.kind(), io::ErrorKind::AlreadyExists);
    assert!(error.to_string().starts_with("output-changed"), "{error}");
    let calls = ops.calls.clone();
    drop(ops);
    let link = calls.iter().rposition(|k| *k == Kind::HardLink).unwrap();
    assert_eq!(
        calls[link + 1..],
        [Kind::Unlink, Kind::FsyncDir],
        "nothing written after W4's link"
    );
    assert_eq!(std::fs::read(&w.p).unwrap(), b"another link's output");
    assert!(!w.a.exists(), "A removed");
    assert_eq!(judge(&w, &real, Some(b"another link's output")), Ok(Verdict::Named("output-torn")));
}

// spec: incr.md IN-77 W2 (the file renamed aside is not the descriptor's:
// it is put back with `link`, never `rename`, `A` unlinked, and
// `output-changed` with the flag set)
#[test]
fn w2_foreign_file_is_put_back() {
    let tmp = Tmp::new("w2-foreign");
    let w = World::create(&tmp.0, false);
    let (out, state) = w.open(true);
    let mut ops = Faulty::new(None, |_, k| {
        if k == Kind::Flock {
            // Between IN-77a's check and W2's rename (the lock on P is the
            // call between them).
            interfere(&w, true);
        }
    });
    let r = run(&w, &out, &state, WritePath::InPlace, false, &mut ops, &mut Vec::new());
    assert!(matches!(r, Err(WriteFailure::Fallback(IncrReason::OutputChanged(_)))), "{r:?}");
    let log = ops.rec.log.clone();
    drop(ops);
    assert!(
        log.iter().any(|o| matches!(o, Op::HardLink { from, to } if *from == w.a && *to == w.p))
    );
    assert_eq!(std::fs::read(&w.p).unwrap(), b"another link's output");
    assert!(!w.a.exists());
    assert_eq!(judge(&w, &real, Some(b"another link's output")), Ok(Verdict::Named("output-torn")));
}

// ---- leftovers ----------------------------------------------------------------

// spec: incr.md IN-77d (each leftover removed by its exact name, only
// after a `try_lock` on it succeeds: a held one belongs to a live link
// and is left); IN-77 W2 (a held `A` keeps the update off in place:
// WP-219, U-7's reading)
#[test]
fn leftovers_removed_only_when_unlocked() {
    let tmp = Tmp::new("leftovers");
    let w = World::create(&tmp.0, false);
    for p in [&w.a, &w.t, &w.c] {
        std::fs::write(p, b"leftover").unwrap();
    }
    // A live link holds `T` (its own `T0`).
    let held = File::open(&w.t).unwrap();
    held.try_lock().unwrap();
    let (out, state) = w.open(true);
    let mut ops = RecordingOps::default();
    let r = run(&w, &out, &state, WritePath::Copy, false, &mut ops, &mut Vec::new()).unwrap();
    assert_eq!(r.path, WritePath::Copy);
    assert!(!w.a.exists() && !w.c.exists(), "unheld leftovers removed");
    assert_eq!(std::fs::read(&w.t).unwrap(), b"leftover", "the held one is left");
    let first_write = ops.log.iter().position(|o| matches!(o, Op::Pwrite { .. })).unwrap();
    let removed: Vec<&Op> =
        ops.log[..first_write].iter().filter(|o| matches!(o, Op::Unlink { .. })).collect();
    assert_eq!(
        removed,
        [&Op::Unlink { path: w.a.clone() }, &Op::Unlink { path: w.c.clone() }],
        "removed before the update's first write"
    );
    assert_eq!(std::fs::read(&w.p).unwrap(), w.p_new);
    drop(held);

    // A held `A`: in place is not taken.
    let tmp = Tmp::new("leftovers-a");
    let w = World::create(&tmp.0, false);
    std::fs::write(&w.a, b"leftover").unwrap();
    let held = File::open(&w.a).unwrap();
    held.try_lock().unwrap();
    let (out, state) = w.open(true);
    let ino = out.metadata().unwrap().ino();
    let r = run(&w, &out, &state, WritePath::InPlace, false, &mut StdOps, &mut Vec::new()).unwrap();
    assert_eq!(r.path, WritePath::Copy, "a held A: the copy path");
    assert_ne!(std::fs::metadata(&w.p).unwrap().ino(), ino);
    assert_eq!(std::fs::read(&w.a).unwrap(), b"leftover");
    drop(held);
}

// spec: incr.md IN-77 W2, IN-77d, WP-211 §3: a kill between W4's link and
// unlink leaves `A` a second name of `P`; the next update removes it
// before W2 (else `rename(P, A)` would do nothing and leave `P` named)
#[test]
fn leftover_aside_same_inode() {
    let tmp = Tmp::new("aside-same");
    let w = World::create(&tmp.0, false);
    std::fs::hard_link(&w.p, &w.a).unwrap();
    let (out, state) = w.open(true);
    // nlink is 2 while A is there: IN-45 would take the copy path if A
    // were not removed first.
    let mut ops = RecordingOps::default();
    let r = run(&w, &out, &state, WritePath::InPlace, false, &mut ops, &mut Vec::new()).unwrap();
    assert_eq!(r.path, WritePath::InPlace);
    let unlink = ops.log.iter().position(|o| *o == Op::Unlink { path: w.a.clone() }).unwrap();
    let rename = ops.log.iter().position(|o| matches!(o, Op::Rename { .. })).unwrap();
    assert!(unlink < rename, "A removed before W2: {:?}", ops.log);
    assert_eq!(std::fs::read(&w.p).unwrap(), w.p_new);
    assert!(!w.a.exists());
    assert_eq!(judge(&w, &real, None), Ok(Verdict::NewPair));
}

// ---- the state ----------------------------------------------------------------

// spec: incr.md IN-77b (`S` hard-linked or reached through a symbolic
// link is never patched: W5 is the compaction, a rename, so the other name
// keeps the old state's blocks); a plan that patches such an `S` is not
// written (U-2 decides the compaction; WP-219)
#[test]
fn hard_linked_state_is_compacted() {
    let tmp = Tmp::new("state-linked");
    let w = World::create(&tmp.0, false);
    let snapshot = w.dir.join("snapshot");
    std::fs::hard_link(&w.s, &snapshot).unwrap();
    let (out, state) = w.open(true);
    let mut ops = RecordingOps::default();
    let r = run(&w, &out, &state, WritePath::InPlace, false, &mut ops, &mut Vec::new());
    let want = IncrReason::UpdateNotImplemented(Some(Unimplemented::Write));
    assert!(matches!(r, Err(WriteFailure::Fallback(ref e)) if *e == want), "{r:?}");
    assert!(ops.log.is_empty(), "nothing written: {:?}", ops.log);
    assert_eq!(std::fs::read(&w.s).unwrap(), w.s_old);

    let r = run(&w, &out, &state, WritePath::InPlace, true, &mut StdOps, &mut Vec::new()).unwrap();
    assert!(r.compacted);
    assert_eq!(judge(&w, &real, None), Ok(Verdict::NewPair));
    let kept = std::fs::read(&snapshot).unwrap();
    // WP-219 finding F-2: W1 sets the flag through `S`'s descriptor, which
    // is the snapshot's inode: the snapshot keeps its blocks, and its
    // flag word is set (as a state it is `output-torn`).
    let mut want = w.s_old.clone();
    want[FLAGS_OFFSET as usize..FLAGS_OFFSET as usize + 4]
        .copy_from_slice(&FLAG_UPDATING.to_le_bytes());
    assert_eq!(kept, want, "the snapshot: the old blocks, the flag set");
}

// ---- the path (IN-76, IN-45) ------------------------------------------------------

// spec: incr.md IN-76 (in place by default at or above `SMALL_OUTPUT`,
// the copy path below; the inode checked)
#[test]
fn write_path_default_by_size() {
    for big in [false, true] {
        let tmp = Tmp::new("default");
        let w = World::create(&tmp.0, false);
        let len = if big { SMALL_OUTPUT } else { P_LEN as u64 };
        // A sparse output at the bound: no block is allocated for it.
        File::options().write(true).open(&w.p).unwrap().set_len(len).unwrap();
        let (out, state) = w.open(true);
        let ino = out.metadata().unwrap().ino();
        let mut m = w.view();
        m.meta.file_size = len;
        let pair = Pair { size: len, ..w.pair(&out, &state) };
        let _u = updating();
        let r = write_with(
            &pair,
            &m.view(),
            &w.ranges,
            &w.bytes,
            false,
            &mut StdOps,
            WRITE_PATH,
            &mut |row, ops| stub_patch(&state, row, ops),
        )
        .unwrap();
        let after = std::fs::metadata(&w.p).unwrap();
        if big {
            assert_eq!((r.path, after.ino()), (WritePath::InPlace, ino), "at the bound: in place");
        } else {
            assert_eq!(r.path, WritePath::Copy, "below the bound: the copy path");
            assert_ne!(after.ino(), ino, "a new inode");
        }
        assert_eq!(after.len(), len);
        assert_eq!(r.copy_because, None, "IN-76: the default is no condition's");
    }
}

/// An in-place update with one IN-45 condition broken: the copy path, a
/// new inode at `P`.
fn condition(
    label: &str,
    setup: impl FnOnce(&World) -> Option<std::process::Child>,
    writable: bool,
    kind: u8,
) {
    let tmp = Tmp::new("cond");
    let w = World::create(&tmp.0, false);
    let mut child = setup(&w);
    let (out, state) = w.open(writable);
    let ino = out.metadata().unwrap().ino();
    let pair = w.pair(&out, &state);
    let mut m = w.view();
    m.meta.output_kind = kind;
    let _u = updating();
    let r = write_with(
        &pair,
        &m.view(),
        &w.ranges,
        &w.bytes,
        false,
        &mut StdOps,
        policy(WritePath::InPlace),
        &mut |row, ops| stub_patch(&state, row, ops),
    );
    if let Some(c) = &mut child {
        let _ = c.kill();
        let _ = c.wait();
    }
    let r = r.unwrap_or_else(|e| panic!("{label}: {e:?}"));
    assert_eq!(r.path, WritePath::Copy, "{label}");
    assert!(r.copy_because.is_some(), "{label}: IN-76, the report says which");
    let after = std::fs::symlink_metadata(&w.p).unwrap();
    assert!(after.is_file() && after.ino() != ino, "{label}: a new inode at P");
    assert_eq!(std::fs::read(&w.p).unwrap(), w.p_new, "{label}");
}

// spec: incr.md IN-45 (in place only when every condition holds; each one
// broken gives the copy path, not a fall-back), IN-76 ("the update takes
// the copy path"); the inode checked
#[test]
fn in_place_conditions_each_take_copy() {
    condition(
        "symbolic link",
        |w| {
            let real = w.dir.join("real");
            std::fs::rename(&w.p, &real).unwrap();
            std::os::unix::fs::symlink(&real, &w.p).unwrap();
            None
        },
        true,
        0,
    );
    condition(
        "two links",
        |w| {
            std::fs::hard_link(&w.p, w.dir.join("other-name")).unwrap();
            None
        },
        true,
        0,
    );
    condition("not an executable", |_| None, true, 1);
    condition("not writable (ETXTBSY at U1)", |_| None, false, 0);
    condition(
        "held open by another process",
        |w| {
            let f = File::open(&w.p).unwrap();
            Some(std::process::Command::new("sleep").arg("30").stdin(f).fork_spawn().unwrap())
        },
        true,
        0,
    );
    // The same with every condition holding: in place.
    let tmp = Tmp::new("cond-ok");
    let w = World::create(&tmp.0, false);
    let (out, state) = w.open(true);
    let ino = out.metadata().unwrap().ino();
    let r = run(&w, &out, &state, WritePath::InPlace, false, &mut StdOps, &mut Vec::new()).unwrap();
    assert_eq!((r.path, std::fs::metadata(&w.p).unwrap().ino()), (WritePath::InPlace, ino));
}

// spec: incr.md IN-77a, WP-211 (review-wp207 m-1): U1 opens `P`
// read-write for in place, so from U1 an `exec` of `P` fails with
// `ETXTBSY` (kernel 6.8; IN-45 names where it does not)
#[test]
fn exec_fails_from_u1() {
    let tmp = Tmp::new("etxtbsy");
    let p = tmp.0.join("prog");
    // Written by a child (`cp`): a descriptor this process opened for
    // writing would be inherited by every child another test forks.
    let o = std::process::Command::new("cp").arg("/bin/true").arg(&p).fork_status().unwrap();
    assert!(o.success());
    let out = File::options().read(true).write(true).open(&p).unwrap();
    let e = std::process::Command::new(&p).fork_status().unwrap_err();
    assert_eq!(e.raw_os_error(), Some(26), "ETXTBSY: {e}");
    drop(out);
    // Closed (W4's end): it runs, once no other test's child holds it.
    let mut ok = false;
    for _ in 0..50 {
        match std::process::Command::new(&p).fork_status() {
            Ok(s) => {
                ok = s.success();
                break;
            }
            Err(e) if e.raw_os_error() == Some(26) => {
                std::thread::sleep(std::time::Duration::from_millis(20));
            }
            Err(e) => panic!("{e}"),
        }
    }
    assert!(ok, "the program runs once the descriptor is closed");
}

// ---- kills (MORTAR_INCR_CRASH; `--features testhooks`) ----------------------

/// The child of a kill test: runs the update in `MORTAR_WP219_CHILD`'s
/// directory as `MORTAR_WP219_MODE` says (`in-place` or `copy`, then
/// `compact` or `patch`) and aborts at `MORTAR_INCR_CRASH`. Does nothing
/// in a normal run.
#[test]
fn kill_child() {
    let (Some(dir), Some(mode)) =
        (std::env::var_os("MORTAR_WP219_CHILD"), std::env::var("MORTAR_WP219_MODE").ok())
    else {
        return;
    };
    let w = World::names(Path::new(&dir), false);
    let path = if mode.starts_with("in-place") { WritePath::InPlace } else { WritePath::Copy };
    let compacts = mode.ends_with("compact");
    let (out, state) = w.open(path == WritePath::InPlace);
    run(&w, &out, &state, path, compacts, &mut StdOps, &mut Vec::new()).unwrap();
}

/// What a kill at `point` leaves (IN-77's table): the next link's verdict,
/// whether `P` and `A` exist, and `A`'s `e_ident` zero.
#[cfg(feature = "testhooks")]
#[allow(clippy::struct_excessive_bools)]
struct Killed {
    verdict: Verdict,
    p: bool,
    a: bool,
    a_ident_zero: bool,
    same_inode: bool,
}

#[cfg(feature = "testhooks")]
fn kill_at(point: &str, path: WritePath, compacts: bool) -> (Killed, Tmp) {
    use std::os::unix::process::ExitStatusExt;
    let tmp = Tmp::new("kill");
    let w = World::create(&tmp.0, false);
    let mode = format!(
        "{}-{}",
        if path == WritePath::InPlace { "in-place" } else { "copy" },
        if compacts { "compact" } else { "patch" }
    );
    let o = std::process::Command::new(std::env::current_exe().unwrap())
        .args(["--exact", "kill_child", "--nocapture", "--test-threads=1"])
        .env("MORTAR_WP219_CHILD", &tmp.0)
        .env("MORTAR_WP219_MODE", &mode)
        .env("MORTAR_INCR_CRASH", point)
        .fork_output()
        .unwrap();
    let err = String::from_utf8_lossy(&o.stderr);
    assert_eq!(o.status.signal(), Some(6), "{point} {mode}: aborted? {:?} {err}", o.status);
    assert!(err.contains(&format!("mortar: crash hook {point}")), "{point}: {err}");
    let verdict = judge(&w, &real, None).unwrap_or_else(|e| panic!("{point} {mode}: {e}"));
    let a = std::fs::read(&w.a).ok();
    let id = |p: &Path| std::fs::metadata(p).ok().map(|m| (m.dev(), m.ino()));
    let k = Killed {
        verdict,
        p: w.p.exists(),
        a: a.is_some(),
        a_ident_zero: a.as_ref().is_some_and(|b| b[..16].iter().all(|x| *x == 0)),
        same_inode: id(&w.p).is_some() && id(&w.p) == id(&w.a),
    };
    (k, tmp)
}

/// IN-77's table, one row per crash point: the verdict, P present, A
/// present.
#[cfg(feature = "testhooks")]
fn table(point: &str) -> (WritePath, bool, Verdict, bool, bool) {
    use Verdict::{Named, NewPair, OldPair};
    let torn = Named("output-torn");
    let ip = WritePath::InPlace;
    let cp = WritePath::Copy;
    match point {
        "W2-renamed" | "W2-written" | "W2" | "W3-written" | "W3" | "W4-written" => {
            (ip, false, torn, false, true)
        }
        "W4-linked" => (ip, false, torn, true, true),
        "W1" | "W4-unlinked" | "W4" | "W5-written" | "W5" => (ip, false, torn, true, false),
        "W6" => (ip, false, NewPair, true, false),
        "compact-written" => (ip, true, torn, true, false),
        "compact-renamed" => (ip, true, NewPair, true, false),
        "C1-written" | "C1" => (cp, false, OldPair, true, false),
        "C2" | "C3" | "C4" => (cp, false, torn, true, false),
        "C5" => (cp, false, NewPair, true, false),
        _ => panic!("no row for {point}"),
    }
}

#[cfg(feature = "testhooks")]
fn kill_case(point: &str) {
    let (path, compacts, verdict, p, a) = table(point);
    let (k, tmp) = kill_at(point, path, compacts);
    assert_eq!((k.verdict, k.p, k.a), (verdict, p, a), "{point}: verdict, P, A");
    if matches!(point, "W2-written" | "W2" | "W3-written" | "W3") {
        assert!(k.a_ident_zero, "{point}: the torn file has a zero e_ident");
    }
    if point == "W4-linked" {
        assert!(k.same_inode, "{point}: P and A name one inode");
    }
    if point.starts_with('C') && point != "C5" && point < "C3" {
        let w = World::names(&tmp.0, false);
        assert!(w.t.exists(), "{point}: T beside P");
    }
}

// spec: incr.md IN-77 (a kill at every point of the crash hooks: the
// table's `P`, `A` and next link)
#[cfg(feature = "testhooks")]
#[test]
fn kill_at_every_point() {
    for point in mortar_incr::testhook::CRASH_POINTS.iter().filter(|p| !p.starts_with("F-")) {
        kill_case(point);
    }
}

#[cfg(feature = "testhooks")]
macro_rules! between {
    ($($name:ident: $point:literal),* $(,)?) => {$(
        // spec: incr.md IN-77 "Crash hooks" (a kill between a write and
        // its sync, review-in77 m-7, WP-211: the table's outcome)
        #[test]
        fn $name() {
            kill_case($point);
        }
    )*};
}

#[cfg(feature = "testhooks")]
between! {
    kill_between_write_and_sync_w2_renamed: "W2-renamed",
    kill_between_write_and_sync_w2_written: "W2-written",
    kill_between_write_and_sync_w3_written: "W3-written",
    kill_between_write_and_sync_w4_written: "W4-written",
    kill_between_write_and_sync_w4_linked: "W4-linked",
    kill_between_write_and_sync_w4_unlinked: "W4-unlinked",
    kill_between_write_and_sync_w5_written: "W5-written",
    kill_between_write_and_sync_c1_written: "C1-written",
    kill_between_write_and_sync_compact_written: "compact-written",
    kill_between_write_and_sync_compact_renamed: "compact-renamed",
}

// spec: incr.md IN-77 (after a kill in W2..W4 `P` is missing: `exec P`
// fails with `ENOENT`, the torn file is at `A`)
#[cfg(feature = "testhooks")]
#[test]
fn output_missing_after_kill_in_window() {
    for point in ["W2", "W3", "W4-written"] {
        let (k, tmp) = kill_at(point, WritePath::InPlace, false);
        assert!(!k.p && k.a, "{point}");
        let w = World::names(&tmp.0, false);
        let e = std::process::Command::new(&w.p).fork_status().unwrap_err();
        assert_eq!(e.kind(), io::ErrorKind::NotFound, "{point}: {e}");
    }
}

// spec: incr.md IN-77 "Why the aside name" (a kill inside W3 leaves no
// file at `P`, so make relinks it; with a file at `P` newer than its
// input, make would not)
#[cfg(feature = "testhooks")]
#[test]
fn make_relinks_after_kill_at_w3() {
    let make = |dir: &Path| {
        let mark = dir.join("relinked");
        let _ = std::fs::remove_file(&mark);
        let o = std::process::Command::new("make")
            .arg("-s")
            .arg("-C")
            .arg(dir)
            .arg("out")
            .fork_output();
        let Ok(o) = o else {
            assert!(!mortar_testutil::tools_required(), "make is required");
            return None;
        };
        assert!(o.status.success(), "{}", String::from_utf8_lossy(&o.stderr));
        Some(mark.exists())
    };
    let setup = |dir: &Path| {
        std::fs::write(dir.join("Makefile"), "out: in.o\n\ttouch relinked\n").unwrap();
        std::fs::write(dir.join("in.o"), b"").unwrap();
        let old = std::time::SystemTime::now() - std::time::Duration::from_hours(1);
        File::options()
            .write(true)
            .open(dir.join("in.o"))
            .unwrap()
            .set_times(std::fs::FileTimes::new().set_modified(old))
            .unwrap();
    };
    // Control: the old output, newer than its input: not relinked.
    let tmp = Tmp::new("make-control");
    World::create(&tmp.0, false);
    setup(&tmp.0);
    let Some(relinked) = make(&tmp.0) else { return };
    assert!(!relinked, "an up-to-date output is not relinked");
    // Killed at W3: relinked.
    let (_, tmp) = kill_at("W3", WritePath::InPlace, false);
    setup(&tmp.0);
    assert_eq!(make(&tmp.0), Some(true), "make relinks after a kill at W3");
}

// ---- the floor (a full link in incremental mode) ------------------------------

/// A directory with a one-instruction object `a.o`, and the floor's
/// command line `mortar a.o -static --incremental -o out` in it; `None`
/// (the test passes) where no assembler is found and tools are optional.
struct Floor {
    tmp: Tmp,
}

impl Floor {
    fn new(label: &str) -> Option<Self> {
        let tmp = Tmp::new(label);
        let src = tmp.0.join("a.s");
        let text = if cfg!(target_arch = "aarch64") {
            "\t.text\n\t.globl _start\n_start:\n\tmov x0, #0\n\tmov x8, #93\n\tsvc #0\n"
        } else {
            "\t.text\n\t.globl _start\n_start:\n\tmov $60, %eax\n\txor %edi, %edi\n\tsyscall\n"
        };
        std::fs::write(&src, text).unwrap();
        let cc = std::process::Command::new("cc")
            .arg("-c")
            .arg(&src)
            .arg("-o")
            .arg(tmp.0.join("a.o"))
            .fork_status();
        if !cc.is_ok_and(|s| s.success()) {
            assert!(!mortar_testutil::tools_required(), "cc is required");
            return None;
        }
        Some(Self { tmp })
    }

    fn dir(&self) -> &Path {
        &self.tmp.0
    }

    fn command(&self, extra: &[&str]) -> std::process::Command {
        let mut c = std::process::Command::new(env!("CARGO_BIN_EXE_mortar"));
        c.current_dir(self.dir())
            .args(["a.o", "-static", "--incremental", "-o", "out"])
            .args(extra);
        c
    }

    fn link(&self, extra: &[&str]) -> std::process::Output {
        let o = self.command(extra).fork_output().unwrap();
        assert!(o.status.success(), "{}", String::from_utf8_lossy(&o.stderr));
        o
    }

    fn state(&self) -> PathBuf {
        self.dir().join("out.mortar-state")
    }

    fn temporary(&self) -> PathBuf {
        self.dir().join(".out.mortar-state.mortar-compact")
    }

    /// The next link's reason (`--incremental-report` on standard error).
    fn next_reason(&self) -> String {
        let o = self.link(&["--incremental-report"]);
        String::from_utf8_lossy(&o.stderr)
            .lines()
            .find_map(|l| l.strip_prefix("incremental: full ("))
            .map(|l| l.split(')').next().unwrap_or_default().to_owned())
            .unwrap_or_default()
    }
}

// spec: incr.md IN-77d (the floor's state temporary is
// `dir(S)/.<base(S)>.mortar-compact`, no pid: a kill after it is written
// leaves that name, and the next link, finding it unheld, reuses it)
#[cfg(feature = "testhooks")]
#[test]
fn floor_state_temporary_is_the_compact_name() {
    let Some(fl) = Floor::new("floor-name") else { return };
    let o = fl.command(&[]).env("MORTAR_INCR_CRASH", "F-state-written").fork_output().unwrap();
    assert!(!o.status.success(), "{}", String::from_utf8_lossy(&o.stderr));
    let names: Vec<String> = std::fs::read_dir(fl.dir())
        .unwrap()
        .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
        .collect();
    assert!(names.contains(&".out.mortar-state.mortar-compact".to_owned()), "{names:?}");
    assert!(!names.iter().any(|n| n.contains(".tmp-")), "{names:?}");
    fl.link(&[]);
    assert!(fl.state().exists());
    assert!(!fl.temporary().exists());
}

// spec: incr.md IN-77d, IN-21 (WP-219 F-8: a temporary held by a live
// link: this link writes no state, `state-busy`, and touches neither the
// temporary nor `S`; the link itself succeeds)
#[test]
fn floor_temporary_held_writes_no_state() {
    let Some(fl) = Floor::new("floor-held") else { return };
    std::fs::write(fl.temporary(), b"another link's state, half written").unwrap();
    let held = File::options().read(true).write(true).open(fl.temporary()).unwrap();
    held.try_lock().unwrap();
    let o = fl.link(&[]);
    let err = String::from_utf8_lossy(&o.stderr);
    assert!(err.contains("no state written") && err.contains("state-busy"), "{err}");
    assert!(fl.dir().join("out").exists(), "the link wrote its output");
    assert!(!fl.state().exists(), "no state");
    assert_eq!(std::fs::read(fl.temporary()).unwrap(), b"another link's state, half written");
    drop(held);
}

// spec: incr.md IN-77d (WP-219 F-8: an unheld temporary is a leftover:
// truncated and reused; the state written is whole)
#[test]
fn floor_reuses_a_leftover_temporary() {
    let Some(fl) = Floor::new("floor-leftover") else { return };
    std::fs::write(fl.temporary(), vec![0x5a; 1 << 20]).unwrap();
    fl.link(&[]);
    assert!(!fl.temporary().exists());
    assert!(fl.state().exists());
    let reason = fl.next_reason();
    assert!(!reason.contains("state-corrupt"), "{reason}");
}

// spec: incr.md IN-21, IN-77d (WP-219 F-8: two full links that find no
// state, at once: each succeeds, no temporary is left, and the state that
// remains, if any, is whole: never `state-corrupt`)
#[test]
fn two_floors_at_once() {
    let Some(fl) = Floor::new("two-floors") else { return };
    for round in 0..10 {
        let _ = std::fs::remove_file(fl.state());
        let mut a = fl.command(&[]).stderr(std::process::Stdio::piped()).fork_spawn().unwrap();
        let mut b = fl.command(&[]).stderr(std::process::Stdio::piped()).fork_spawn().unwrap();
        let (sa, sb) = (a.wait().unwrap(), b.wait().unwrap());
        assert!(sa.success() && sb.success(), "round {round}");
        assert!(!fl.temporary().exists(), "round {round}: no temporary left");
        let reason = fl.next_reason();
        assert!(!reason.contains("state-corrupt"), "round {round}: {reason}");
    }
}

// spec: incr.md IN-77b (the full link in incremental mode that finds a
// state takes the same lock: it waits while another link holds it)
#[test]
fn floor_waits_for_the_state_lock() {
    let Some(fl) = Floor::new("floor-lock") else { return };
    fl.link(&[]);
    let held = File::open(fl.state()).unwrap();
    held.lock().unwrap();
    let mut child = fl.command(&[]).stderr(std::process::Stdio::null()).fork_spawn().unwrap();
    std::thread::sleep(std::time::Duration::from_millis(500));
    assert!(child.try_wait().unwrap().is_none(), "the link waits for the lock");
    drop(held);
    assert!(child.wait().unwrap().success());
}

// ---- IN-77a: U1's tuple (WP-219 F-7) ------------------------------------------------

// spec: incr.md IN-77a (the descriptor's `fstat` must still give U1's
// tuple, mtime and ctime included: a write to `P` in place after U1's
// check, the size kept, is `output-changed` with nothing written)
#[test]
fn output_written_after_u1_check() {
    for path in [WritePath::InPlace, WritePath::Copy] {
        let tmp = Tmp::new("after-u1");
        let w = World::create(&tmp.0, false);
        let (out, state) = w.open(path == WritePath::InPlace);
        let pair = w.pair(&out, &state);
        std::thread::sleep(std::time::Duration::from_millis(20));
        File::options().write(true).open(&w.p).unwrap().write_all_at(b"XX", 100).unwrap();
        let m = w.view();
        let mut ops = RecordingOps::default();
        let _u = updating();
        let r = write_with(
            &pair,
            &m.view(),
            &w.ranges,
            &w.bytes,
            false,
            &mut ops,
            policy(path),
            &mut |row, ops| stub_patch(&state, row, ops),
        );
        assert!(
            matches!(r, Err(WriteFailure::Fallback(IncrReason::OutputChanged(_)))),
            "{path:?}: {r:?}"
        );
        assert!(ops.log.is_empty(), "{path:?}: nothing written: {:?}", ops.log);
        assert_eq!(std::fs::read(&w.s).unwrap(), w.s_old);
    }
}

// ---- IN-77b: the lock ---------------------------------------------------------------

// spec: incr.md IN-77b (no state: nothing to lock; a path that is no
// regular file is never opened: `state-corrupt`)
#[test]
fn lock_state_no_state_and_not_a_file() {
    let tmp = Tmp::new("lock-none");
    let s = tmp.0.join("out.mortar-state");
    assert!(matches!(lock_state(&s, &mut StdOps), Ok(None)));
    let made = std::process::Command::new("mkfifo").arg(&s).fork_status();
    if made.is_ok_and(|x| x.success()) {
        assert!(matches!(lock_state(&s, &mut StdOps), Err(IncrReason::StateCorrupt(_))));
    }
}

// spec: incr.md IN-77b (a rename of `S` while a link waits for its lock:
// the waiter wakes on the old inode, fails the check, and locks the new
// one)
#[test]
fn state_renamed_while_waiting_retries() {
    let tmp = Tmp::new("lock-rename");
    let w = World::create(&tmp.0, false);
    let holder = File::open(&w.s).unwrap();
    holder.lock().unwrap();
    let (tx, rx) = std::sync::mpsc::channel();
    let s = w.s.clone();
    let waiter = std::thread::spawn(move || {
        let mut ops = RecordingOps::default();
        let r = lock_state(&s, &mut ops);
        tx.send(()).unwrap();
        (r, ops.log)
    });
    std::thread::sleep(std::time::Duration::from_millis(200));
    assert!(rx.try_recv().is_err(), "the waiter blocks while the lock is held");
    // The holder compacts: a new state renamed over `S`, then unlocks.
    let new = w.dir.join("new-state");
    std::fs::write(&new, &w.s_old).unwrap();
    std::fs::rename(&new, &w.s).unwrap();
    let new_ino = std::fs::metadata(&w.s).unwrap().ino();
    drop(holder);
    let (r, log) = waiter.join().unwrap();
    let file = r.unwrap().expect("a state");
    assert_eq!(file.metadata().unwrap().ino(), new_ino, "the new inode is locked");
    let rounds =
        log.iter().filter(|o| matches!(o, Op::Flock { mode: LockMode::Exclusive, .. })).count();
    assert_eq!(rounds, 2, "a second round: {log:?}");
}

// spec: incr.md IN-77b (renamed at every round: `state-busy` after 8
// rounds, never a lock on a state the path does not name)
#[test]
fn state_busy_after_eight_rounds() {
    let tmp = Tmp::new("lock-busy");
    let w = World::create(&tmp.0, false);
    let mut n = 0;
    let mut ops = Faulty::new(None, |_, k| {
        if k == Kind::Flock {
            n += 1;
            let new = w.dir.join(format!("state-{n}"));
            std::fs::write(&new, &w.s_old).unwrap();
            std::fs::rename(&new, &w.s).unwrap();
        }
    });
    let r = lock_state(&w.s, &mut ops);
    assert!(matches!(r, Err(IncrReason::StateBusy(_))), "{r:?}");
    let rounds = ops.calls.iter().filter(|k| **k == Kind::Flock).count();
    assert_eq!(rounds, 8);
}

// spec: incr.md IN-77b, image.md I-59 (where `flock` is not enforced,
// `lock-unavailable`)
#[test]
fn lock_unavailable_when_not_enforced() {
    let tmp = Tmp::new("lock-unenforced");
    let w = World::create(&tmp.0, false);
    let mut ops = Faulty::new(None, |_, _| {});
    ops.unenforced = true;
    let r = lock_state(&w.s, &mut ops);
    assert!(matches!(r, Err(IncrReason::LockUnavailable(_))), "{r:?}");
    let mut ops = Faulty::new(None, |_, _| {});
    assert!(lock_state(&w.s, &mut ops).unwrap().is_some(), "enforced here");
}

// spec: incr.md IN-77b (two updates of one output: the second's lock
// waits until the first's last write; it then finds the first's new pair)
#[test]
fn two_updates_one_output_serialize() {
    let tmp = Tmp::new("serialize");
    let w = World::create(&tmp.0, false);
    let (started_tx, started_rx) = std::sync::mpsc::channel();
    let (done, acquired) = std::thread::scope(|scope| {
        let wr = &w;
        let second = scope.spawn(move || {
            started_rx.recv().unwrap();
            let lock = lock_state(&wr.s, &mut StdOps).unwrap().expect("a state");
            let at = std::time::Instant::now();
            let verdict = judge(wr, &real, None);
            drop(lock);
            (at, verdict)
        });
        let lock = lock_state(&w.s, &mut StdOps).unwrap().expect("a state");
        started_tx.send(()).unwrap();
        let out = File::options().read(true).write(true).open(&w.p).unwrap();
        let mut ops = Faulty::new(None, |_, k| {
            if k == Kind::Fdatasync {
                std::thread::sleep(std::time::Duration::from_millis(30));
            }
        });
        run(&w, &out, &lock, WritePath::InPlace, false, &mut ops, &mut Vec::new()).unwrap();
        let done = std::time::Instant::now();
        drop(lock);
        (done, second.join().unwrap())
    });
    let (at, verdict) = acquired;
    assert!(at >= done, "the second link locked only after the first's last write");
    assert_eq!(verdict, Ok(Verdict::NewPair), "it finds the first's new pair");
}

// ---- IN-77c: the boot id --------------------------------------------------------------

/// The leaves of `bytes` (L128, `sealed` hashed as zeros, image.md I-73).
fn leaves_of(bytes: &[u8], sealed: std::ops::Range<u64>) -> Vec<[u8; 32]> {
    let size = bytes.len() as u64;
    let mut out = Vec::new();
    let mut i = 0;
    loop {
        let r = mortar_digest::leaf_range(size, i);
        if r.start >= size {
            break;
        }
        let mut window = bytes[r.start as usize..r.end as usize].to_vec();
        let (lo, hi) = (sealed.start.max(r.start), sealed.end.min(r.end));
        if lo < hi {
            window[(lo - r.start) as usize..(hi - r.start) as usize].fill(0);
        }
        let leaf = mortar_digest::leaf(mortar_digest::TreeFn::L128, &window);
        let mut row = [0u8; 32];
        row[..leaf.as_bytes().len()].copy_from_slice(leaf.as_bytes());
        out.push(row);
        i += 1;
    }
    out
}

// spec: incr.md IN-77c (bit 0 and another boot id: every leaf of `P`
// against `LEAVES` before `P` is trusted; an unreadable boot id is a
// changed one; bit 1 or the same boot: nothing read)
#[test]
fn boot_id_changed_checks_leaves() {
    assert!(boot_id().is_some(), "Linux has /proc/sys/kernel/random/boot_id");
    let tmp = Tmp::new("boot");
    let p = tmp.0.join("out");
    let bytes: Vec<u8> = (0..(5 << 19)).map(|i: u32| (i % 241) as u8).collect();
    std::fs::write(&p, &bytes).unwrap();
    let sealed = (1 << 20) + 16..(1 << 20) + 36; // the build id, in leaf 1
    let mut m = MemoryState::default();
    m.meta.file_size = bytes.len() as u64;
    m.meta.leaf_fn = 0;
    m.meta.sealed_start = sealed.start;
    m.meta.sealed_end = sealed.end;
    m.meta.boot_id = [1; 16];
    m.meta.output_synced = false;
    m.leaves = leaves_of(&bytes, sealed.clone());
    assert!(m.leaves.len() >= 3);
    let out = File::open(&p).unwrap();
    let same = Some([1; 16]);
    let other = Some([2; 16]);
    assert_eq!(boot_check(&m.view(), &out, same), Ok(()));
    assert_eq!(boot_check(&m.view(), &out, other), Ok(()), "every leaf matches");
    assert_eq!(boot_check(&m.view(), &out, None), Ok(()), "unreadable: compared, matches");
    // The build id's bytes are hashed as zeros: a change there is no change.
    File::options().write(true).open(&p).unwrap().write_all_at(b"!", sealed.start).unwrap();
    assert_eq!(boot_check(&m.view(), &out, other), Ok(()));
    // A byte of leaf 2 that did not reach the disk.
    File::options().write(true).open(&p).unwrap().write_all_at(b"!", (2 << 20) + 5).unwrap();
    for boot in [other, None] {
        let r = boot_check(&m.view(), &out, boot);
        assert!(
            matches!(&r, Err(IncrReason::OutputChanged(why)) if why.contains("window 2")),
            "{boot:?}: {r:?}"
        );
    }
    assert_eq!(boot_check(&m.view(), &out, same), Ok(()), "the same boot: not read");
    m.meta.output_synced = true;
    assert_eq!(boot_check(&m.view(), &out, other), Ok(()), "synced at W4/C1: not read");
}

// ---- IN-77b's virtiofs note: a full link that saw no state -----------------------

/// A full link that found no state (its `open(S)` gave `ENOENT`): its own
/// output renamed over `P`, then its state, bound to that output, written
/// by the floor's protocol (IN-77d); IN-21's check after the rename (the
/// output still its own) else its state removed. No lock on `S`.
fn full_link_without_state(w: &World) {
    let t = w.dir.join("full-link-output");
    std::fs::write(&t, OTHER_OUTPUT).unwrap();
    std::fs::rename(&t, &w.p).unwrap();
    let m = std::fs::metadata(&w.p).unwrap();
    let img = s_image(m.dev(), m.ino(), m.size(), OTHER_BODY, 0);
    if let Ok(true) = write_state_whole(&w.s, &img, &mut StdOps) {
        let now = std::fs::metadata(&w.p).ok().map(|n| (n.dev(), n.ino()));
        if now != Some((m.dev(), m.ino())) {
            let _ = std::fs::remove_file(&w.s);
        }
    }
}

const OTHER_OUTPUT: &[u8] = b"\x7fELF another link's output";

// spec: incr.md IN-77b (the virtiofs note: `ENOENT` on `S` is "no state";
// such a full link takes no lock): an update holding the lock, and a full
// link that saw no state run in full after each of the update's calls
// (its compaction's among them), in place and by copy: what remains is
// the update's pair, the full link's pair, or a reason the next link
// names; never a trusted mix
#[test]
fn full_link_during_compaction() {
    for path in [WritePath::InPlace, WritePath::Copy] {
        for compacts in [true, false] {
            let calls = clean_calls(path, compacts);
            for (at, &kind) in calls.iter().enumerate() {
                let tmp = Tmp::new("full-during");
                let w = World::create(&tmp.0, false);
                let (out, state) = w.open(path == WritePath::InPlace);
                let mut ops = Faulty::new(None, |n, _| {
                    if n == at {
                        full_link_without_state(&w);
                    }
                });
                let r = run(&w, &out, &state, path, compacts, &mut ops, &mut Vec::new());
                let v = judge(&w, &real, Some(OTHER_OUTPUT))
                    .unwrap_or_else(|e| panic!("{path:?} {compacts} after {at} ({kind:?}): {e}"));
                assert!(
                    matches!(v, Verdict::OtherPair | Verdict::Named(_)),
                    "{path:?} {compacts} after {at} ({kind:?}): {v:?} ({r:?})"
                );
                assert_eq!(std::fs::read(&w.p).unwrap(), OTHER_OUTPUT, "the full link's output");
            }
        }
    }
}

// spec: incr.md IN-77b, IN-77d, §7 `lock-unavailable` (WP-219 rulings-2:
// where `flock` is not enforced, the full link writes no state and
// removes a previous one by its exact name; its output is a full link's;
// a link that found no state, probing the temporary, writes none either)
#[cfg(feature = "testhooks")]
#[test]
fn lock_unavailable_writes_no_state() {
    let Some(fl) = Floor::new("lock-unavailable") else { return };
    fl.link(&[]);
    assert!(fl.state().exists());
    let full = std::fs::read(fl.dir().join("out")).unwrap();
    let o = fl.command(&[]).env("MORTAR_INCR_UNENFORCED", "1").fork_output().unwrap();
    let err = String::from_utf8_lossy(&o.stderr);
    assert!(o.status.success(), "{err}");
    assert!(err.contains("no state written: lock-unavailable"), "{err}");
    assert!(!fl.state().exists(), "the previous state is removed");
    assert!(!fl.temporary().exists());
    assert_eq!(std::fs::read(fl.dir().join("out")).unwrap(), full, "a full link's output");
    // No state before: the temporary's probe says the same.
    let o = fl.command(&[]).env("MORTAR_INCR_UNENFORCED", "1").fork_output().unwrap();
    let err = String::from_utf8_lossy(&o.stderr);
    assert!(o.status.success(), "{err}");
    assert!(err.contains("no state written") && err.contains("lock-unavailable"), "{err}");
    assert!(!fl.state().exists() && !fl.temporary().exists());
}

// spec: incr.md IN-77d (the floor's temporary, locked, probed: not
// enforced is `lock-unavailable`, the temporary removed, `S` untouched)
#[test]
fn state_temporary_probe_unenforced() {
    let tmp = Tmp::new("tmp-probe");
    let s = tmp.0.join("out.mortar-state");
    let mut ops = Faulty::new(None, |_, _| {});
    ops.unenforced = true;
    let e = write_state_whole(&s, b"state", &mut ops).unwrap_err();
    assert_eq!(e.kind(), io::ErrorKind::Unsupported, "{e}");
    assert!(e.to_string().contains("lock-unavailable"), "{e}");
    assert!(!s.exists());
    assert!(!tmp.0.join(".out.mortar-state.mortar-compact").exists());
    assert!(write_state_whole(&s, b"state", &mut StdOps).unwrap());
    assert_eq!(std::fs::read(&s).unwrap(), b"state");
}
