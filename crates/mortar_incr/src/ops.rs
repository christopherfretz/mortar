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
    Pwrite { file: u32, offset: u64, bytes: Vec<u8> },
    SetLen { file: u32, len: u64 },
    Fdatasync { file: u32 },
    Fsync { file: u32 },
    Rename { from: PathBuf, to: PathBuf },
    HardLink { from: PathBuf, to: PathBuf },
    Unlink { path: PathBuf },
    FsyncDir { dir: PathBuf },
    Flock { file: u32, mode: LockMode },
    CreateNew { path: PathBuf },
    CopyRange { from: u32, to: u32, len: u64 },
}

/// The crash harness's [`WriteOps`] (IN-77's power-loss test): the TYPE is
/// U-1's, its behaviour U-7's. Until U-7 lands it records every call in
/// [`RecordingOps::log`] and performs it through [`StdOps`]; U-7 decides
/// what it records (descriptors by `(dev, ino)`, injected failures) and
/// writes the replay onto byte buffers.
#[derive(Debug, Default)]
pub struct RecordingOps {
    /// Every call, in order.
    pub log: Vec<Op>,
    /// `(dev, ino)` of each descriptor seen, its index the `file` of an
    /// [`Op`].
    pub files: Vec<(u64, u64)>,
}

impl RecordingOps {
    fn id(&mut self, file: &File) -> u32 {
        use std::os::unix::fs::MetadataExt;
        let key = file.metadata().map_or((0, 0), |m| (m.dev(), m.ino()));
        let at = self.files.iter().position(|k| *k == key).unwrap_or_else(|| {
            self.files.push(key);
            self.files.len() - 1
        });
        u32::try_from(at).unwrap_or(u32::MAX)
    }
}

impl WriteOps for RecordingOps {
    fn pwrite(&mut self, file: &File, offset: u64, bytes: &[u8]) -> io::Result<()> {
        let file_id = self.id(file);
        self.log.push(Op::Pwrite { file: file_id, offset, bytes: bytes.to_vec() });
        StdOps.pwrite(file, offset, bytes)
    }
    fn set_len(&mut self, file: &File, len: u64) -> io::Result<()> {
        let file_id = self.id(file);
        self.log.push(Op::SetLen { file: file_id, len });
        StdOps.set_len(file, len)
    }
    fn fdatasync(&mut self, file: &File) -> io::Result<()> {
        let file_id = self.id(file);
        self.log.push(Op::Fdatasync { file: file_id });
        StdOps.fdatasync(file)
    }
    fn fsync(&mut self, file: &File) -> io::Result<()> {
        let file_id = self.id(file);
        self.log.push(Op::Fsync { file: file_id });
        StdOps.fsync(file)
    }
    fn rename(&mut self, from: &Path, to: &Path) -> io::Result<()> {
        self.log.push(Op::Rename { from: from.to_owned(), to: to.to_owned() });
        StdOps.rename(from, to)
    }
    fn hard_link(&mut self, from: &Path, to: &Path) -> io::Result<()> {
        self.log.push(Op::HardLink { from: from.to_owned(), to: to.to_owned() });
        StdOps.hard_link(from, to)
    }
    fn unlink(&mut self, path: &Path) -> io::Result<()> {
        self.log.push(Op::Unlink { path: path.to_owned() });
        StdOps.unlink(path)
    }
    fn fsync_dir(&mut self, dir: &Path) -> io::Result<()> {
        self.log.push(Op::FsyncDir { dir: dir.to_owned() });
        StdOps.fsync_dir(dir)
    }
    fn flock(&mut self, file: &File, mode: LockMode) -> io::Result<bool> {
        let file_id = self.id(file);
        self.log.push(Op::Flock { file: file_id, mode });
        StdOps.flock(file, mode)
    }
    fn create_new(&mut self, path: &Path) -> io::Result<File> {
        self.log.push(Op::CreateNew { path: path.to_owned() });
        let file = StdOps.create_new(path)?;
        self.id(&file);
        Ok(file)
    }
    fn copy_range(&mut self, from: &File, to: &File, len: u64) -> io::Result<()> {
        let (a, b) = (self.id(from), self.id(to));
        self.log.push(Op::CopyRange { from: a, to: b, len });
        StdOps.copy_range(from, to, len)
    }
}

/// How the update's writes end when they do not succeed (IN-77e; WP-213:
/// WP-211 §C gave `write`, `patch` and `compact` an `IncrReason` error,
/// which cannot say "the link fails": a write's own failure is not a §7
/// reason).
#[derive(Debug)]
pub enum WriteFailure {
    /// A named reason found before anything the reason forbids was
    /// written (IN-77a's `output-changed` before W2/C3; W2's and W4's
    /// foreign file; a block failing its hash before W1, IN-46b): the
    /// update ends and the link falls back as §7's row says.
    Fallback(IncrReason),
    /// A write, sync, rename or `set_len` failed (IN-77e): the update ends
    /// at once, no sync is retried, the link fails. `step` names IN-77's
    /// step (`W1`..`W6`, `C1`..`C5`, `compact`); `error` is the I/O error.
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
}
