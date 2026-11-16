//! The output file (§7): a temporary next to `P` under a lock, the old
//! output set aside and unlinked on a helper thread, the rename; a
//! character device written in place; FIFOs, sockets, block devices and
//! directories refused by name; nothing left after a failure.

use std::fs::{self, File, OpenOptions};
use std::io::{self, ErrorKind};
use std::os::unix::fs::{FileTypeExt, MetadataExt, OpenOptionsExt};
use std::path::{Path, PathBuf};
use std::thread::JoinHandle;

use mortar_diag::{Diagnostic, Key, Kind};

use crate::PHASE_OUTPUT;
use crate::fill::{Sink, write_error};

/// A file's identity: `(st_dev, st_ino)`, following symbolic links.
pub type FileIdentity = (u64, u64);

/// The identity of `path`, following symbolic links.
#[must_use]
pub fn identity(path: &Path) -> Option<FileIdentity> {
    fs::metadata(path).ok().map(|m| (m.dev(), m.ino()))
}

/// Whether the file system's `flock` is probed (I-59) or assumed absent.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum LockMode {
    /// Probe with a second descriptor (I-59).
    #[default]
    Probe,
    /// Behave as on a file system that does not enforce `flock`: never
    /// reuse a name (I-59's last case; a test of that case on `/tmp`).
    Unenforced,
}

/// How the output is created.
#[derive(Debug, Clone, Default)]
pub struct OutputOptions {
    /// `0o777` for executables and shared objects, `0o666` for `-r`
    /// (I-59); the kernel applies the umask.
    pub mode: u32,
    /// The identities of every input file (I-66, I-113; the facade
    /// refuses an output that is one of them before this, I-62).
    pub inputs: Vec<FileIdentity>,
    pub lock: LockMode,
}

impl OutputOptions {
    /// The options of an executable or shared object.
    #[must_use]
    pub fn executable(inputs: Vec<FileIdentity>) -> Self {
        Self { mode: 0o777, inputs, lock: LockMode::Probe }
    }
}

#[derive(Debug)]
enum Target {
    /// A character device, written in place (I-58).
    InPlace(File),
    /// The temporary `T`, renamed to `P` at the end (I-59, I-60).
    Temp { file: File, path: PathBuf },
}

/// The output file of one link, between creation and commit or abort.
#[derive(Debug)]
pub struct Output {
    path: PathBuf,
    shown: String,
    target: Option<Target>,
    aside: Option<PathBuf>,
    helper: Option<JoinHandle<()>>,
}

/// `cannot-open-output` naming `P` (I-57..I-59, I-68).
fn cannot_open(shown: &str, reason: String) -> (Key, Diagnostic) {
    let kind = Kind::CannotOpenOutput { path: shown.to_owned(), reason };
    (Key::new(PHASE_OUTPUT, 0, 0), Diagnostic::error(kind))
}

fn open_reason(e: &io::Error) -> String {
    mortar_diag::os_reason(e)
}

/// `dir(P)` and `base(P)`.
fn split(path: &Path) -> (PathBuf, std::ffi::OsString) {
    let dir = match path.parent() {
        Some(d) if !d.as_os_str().is_empty() => d.to_path_buf(),
        _ => PathBuf::from("."),
    };
    let base = path.file_name().map(ToOwned::to_owned).unwrap_or_default();
    (dir, base)
}

/// `dir(P)/.<base(P)><suffix>`.
fn sibling(path: &Path, suffix: &str) -> PathBuf {
    let (dir, base) = split(path);
    let mut name = std::ffi::OsString::from(".");
    name.push(base);
    name.push(suffix);
    dir.join(name)
}

/// `T0` (I-59).
#[must_use]
pub fn temp_name(path: &Path) -> PathBuf {
    sibling(path, ".mortar-tmp")
}

/// `A` (I-113).
#[must_use]
pub fn aside_name(path: &Path) -> PathBuf {
    sibling(path, ".mortar-old")
}

fn create_new(path: &Path, mode: u32) -> io::Result<File> {
    OpenOptions::new().write(true).create_new(true).mode(mode).open(path)
}

