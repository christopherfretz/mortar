//! The update's file operations (`spec/incr.md` IN-77, IN-77a..e, IN-18f):
//! every write, sync, rename, link, unlink and lock of an update's writes
//! (U-7, `mortar::update::write`) and of the state's patch and compaction
//! (U-2, `crate::state5`) goes through [`WriteOps`], so that the crash
//! harness can record them and replay every prefix and subset of the
//! unsynced ones (`power_loss_every_prefix_and_subset`, IN-77).
//!
//! WP-213 (U-1) moved the trait here from `mortar/src/update/write.rs`
//! (WP-211 §C): `state5::patch` and `state5::compact` in this crate take a
//! `&mut dyn WriteOps`, and `mortar_incr` cannot see the `mortar` crate.
//! U-1 wrote the trait, the real implementation [`StdOps`] and the
//! recording type [`RecordingOps`]; **U-7 owns this file after U-1** (the
//! recording's replay is U-7's; it adds no call without the supervisor).
//!
//! Every call takes the descriptor U1 opened (IN-77a: no step reopens `P`
//! or `S` by path); paths appear only where the call is a directory
//! operation (`rename`, `hard_link`, `unlink`, `create_new`, `fsync_dir`).

use std::fs::File;
use std::io;
use std::path::{Path, PathBuf};

use mortar_core::incr::IncrReason;

/// How [`WriteOps::flock`] locks (IN-77b; `std`'s `File::lock`,
/// `try_lock`, `unlock`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum LockMode {
    /// Block until the exclusive lock is held (U1 on `S`, IN-77b).
    Exclusive,
    /// Take the exclusive lock if free; `Ok(false)` if another holds it
    /// (IN-77d: leftovers are removed only by a lock holder).
    TryExclusive,
    /// Release it.
    Unlock,
}

/// The file operations of IN-77 (WP-211 §C's list, plus the two WP-213
/// found missing: `create_new` for C1's `T` and the compaction's
/// temporary, and `copy_range` for C1's copy). Each is one system call's
/// worth of effect, so the recording can cut between any two.
pub trait WriteOps {
    /// Write `bytes` at `offset` (`pwrite`, all of them).
    ///
    /// # Errors
    /// The I/O error; IN-77e: the update ends at once.
    fn pwrite(&mut self, file: &File, offset: u64, bytes: &[u8]) -> io::Result<()>;
    /// Truncate or extend to `len` bytes (image.md I-97; W3).
    ///
    /// # Errors
    /// The I/O error.
    fn set_len(&mut self, file: &File, len: u64) -> io::Result<()>;
    /// `fdatasync`. Never retried after an error (IN-77e).
    ///
    /// # Errors
    /// The I/O error.
    fn fdatasync(&mut self, file: &File) -> io::Result<()>;
    /// `fsync` (data and metadata; W4, review-in77 m-6).
    ///
    /// # Errors
    /// The I/O error.
    fn fsync(&mut self, file: &File) -> io::Result<()>;
    /// `rename(from, to)` (W2's `rename(P, A)`, C3, the compaction).
    ///
    /// # Errors
    /// The I/O error.
    fn rename(&mut self, from: &Path, to: &Path) -> io::Result<()>;
    /// `link(from, to)` (W4's `link(A, P)`; `EEXIST` is
    /// `io::ErrorKind::AlreadyExists`, which W4 handles).
    ///
    /// # Errors
    /// The I/O error.
    fn hard_link(&mut self, from: &Path, to: &Path) -> io::Result<()>;
    /// `unlink(path)`.
    ///
    /// # Errors
    /// The I/O error.
    fn unlink(&mut self, path: &Path) -> io::Result<()>;
    /// `fsync` of the directory `dir` (open it, `fsync`, close).
    ///
    /// # Errors
    /// The I/O error.
    fn fsync_dir(&mut self, dir: &Path) -> io::Result<()>;
    /// Lock, try or unlock `file` (IN-77b; `flock` through `std`).
    /// `Ok(true)` when the lock is held (or released, for `Unlock`).
    ///
    /// # Errors
    /// The I/O error.
    fn flock(&mut self, file: &File, mode: LockMode) -> io::Result<bool>;
    /// Create `path` exclusively, read-write (`O_CREAT|O_EXCL`): C1's `T`
    /// and the compaction's temporary (IN-18f step 5).
    ///
    /// # Errors
    /// The I/O error (`AlreadyExists` for a leftover, IN-77d).
    fn create_new(&mut self, path: &Path) -> io::Result<File>;
    /// Copy `len` bytes of `from` from its start to `to`'s start (C1:
    /// `copy_file_range` through `std::io::copy`, a clone where the file
    /// system clones, IN-47).
    ///
    /// # Errors
    /// The I/O error.
    fn copy_range(&mut self, from: &File, to: &File, len: u64) -> io::Result<()>;
}