/// Whether `path` names the inode `file` has open (a lock on an inode
/// unlinked since is no lock on `path`).
fn is_path_of(file: &File, path: &Path) -> bool {
    match (file.metadata(), fs::metadata(path)) {
        (Ok(a), Ok(b)) => a.dev() == b.dev() && a.ino() == b.ino(),
        _ => false,
    }
}

/// Whether `file`'s exclusive lock is enforced against a second descriptor
/// of `path` (I-59's probe).
fn enforced(path: &Path) -> bool {
    let Ok(other) = OpenOptions::new().write(true).open(path) else { return false };
    matches!(other.try_lock(), Err(fs::TryLockError::WouldBlock))
}

/// What a path the link writes is used for: the output, positioned
/// writes (I-63), or a file written whole, in order (I-79, I-79a).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Use {
    Output,
    Whole,
}

/// What the link may do with a path it writes (I-58b): the one decision,
/// made by [`disposition`], that every writer, remover and renamer of a
/// path the command line named obeys.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Disposition {
    /// No file, a regular file, or an ordinary symbolic link (to a
    /// regular file, or dangling) in a directory the link may create files
    /// in: a temporary beside it renamed over it (I-59, I-60); a symbolic
    /// link is itself replaced, its target untouched.
    Replace,
    /// Opened for writing without creating it, and written in place: a
    /// character device, a FIFO written whole (I-58, I-79a), and a regular
    /// file reached through a descriptor table, which is truncated (I-58a).
    /// Never renamed over or removed.
    InPlace { truncate: bool },
    /// `cannot-open-output` naming the path (I-57, I-58: a directory, a
    /// FIFO output, a socket, a block device; a path whose own `lstat`
    /// fails), with the system's reason and a note; nothing is created,
    /// renamed or removed.
    Refused { reason: String, note: Option<&'static str> },
    /// `cannot-write-output` naming the path (I-58b): a path the link may
    /// neither write in place nor replace (a descriptor alias that does not
    /// resolve or resolves to a file of no type, a chain of symbolic links
    /// that cannot be examined, a name in a system directory), with the
    /// reason and a note; nothing is created, renamed or removed.
    Forbidden { reason: String, note: Option<&'static str> },
}

/// I-58b's note for a symbolic link through a descriptor table whose file
/// cannot be written in place.
pub const NOTE_ALIAS: &str = "a symbolic link through a descriptor table (/dev/stdout, /dev/fd/N) \
     is written in place or refused, never replaced";
/// I-58b's note for a chain of symbolic links that cannot be examined.
pub const NOTE_CHAIN: &str =
    "a symbolic link that cannot be followed to its end is neither written nor replaced";
/// I-58b's note for a file of no type (an anonymous descriptor).
pub const NOTE_NO_TYPE: &str =
    "it is of no file type (an eventfd, epoll, pidfd or other anonymous descriptor)";
/// I-58b's note for a path in a system directory.
pub const NOTE_SYSTEM: &str = "mortar creates, renames and removes no file in /dev, /proc or /sys";

fn refused(reason: impl Into<String>, note: Option<&'static str>) -> Disposition {
    Disposition::Refused { reason: reason.into(), note }
}

fn forbidden(reason: impl Into<String>, note: Option<&'static str>) -> Disposition {
    Disposition::Forbidden { reason: reason.into(), note }
}

/// I-58b: what the link may do with `path`, from its type and, for a
/// symbolic link, from its chain and its target's type. Every outcome of
/// resolving it has a row: a link through a descriptor table is written in
/// place or refused, never replaced; a path that cannot be examined
/// (`EACCES`, `ELOOP`, `ENOTDIR`, a descriptor that does not resolve) is
/// refused; a file of no type is refused; nothing in `/dev` (the directory
/// itself), `/proc` or `/sys` is created, renamed over or removed.
#[must_use]
pub fn disposition(path: &Path, used: Use) -> Disposition {
    let link = match fs::symlink_metadata(path) {
        Ok(m) => m,
        Err(e) if e.kind() == ErrorKind::NotFound => return replace_in(path),
        Err(e) => return refused(open_reason(&e), None),
    };
    if !link.file_type().is_symlink() {
        return by_type(&link, used, path, false);
    }
    let alias = match descriptor_chain(path) {
        Ok(alias) => alias,
        Err(e) => return forbidden(open_reason(&e), Some(NOTE_CHAIN)),
    };
    match fs::metadata(path) {
        Ok(m) => by_type(&m, used, path, alias),
        // An ordinary dangling symbolic link: replaced, as lld does.
        Err(e) if !alias && e.kind() == ErrorKind::NotFound => replace_in(path),
        Err(e) if alias => forbidden(open_reason(&e), Some(NOTE_ALIAS)),
        Err(e) => forbidden(open_reason(&e), Some(NOTE_CHAIN)),
    }
}

/// `Replace` unless `dir(P)` is a system directory.
fn replace_in(path: &Path) -> Disposition {
    if system_dir(&split(path).0) {
        forbidden("a system directory", Some(NOTE_SYSTEM))
    } else {
        Disposition::Replace
    }
}

/// The row of a file of type `m` (following symbolic links); `alias`: `P`
/// reaches it through a descriptor table.
fn by_type(m: &fs::Metadata, used: Use, path: &Path, alias: bool) -> Disposition {
    let t = m.file_type();
    if t.is_dir() {
        refused("Is a directory", None)
    } else if t.is_file() {
        if alias { Disposition::InPlace { truncate: true } } else { replace_in(path) }
    } else if t.is_char_device() {
        Disposition::InPlace { truncate: false }
    } else if t.is_fifo() {
        match used {
            Use::Whole => Disposition::InPlace { truncate: false },
            Use::Output => refused("not a regular file", Some("output is a FIFO")),
        }
    } else if t.is_socket() {
        refused("not a regular file", Some("output is a socket"))
    } else if t.is_block_device() {
        refused("not a regular file", Some("output is a block device"))
    } else {
        forbidden("not a regular file", Some(NOTE_NO_TYPE))
    }
}

/// `/dev` itself, or `/proc` or `/sys` and below, after resolving `dir`'s
/// symbolic links (`/dev/shm` and other file systems mounted under `/dev`
/// are ordinary directories). A directory that cannot be resolved is not
/// one: creating `T` in it fails by name (I-59).
#[must_use]
pub fn system_dir(dir: &Path) -> bool {
    let Ok(c) = fs::canonicalize(dir) else { return false };
    c == Path::new("/dev") || c.starts_with("/proc") || c.starts_with("/sys")
}

/// Whether the link may replace or remove `path` (I-58b): `Ok` for
/// [`Disposition::Replace`], else the reason. For writers of their own
/// (the incremental state) and removers.
pub fn may_replace(path: &Path) -> Result<(), String> {
    match disposition(path, Use::Whole) {
        Disposition::Replace => Ok(()),
        Disposition::InPlace { .. } => {
            Err(format!("{} is not a regular file mortar may replace (I-58b)", path.display()))
        }
        Disposition::Refused { reason, note } | Disposition::Forbidden { reason, note } => {
            Err(match note {
                Some(n) => format!("{}: {reason} ({n})", path.display()),
                None => format!("{}: {reason}", path.display()),
            })
        }
    }
}

/// Removes `path` if the link may (I-58b); otherwise leaves it.
pub fn remove_if_replaceable(path: &Path) -> bool {
    may_replace(path).is_ok() && fs::remove_file(path).is_ok()
}

impl Output {
    /// Creates the output for `path` (I-56..I-59, I-58b, I-113). `path` is
    /// shown as given in every diagnostic.
    pub fn create(path: &Path, opts: &OutputOptions) -> Result<Self, (Key, Diagnostic)> {
        Self::create_as(path, opts, disposition(path, Use::Output))
    }

    fn create_as(
        path: &Path,
        opts: &OutputOptions,
        disposition: Disposition,
    ) -> Result<Self, (Key, Diagnostic)> {
        let shown = path.to_string_lossy().into_owned();
        match disposition {
            Disposition::Refused { reason, note } => {
                let (key, d) = cannot_open(&shown, reason);
                return Err((
                    key,
                    match note {
                        Some(n) => d.note(n),
                        None => d,
                    },
                ));
            }
            Disposition::Forbidden { reason, note } => {
                let kind = Kind::CannotWriteOutput { path: shown, reason };
                let d = Diagnostic::error(kind);
                return Err((
                    Key::new(PHASE_OUTPUT, 0, 0),
                    match note {
                        Some(n) => d.note(n),
                        None => d,
                    },
                ));
            }
            Disposition::InPlace { truncate } => {
                // I-58, I-58a: opened without creating it; never renamed.
                let file = OpenOptions::new()
                    .write(true)
                    .truncate(truncate)
                    .open(path)
                    .map_err(|e| cannot_open(&shown, open_reason(&e)))?;
                return Ok(Self {
                    path: path.to_path_buf(),
                    shown,
                    target: Some(Target::InPlace(file)),
                    aside: None,
                    helper: None,
                });
            }
            Disposition::Replace => {}
        }
        let link = fs::symlink_metadata(path).ok();
        let (file, tmp) =
            Self::temp(path, opts).map_err(|e| cannot_open(&shown, open_reason(&e)))?;
        let mut out = Self {
            path: path.to_path_buf(),
            shown,
            target: Some(Target::Temp { file, path: tmp }),
            aside: None,
            helper: None,
        };
        // I-113: set the old output aside, unless it is an input (a guard
        // the facade can no longer trigger: I-62 refuses such a link).
        if let Some(m) = link
            && (m.file_type().is_file() || m.file_type().is_symlink())
            && identity(path).is_none_or(|id| !opts.inputs.contains(&id))
        {
            let a = aside_name(path);
            if fs::rename(path, &a).is_ok() {
                let unlink = a.clone();
                out.helper = std::thread::Builder::new()
                    .name("mortar-unlink".into())
                    .spawn(move || {
                        let _ = fs::remove_file(&unlink);
                    })
                    .ok();
                if out.helper.is_none() {
                    let _ = fs::remove_file(&a);
                }
                out.aside = Some(a);
            }
        }
        Ok(out)
    }

    /// I-59: `T0` under a lock when the file system enforces it, else
    /// `T0.<pid>` (then `.1` … `.63`).
    fn temp(path: &Path, opts: &OutputOptions) -> io::Result<(File, PathBuf)> {
        let t0 = temp_name(path);
        if opts.lock == LockMode::Probe {
            // `create_new` first, to know whether `T0` is ours to remove.
            let (file, fresh) = match create_new(&t0, opts.mode) {
                Ok(f) => (f, true),
                Err(e) if e.kind() == ErrorKind::AlreadyExists => {
                    (OpenOptions::new().write(true).open(&t0)?, false)
                }
                Err(e) => return Err(e),
            };
            match file.try_lock() {
                // Locked an inode another link has since unlinked (it
                // replaced a stale `T0`): `T0` is not ours.
                Ok(()) if !is_path_of(&file, &t0) => {}
                Ok(()) if enforced(&t0) => {
                    if fresh {
                        return Ok((file, t0));
                    }
                    // A stale `T0` of a killed link: its owner is dead. It
                    // is replaced by a new inode, so that the kernel gives it
                    // the mode of this link (a truncated stale file would
                    // keep the old mode). The lock is held on the old inode
                    // until the new one is locked.
                    fs::remove_file(&t0)?;
                    // If another link takes the name between, it is theirs.
                    if let Ok(new) = create_new(&t0, opts.mode)
                        && new.try_lock().is_ok()
                        && is_path_of(&new, &t0)
                    {
                        return Ok((new, t0));
                    }
                }
                Ok(()) | Err(fs::TryLockError::Error(_)) => {
                    // Not enforced, or unsupported: never reuse a name.
                    drop(file);
                    if fresh {
                        let _ = fs::remove_file(&t0);
                    }
                }
                Err(fs::TryLockError::WouldBlock) => {
                    // Another link to `P` is running; `T0` is theirs.
                }
            }
        }
        let pid = std::process::id();
        let mut last = None;
        for k in 0..64u32 {
            let mut name = t0.clone().into_os_string();
            name.push(format!(".{pid}"));
            if k > 0 {
                name.push(format!(".{k}"));
            }
            let t = PathBuf::from(name);
            match create_new(&t, opts.mode) {
                Ok(f) => return Ok((f, t)),
                Err(e) if e.kind() == ErrorKind::AlreadyExists => last = Some(e),
                Err(e) => return Err(e),
            }
        }
        Err(last.unwrap_or_else(|| io::Error::from(ErrorKind::AlreadyExists)))
    }

    /// The path as the user gave it.
    #[must_use]
    pub fn shown(&self) -> &str {
        &self.shown
    }

    /// The temporary's path, when there is one.
    #[must_use]
    pub fn temp_path(&self) -> Option<&Path> {
        match &self.target {
            Some(Target::Temp { path, .. }) => Some(path),
            _ => None,
        }
    }

    /// The path the old output was set aside to (I-113).
    #[must_use]
    pub fn aside_path(&self) -> Option<&Path> {
        self.aside.as_deref()
    }

    fn file(&self) -> Option<&File> {
        match &self.target {
            Some(Target::InPlace(f) | Target::Temp { file: f, .. }) => Some(f),
            None => None,
        }
    }

    /// I-60: renames `T` to `P` (while `T0`'s lock is still held), then
    /// closes it; a failed rename is `cannot-write-output` and `T` is
    /// removed. No `fsync` (I-61).
    pub fn commit(self) -> Result<(), (Key, Diagnostic)> {
        self.commit_with(false).map(|_| ())
    }

    /// [`Output::commit`], also returning the device and inode of the file
    /// written, from `fstat` of its descriptor before the rename
    /// (`spec/incr.md` IN-21: an incremental-mode link binds its state to
    /// this file, not to whatever the path names later). `None` when the
    /// `fstat` failed.
    pub fn commit_identified(self) -> Result<Option<(u64, u64)>, (Key, Diagnostic)> {
        self.commit_with(true)
    }

    fn commit_with(mut self, identify: bool) -> Result<Option<(u64, u64)>, (Key, Diagnostic)> {
        let id = |f: &File| {
            use std::os::unix::fs::MetadataExt as _;
            if identify { f.metadata().ok().map(|m| (m.dev(), m.ino())) } else { None }
        };
        let result = match self.target.take() {
            Some(Target::Temp { file, path }) => {
                let written = id(&file);
                let r = fs::rename(&path, &self.path).map(|()| written);
                drop(file);
                r.map_err(|e| {
                    let _ = fs::remove_file(&path);
                    write_error(&self.shown, &e)
                })
            }
            Some(Target::InPlace(file)) => {
                let written = id(&file);
                drop(file);
                Ok(written)
            }
            None => Ok(None),
        };
        self.join();
        result
    }

    /// I-65: the link failed; removes `T` and `A`, renames nothing back.
    pub fn abort(mut self) {
        if let Some(Target::Temp { file, path }) = self.target.take() {
            drop(file);
            let _ = fs::remove_file(&path);
        }
        self.join();
    }

    fn join(&mut self) {
        if let Some(h) = self.helper.take() {
            let _ = h.join();
        }
        // The helper removed `A`; a later `A` belongs to a later link.
        self.aside = None;
    }
}

impl Drop for Output {
    /// Neither committed nor aborted: abort.
    fn drop(&mut self) {
        if let Some(Target::Temp { path, .. }) = self.target.take() {
            let _ = fs::remove_file(&path);
        }
        self.join();
    }
}

impl Sink for Output {
    fn write_at(&self, offset: u64, bytes: &[u8]) -> io::Result<()> {
        match self.file() {
            Some(f) => f.write_at(offset, bytes),
            None => Err(io::Error::other("output already closed")),
        }
    }
}

/// I-66: after a failure at a phase after the command line, an existing
/// regular output (or an ordinary symbolic link at `P`, the link itself)
/// is removed unless it is the same file as an input. Only a path the link
/// may replace is removed (I-58b): a non-regular `P`, a symbolic link to
/// one or through a descriptor table (`/dev/stdout`), and anything in a
/// system directory are left alone. Returns whether something was
/// removed.
pub fn remove_stale_output(path: &Path, inputs: &[FileIdentity]) -> bool {
    let Ok(m) = fs::symlink_metadata(path) else { return false };
    let t = m.file_type();
    if !(t.is_file() || t.is_symlink()) || may_replace(path).is_err() {
        return false;
    }
    if identity(path).is_some_and(|id| inputs.contains(&id)) {
        return false;
    }
    fs::remove_file(path).is_ok()
}

/// Whether `path` reaches its file through a process's descriptor table
/// (`/dev/stdout`, `/dev/fd/1`, `/proc/self/fd/1`; I-58a): one of the
/// symbolic links it follows is an entry of a `/proc/<pid>/fd` directory.
/// `false` when the chain cannot be examined; [`disposition`] refuses such
/// a path instead.
#[must_use]
pub fn through_descriptor(path: &Path) -> bool {
    descriptor_chain(path).unwrap_or(false)
}

/// I-58b: walks the chain of symbolic links of `path` without following
/// the last one's target: `Ok(true)` as soon as a link is an entry of a
/// descriptor table, `Ok(false)` at the first name that is not a symbolic
/// link (or does not exist), and the error of any step that cannot be
/// examined (`EACCES`, `ENOTDIR`; `ELOOP` past the kernel's 40 links).
fn descriptor_chain(path: &Path) -> io::Result<bool> {
    let mut p = path.to_path_buf();
    // The kernel's limit on nested symbolic links.
    for _ in 0..40 {
        let dir = split(&p).0;
        // Each name is judged by its directory before it is examined: an
        // entry of a descriptor table whose descriptor is closed does not
        // exist, and is still not an ordinary dangling link.
        match fs::canonicalize(&dir) {
            Ok(c) if is_descriptor_dir(&c) => return Ok(true),
            Ok(_) => {}
            Err(e) if e.kind() == ErrorKind::NotFound => return Ok(false),
            Err(e) => return Err(e),
        }
        let m = match fs::symlink_metadata(&p) {
            Ok(m) => m,
            Err(e) if e.kind() == ErrorKind::NotFound => return Ok(false),
            Err(e) => return Err(e),
        };
        if !m.file_type().is_symlink() {
            return Ok(false);
        }
        let target = fs::read_link(&p)?;
        p = dir.join(target);
    }
    // ELOOP: more symbolic links than the kernel follows.
    Err(io::Error::from_raw_os_error(40))
}

/// `/proc/<pid>/fd` or `/proc/<pid>/task/<tid>/fd`.
fn is_descriptor_dir(dir: &Path) -> bool {
    let parts: Vec<&std::ffi::OsStr> = dir.iter().collect();
    let digits = |s: &std::ffi::OsStr| {
        let b = s.as_encoded_bytes();
        !b.is_empty() && b.iter().all(u8::is_ascii_digit)
    };
    match parts.as_slice() {
        [root, proc, pid, fd] => *root == "/" && *proc == "proc" && digits(pid) && *fd == "fd",
        [root, proc, pid, task, tid, fd] => {
            *root == "/"
                && *proc == "proc"
                && digits(pid)
                && *task == "task"
                && digits(tid)
                && *fd == "fd"
        }
        _ => false,
    }
}

/// Writes `bytes` to `path` with the output's protocol (I-79: the map
/// file), or in order and in place for a stream (I-79a), as
/// [`disposition`] says (I-58b).
pub fn write_whole(path: &Path, bytes: &[u8], mode: u32) -> Result<(), (Key, Diagnostic)> {
    let disposition = disposition(path, Use::Whole);
    if let Disposition::InPlace { truncate } = disposition {
        use std::io::Write as _;
        let shown = path.to_string_lossy().into_owned();
        let mut file = OpenOptions::new()
            .write(true)
            .truncate(truncate)
            .open(path)
            .map_err(|e| cannot_open(&shown, open_reason(&e)))?;
        return file.write_all(bytes).map_err(|e| write_error(&shown, &e));
    }
    let opts = OutputOptions { mode, inputs: Vec::new(), lock: LockMode::Probe };
    let out = Output::create_as(path, &opts, disposition)?;
    if let Err(e) = out.write_at(0, bytes) {
        let err = write_error(out.shown(), &e);
        out.abort();
        return Err(err);
    }
    out.commit()
}