/// The real [`WriteOps`], over `std` only (no `unsafe`, no `libc`).
#[derive(Debug, Clone, Copy, Default)]
pub struct StdOps;

impl WriteOps for StdOps {
    fn pwrite(&mut self, file: &File, offset: u64, bytes: &[u8]) -> io::Result<()> {
        std::os::unix::fs::FileExt::write_all_at(file, bytes, offset)
    }
    fn set_len(&mut self, file: &File, len: u64) -> io::Result<()> {
        file.set_len(len)
    }
    fn fdatasync(&mut self, file: &File) -> io::Result<()> {
        file.sync_data()
    }
    fn fsync(&mut self, file: &File) -> io::Result<()> {
        file.sync_all()
    }
    fn rename(&mut self, from: &Path, to: &Path) -> io::Result<()> {
        std::fs::rename(from, to)
    }
    fn hard_link(&mut self, from: &Path, to: &Path) -> io::Result<()> {
        std::fs::hard_link(from, to)
    }
    fn unlink(&mut self, path: &Path) -> io::Result<()> {
        std::fs::remove_file(path)
    }
    fn fsync_dir(&mut self, dir: &Path) -> io::Result<()> {
        File::open(dir)?.sync_all()
    }
    fn flock(&mut self, file: &File, mode: LockMode) -> io::Result<bool> {
        match mode {
            LockMode::Exclusive => file.lock().map(|()| true),
            LockMode::TryExclusive => match file.try_lock() {
                Ok(()) => Ok(true),
                Err(std::fs::TryLockError::WouldBlock) => Ok(false),
                Err(std::fs::TryLockError::Error(e)) => Err(e),
            },
            LockMode::Unlock => file.unlock().map(|()| true),
        }
    }
    fn create_new(&mut self, path: &Path) -> io::Result<File> {
        File::options().read(true).write(true).create_new(true).open(path)
    }
    fn copy_range(&mut self, from: &File, to: &File, len: u64) -> io::Result<()> {
        use std::io::{Read, Seek, SeekFrom};
        let (mut from, mut to) = (from, to);
        from.seek(SeekFrom::Start(0))?;
        to.seek(SeekFrom::Start(0))?;
        let copied = io::copy(&mut from.take(len), &mut to)?;
        if copied == len {
            Ok(())
        } else {
            Err(io::Error::new(io::ErrorKind::UnexpectedEof, "short copy"))
        }
    }
}

/// One recorded call of [`RecordingOps`]: the operation, its file by the
/// recording's own number (the order descriptors were first seen), and
/// whether it is a sync (a barrier for the replay).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Op {
    Pwrite {
        file: u32,
        offset: u64,
        bytes: Vec<u8>,
    },
    SetLen {
        file: u32,
        len: u64,
    },
    Fdatasync {
        file: u32,
    },
    Fsync {
        file: u32,
    },
    Rename {
        from: PathBuf,
        to: PathBuf,
    },
    HardLink {
        from: PathBuf,
        to: PathBuf,
    },
    Unlink {
        path: PathBuf,
    },
    FsyncDir {
        dir: PathBuf,
    },
    Flock {
        file: u32,
        mode: LockMode,
    },
    /// `file` is the new file's number (U-7: a created file always gets a
    /// new number, so an inode number the kernel reuses after an unlink
    /// is not taken for the old file).
    CreateNew {
        path: PathBuf,
        file: u32,
    },
    CopyRange {
        from: u32,
        to: u32,
        len: u64,
    },
}

/// The crash harness's [`WriteOps`] (IN-77's power-loss test): the TYPE is
/// U-1's, its behaviour U-7's. It performs every call through [`StdOps`]
/// and records in [`RecordingOps::log`] **the calls that succeeded**, in
/// order: the effects the file system saw. A call that failed changed
/// nothing the replay must model (a test that injects failures wraps
/// this type). [`RecordingOps::power_loss`] is the replay: every state
/// the disk can be left in by a power loss after any prefix of the log.
#[derive(Debug, Default)]
pub struct RecordingOps {
    /// Every successful call, in order.
    pub log: Vec<Op>,
    /// `(dev, ino)` of each descriptor seen, its index the `file` of an
    /// [`Op`]. A created file always appends a new entry; a descriptor is
    /// numbered by the LAST entry of its `(dev, ino)`.
    pub files: Vec<(u64, u64)>,
}

fn dev_ino(file: &File) -> (u64, u64) {
    use std::os::unix::fs::MetadataExt;
    file.metadata().map_or((0, 0), |m| (m.dev(), m.ino()))
}

fn number(at: usize) -> u32 {
    u32::try_from(at).unwrap_or(u32::MAX)
}

impl RecordingOps {
    fn id(&mut self, file: &File) -> u32 {
        let key = dev_ino(file);
        let at = self.files.iter().rposition(|k| *k == key).unwrap_or_else(|| {
            self.files.push(key);
            self.files.len() - 1
        });
        number(at)
    }

    /// The recording's number of `file` (by its `(dev, ino)`), if it was
    /// seen: how a test names `P` and `S` in the [`Disk`] it replays over.
    #[must_use]
    pub fn file_of(&self, file: &File) -> Option<u32> {
        let key = dev_ino(file);
        self.files.iter().rposition(|k| *k == key).map(number)
    }

    /// The power-loss replay (IN-77 "Crash hooks";
    /// `power_loss_every_prefix_and_subset`). For every prefix of the log
    /// (none, the first call, ..., all of them) and every subset of the
    /// prefix's calls **not yet made durable** by a later sync inside the
    /// prefix, applies the durable calls and the subset, in log order, to
    /// `before` and calls `visit(prefix length, disk)`. Durable: a
    /// `pwrite`, `set_len` or copy into a file once that file is
    /// `fdatasync`ed or `fsync`ed; a `rename`, `link`, `unlink` or create
    /// once the directory of each name it changes is `fsync`ed. A call is
    /// all or nothing (a write within one sector is; a larger one may tear
    /// on a real disk: not modelled). A name operation whose source the
    /// chosen subset never created does nothing; a rename between two
    /// names of one inode does nothing (POSIX). A copy (C1) copies the
    /// bytes the source held when it ran, as the live file system had them
    /// (the page cache), not the durable ones. Returns the number of disks
    /// visited.
    ///
    /// # Errors
    /// A prefix with more than `max_pending` calls not yet durable (the
    /// subsets would be too many), or a call on a file the replay does not
    /// know (a test that did not give it in `before`).
    pub fn power_loss(
        &self,
        before: &Disk,
        max_pending: usize,
        mut visit: impl FnMut(usize, &Disk),
    ) -> Result<u64, String> {
        let effects = self.effects(before)?;
        let mut visited = 0u64;
        for k in 0..=effects.len() {
            let mut durable = Vec::new();
            let mut pending = Vec::new();
            for (i, e) in effects[..k].iter().enumerate() {
                match e.durable_after(&effects[i + 1..k]) {
                    None => {}
                    Some(true) => durable.push(i),
                    Some(false) => pending.push(i),
                }
            }
            if pending.len() > max_pending {
                return Err(format!(
                    "prefix {k}: {} calls not yet durable, more than {max_pending}",
                    pending.len()
                ));
            }
            for mask in 0u64..(1u64 << pending.len()) {
                let mut chosen: Vec<usize> = durable.clone();
                chosen.extend(
                    pending.iter().enumerate().filter(|(b, _)| mask >> b & 1 == 1).map(|(_, i)| *i),
                );
                chosen.sort_unstable();
                let mut disk = before.clone();
                for i in chosen {
                    effects[i].apply(&mut disk);
                }
                visit(k, &disk);
                visited += 1;
            }
        }
        Ok(visited)
    }

    /// The log as effects on a [`Disk`]: a copy becomes the bytes it
    /// copied (from a live replay of every call), flocks disappear.
    fn effects(&self, before: &Disk) -> Result<Vec<Effect>, String> {
        let mut live = before.clone();
        let mut out = Vec::with_capacity(self.log.len());
        for op in &self.log {
            let e = match op {
                Op::Pwrite { file, offset, bytes } => {
                    Effect::Data { file: *file, offset: *offset, bytes: bytes.clone() }
                }
                Op::SetLen { file, len } => Effect::Len { file: *file, len: *len },
                Op::Fdatasync { file } | Op::Fsync { file } => Effect::SyncFile(*file),
                Op::FsyncDir { dir } => Effect::SyncDir(dir.clone()),
                Op::Rename { from, to } => Effect::Rename { from: from.clone(), to: to.clone() },
                Op::HardLink { from, to } => Effect::Link { from: from.clone(), to: to.clone() },
                Op::Unlink { path } => Effect::Unlink { path: path.clone() },
                Op::CreateNew { path, file } => Effect::Create { path: path.clone(), file: *file },
                Op::Flock { .. } => continue,
                Op::CopyRange { from, to, len } => {
                    let src = live.data.get(from).ok_or_else(|| {
                        format!("copy from file {from}, which the replay does not know")
                    })?;
                    let n = usize::try_from(*len).map_err(|e| e.to_string())?;
                    let mut bytes = src.get(..n.min(src.len())).unwrap_or_default().to_vec();
                    bytes.resize(n, 0);
                    Effect::Data { file: *to, offset: 0, bytes }
                }
            };
            if let Effect::Data { file, .. } | Effect::Len { file, .. } = &e
                && !live.data.contains_key(file)
            {
                return Err(format!("a write to file {file}, which the replay does not know"));
            }
            e.apply(&mut live);
            out.push(e);
        }
        Ok(out)
    }
}

/// A file system as byte buffers (the power-loss replay of
/// [`RecordingOps::power_loss`]): the file each path names (by the
/// recording's numbers) and each file's bytes.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Disk {
    pub names: std::collections::BTreeMap<PathBuf, u32>,
    pub data: std::collections::BTreeMap<u32, Vec<u8>>,
}

impl Disk {
    /// The bytes of the file `path` names, if it names one.
    #[must_use]
    pub fn read(&self, path: &Path) -> Option<&[u8]> {
        self.names.get(path).and_then(|f| self.data.get(f)).map(Vec::as_slice)
    }
}

/// One effect of a recorded call on a [`Disk`].
#[derive(Debug, Clone)]
enum Effect {
    Data { file: u32, offset: u64, bytes: Vec<u8> },
    Len { file: u32, len: u64 },
    SyncFile(u32),
    SyncDir(PathBuf),
    Rename { from: PathBuf, to: PathBuf },
    Link { from: PathBuf, to: PathBuf },
    Unlink { path: PathBuf },
    Create { path: PathBuf, file: u32 },
}

fn parent(path: &Path) -> &Path {
    match path.parent() {
        Some(p) if !p.as_os_str().is_empty() => p,
        _ => Path::new("."),
    }
}

impl Effect {
    /// `None` for a sync (a barrier, not an effect); else whether a sync
    /// among `later` makes it durable.
    fn durable_after(&self, later: &[Effect]) -> Option<bool> {
        let file_synced =
            |f: u32| later.iter().any(|e| matches!(e, Effect::SyncFile(g) if *g == f));
        let dir_synced =
            |p: &Path| later.iter().any(|e| matches!(e, Effect::SyncDir(d) if d == parent(p)));
        match self {
            Effect::SyncFile(_) | Effect::SyncDir(_) => None,
            Effect::Data { file, .. } | Effect::Len { file, .. } => Some(file_synced(*file)),
            Effect::Rename { from, to } => Some(dir_synced(from) && dir_synced(to)),
            Effect::Link { to: path, .. }
            | Effect::Unlink { path }
            | Effect::Create { path, .. } => Some(dir_synced(path)),
        }
    }

    fn apply(&self, disk: &mut Disk) {
        match self {
            Effect::SyncFile(_) | Effect::SyncDir(_) => {}
            Effect::Data { file, offset, bytes } => {
                let data = disk.data.entry(*file).or_default();
                let at = usize::try_from(*offset).unwrap_or(usize::MAX);
                let end = at.saturating_add(bytes.len());
                if data.len() < end {
                    data.resize(end, 0);
                }
                data[at..end].copy_from_slice(bytes);
            }
            Effect::Len { file, len } => {
                let n = usize::try_from(*len).unwrap_or(usize::MAX);
                disk.data.entry(*file).or_default().resize(n, 0);
            }
            Effect::Rename { from, to } => {
                let Some(&f) = disk.names.get(from) else { return };
                if disk.names.get(to) == Some(&f) {
                    return; // POSIX: two names of one inode, nothing done
                }
                disk.names.remove(from);
                disk.names.insert(to.clone(), f);
            }
            Effect::Link { from, to } => {
                if let Some(&f) = disk.names.get(from)
                    && !disk.names.contains_key(to)
                {
                    disk.names.insert(to.clone(), f);
                }
            }
            Effect::Unlink { path } => {
                disk.names.remove(path);
            }
            Effect::Create { path, file } => {
                if !disk.names.contains_key(path) {
                    disk.names.insert(path.clone(), *file);
                    disk.data.entry(*file).or_default();
                }
            }
        }
    }
}

impl WriteOps for RecordingOps {
    fn pwrite(&mut self, file: &File, offset: u64, bytes: &[u8]) -> io::Result<()> {
        StdOps.pwrite(file, offset, bytes)?;
        let file_id = self.id(file);
        self.log.push(Op::Pwrite { file: file_id, offset, bytes: bytes.to_vec() });
        Ok(())
    }
    fn set_len(&mut self, file: &File, len: u64) -> io::Result<()> {
        StdOps.set_len(file, len)?;
        let file_id = self.id(file);
        self.log.push(Op::SetLen { file: file_id, len });
        Ok(())
    }
    fn fdatasync(&mut self, file: &File) -> io::Result<()> {
        StdOps.fdatasync(file)?;
        let file_id = self.id(file);
        self.log.push(Op::Fdatasync { file: file_id });
        Ok(())
    }
    fn fsync(&mut self, file: &File) -> io::Result<()> {
        StdOps.fsync(file)?;
        let file_id = self.id(file);
        self.log.push(Op::Fsync { file: file_id });
        Ok(())
    }
    fn rename(&mut self, from: &Path, to: &Path) -> io::Result<()> {
        StdOps.rename(from, to)?;
        self.log.push(Op::Rename { from: from.to_owned(), to: to.to_owned() });
        Ok(())
    }
    fn hard_link(&mut self, from: &Path, to: &Path) -> io::Result<()> {
        StdOps.hard_link(from, to)?;
        self.log.push(Op::HardLink { from: from.to_owned(), to: to.to_owned() });
        Ok(())
    }
    fn unlink(&mut self, path: &Path) -> io::Result<()> {
        StdOps.unlink(path)?;
        self.log.push(Op::Unlink { path: path.to_owned() });
        Ok(())
    }
    fn fsync_dir(&mut self, dir: &Path) -> io::Result<()> {
        StdOps.fsync_dir(dir)?;
        self.log.push(Op::FsyncDir { dir: dir.to_owned() });
        Ok(())
    }
    fn flock(&mut self, file: &File, mode: LockMode) -> io::Result<bool> {
        let held = StdOps.flock(file, mode)?;
        let file_id = self.id(file);
        self.log.push(Op::Flock { file: file_id, mode });
        Ok(held)
    }
    fn create_new(&mut self, path: &Path) -> io::Result<File> {
        let file = StdOps.create_new(path)?;
        self.files.push(dev_ino(&file));
        let file_id = number(self.files.len() - 1);
        self.log.push(Op::CreateNew { path: path.to_owned(), file: file_id });
        Ok(file)
    }
    fn copy_range(&mut self, from: &File, to: &File, len: u64) -> io::Result<()> {
        StdOps.copy_range(from, to, len)?;
        let (a, b) = (self.id(from), self.id(to));
        self.log.push(Op::CopyRange { from: a, to: b, len });
        Ok(())
    }
}

/// How the update's writes end when they do not succeed (IN-77e; WP-213:
/// WP-211 §C gave `write`, `patch` and `compact` an `IncrReason` error,
/// which cannot say "the link fails": a write's own failure is not a §7
/// reason).
#[derive(Debug)]
pub enum WriteFailure {
    /// A named reason found before anything the reason forbids was
    /// written (IN-77a's `output-changed` before W2/C3; W2's foreign file,
    /// put back; a block failing its hash before W1, IN-46b): the update
    /// ends and the link falls back as §7's row says.
    Fallback(IncrReason),
    /// A write, sync, rename or `set_len` failed (IN-77e): the update ends
    /// at once, no sync is retried, the link fails. `step` names IN-77's
    /// step (`W1`..`W6`, `C1`..`C5`, `compact`); `error` is the I/O error.
    /// Also W4's `EEXIST` (U-7: IN-77 "fails with `output-changed`;
    /// nothing more is written", so no full link may follow in this
    /// link): `step` `W4`, `error` of kind `AlreadyExists` whose text
    /// starts `output-changed`.
    Failed { step: &'static str, error: io::Error },
}

impl From<IncrReason> for WriteFailure {
    fn from(r: IncrReason) -> Self {
        Self::Fallback(r)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // spec: incr.md IN-77 (the recording type records each call, in order)
    #[test]
    fn recording_records_in_order() {
        let dir = std::env::temp_dir().join(format!("mortar-wp213-ops-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("f");
        let mut ops = RecordingOps::default();
        let f = ops.create_new(&path).unwrap();
        ops.pwrite(&f, 4, b"ab").unwrap();
        ops.fdatasync(&f).unwrap();
        assert_eq!(std::fs::read(&path).unwrap(), b"\0\0\0\0ab");
        ops.unlink(&path).unwrap();
        std::fs::remove_dir(&dir).unwrap();
        assert_eq!(ops.log.len(), 4);
        assert!(matches!(ops.log[1], Op::Pwrite { file: 0, offset: 4, .. }));
        assert_eq!(ops.log[2], Op::Fdatasync { file: 0 });
    }

    // spec: incr.md IN-77 (the replay: a write is durable after its
    // file's sync, a name after its directory's; every subset before)
    #[test]
    fn replay_keeps_what_a_sync_ordered() {
        let dir = std::env::temp_dir().join(format!("mortar-wp219-ops-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let (a, b) = (dir.join("a"), dir.join("b"));
        std::fs::write(&a, b"old").unwrap();
        let mut ops = RecordingOps::default();
        let f = File::options().read(true).write(true).open(&a).unwrap();
        ops.pwrite(&f, 0, b"new").unwrap();
        ops.fdatasync(&f).unwrap();
        ops.rename(&a, &b).unwrap();
        assert!(ops.unlink(&a).is_err(), "a failed call");
        ops.fsync_dir(&dir).unwrap();
        drop(f);
        std::fs::remove_file(&b).unwrap();
        std::fs::remove_dir(&dir).unwrap();
        assert_eq!(ops.log.len(), 4, "the failed unlink is not recorded");
        let mut before = Disk::default();
        before.names.insert(a.clone(), 0);
        before.data.insert(0, b"old".to_vec());
        let mut seen = Vec::new();
        let n = ops
            .power_loss(&before, 4, |k, d| {
                let at = |p: &Path| d.read(p).map(<[u8]>::to_vec);
                seen.push((k, at(&a), at(&b)));
            })
            .unwrap();
        let old = Some(b"old".to_vec());
        let new = Some(b"new".to_vec());
        // prefix 0: as before; 1: the write or not; 2: written; 3: the
        // rename or not; 4: renamed.
        assert_eq!(n, 1 + 2 + 1 + 2 + 1);
        assert_eq!(seen[0], (0, old.clone(), None));
        assert!(seen.contains(&(1, old, None)) && seen.contains(&(1, new.clone(), None)));
        assert_eq!(seen[3], (2, new.clone(), None));
        assert!(seen.contains(&(3, new.clone(), None)) && seen.contains(&(3, None, new.clone())));
        assert_eq!(*seen.last().unwrap(), (4, None, new));
    }
}
