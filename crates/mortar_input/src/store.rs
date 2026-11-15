//! The store (`spec/input.md` §11): it owns every byte the link reads from
//! its inputs, and is shared and immutable to its users.
//!
//! This is the M1 store, [`StorePolicy::Resident`] (§11.1 (a)): every file
//! is read whole into one owned buffer, its one part. The interface is the
//! one of §1 that the later staged store also satisfies: [`Store::resident`]
//! hands out a [`Resident`] only for a range inside one part, and
//! [`Store::bytes`] cannot fail; [`Store::copy_to`] copies any range of the
//! file.
//!
//! Entries live in an append-only arena of fixed segments (segment `k`
//! holds `2^k` once-cells, allocated on first use and never moved), so a
//! `&[u8]` handed out stays valid for as long as the store is borrowed.
//!
//! Reading (§11.2): `stat` (following symbolic links, before anything is
//! opened: a FIFO is never opened, §2.5), `open`, `fstat`, positioned reads
//! into a buffer of exactly the file's size (a file above [`CHUNK`] in
//! chunks read by several workers into disjoint slices), a read at the end
//! that must return nothing, a second `fstat` that must agree, `close`.
//! A file is open only while it is read (§11.4).

use std::collections::HashMap;
use std::fs::File;
use std::io::{self, Read};
use std::ops::Range;
use std::os::unix::fs::{FileExt, FileTypeExt, MetadataExt};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU32, AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, OnceLock, PoisonError};

use mortar_core::hash::{combine, hash_bytes, hash_u64};
use mortar_core::inputs::{self, ContentHash};
use mortar_core::par::Pool;
use mortar_diag::os_reason;

/// The size of one chunk of a parallel read, and the largest part one
/// worker reads alone (§11.2 step 3). Also the block of the content hash.
pub const CHUNK: u64 = 4 << 20;

/// The smallest file size refused (§11.2): diagnostic keys hold 40 bits of
/// offset.
pub const MAX_INPUT: u64 = 1 << 40;

/// Which store (§11.1).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum StorePolicy {
    /// Every file read whole: one part, the whole file (M1).
    #[default]
    Resident,
}

/// A distinct file read (§9.3), in the store's own order. Only the store
/// and the phase that reads files see it; the order is not observable.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Blob(u32);

impl Blob {
    /// The blob's index in the store.
    #[must_use]
    pub const fn index(self) -> u32 {
        self.0
    }
}

/// A range of a blob that lies inside one resident part (§11.3). Made only
/// by [`Store::resident`], so [`Store::bytes`] cannot fail.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Resident {
    blob: Blob,
    start: u64,
    end: u64,
}

impl Resident {
    /// The blob.
    #[must_use]
    pub const fn blob(self) -> Blob {
        self.blob
    }

    /// The range.
    #[must_use]
    pub const fn range(self) -> Range<u64> {
        self.start..self.end
    }
}

/// What `fstat` says about an open file, and what `stat` says about a
/// regular file or a character device.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct FileMeta {
    pub size: u64,
    pub dev: u64,
    pub ino: u64,
    /// `st_mtime`: seconds, nanoseconds.
    pub mtime: (i64, i64),
    /// `st_ctime`: seconds, nanoseconds.
    pub ctime: (i64, i64),
}

impl FileMeta {
    fn of(m: &std::fs::Metadata) -> Self {
        Self {
            size: m.len(),
            dev: m.dev(),
            ino: m.ino(),
            mtime: (m.mtime(), m.mtime_nsec()),
            ctime: (m.ctime(), m.ctime_nsec()),
        }
    }
}

/// The result of `stat` on a path, following symbolic links (§2.5, §4.4).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Stat {
    /// `ENOENT` or `ENOTDIR`; a dangling symbolic link.
    Absent,
    Regular(FileMeta),
    Directory,
    CharDevice(FileMeta),
    Fifo,
    Socket,
    BlockDevice,
    /// Any other error (`ELOOP`, `EACCES`): the errno and the system's text.
    Error {
        errno: i32,
        reason: String,
    },
}

impl Stat {
    /// Whether the candidate exists for the search of §4.4.
    #[must_use]
    pub fn exists(&self) -> bool {
        !matches!(self, Self::Absent)
    }
}

/// The ways a file that exists is not linked whole and unchanged (§11.2,
/// §11.5, §2.5). Comparable, so that runs can be compared.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum ReadError {
    /// It does not exist (`ENOENT`), at `stat` or at `open`.
    Missing,
    /// `open` or a read failed: the errno and the system's text.
    Os { errno: Option<i32>, reason: String },
    /// It is a directory.
    Directory,
    /// It is not a regular file, and cannot be read (§2.5): the text.
    NotRegular(&'static str),
    /// It changed while it was read (§11.5).
    Changed,
    /// It is too large (§11.2): the refused feature's name.
    TooLarge(&'static str),
    /// `copy_to` was asked for bytes past the end of the file.
    OutOfRange,
}

impl ReadError {
    fn os(error: &io::Error) -> Self {
        if error.kind() == io::ErrorKind::NotFound {
            return Self::Missing;
        }
        Self::Os { errno: error.raw_os_error(), reason: os_reason(error) }
    }
}

/// A file to read: the path the system calls use and what `stat` said.
#[derive(Clone, Debug)]
pub struct ReadJob {
    pub path: PathBuf,
    pub stat: Stat,
}

/// The system calls the store made, for the measurements of
/// `spec/landed/input/WP-021.md`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct StoreStats {
    pub stats: u64,
    pub opens: u64,
    pub fstats: u64,
    pub reads: u64,
    pub closes: u64,
    /// The most descriptors the store held open at once.
    pub peak_open: u64,
    pub files: u64,
    pub bytes: u64,
}

/// A test hook, called with the path of the file.
pub type Hook = Box<dyn Fn(&Path) + Send + Sync>;

/// Test hooks (`spec/input.md` §16: `s11_5_short_read`).
#[derive(Default)]
pub struct Hooks {
    /// Called after a file is opened and `fstat`ed, before it is read.
    pub after_fstat: Option<Hook>,
}

impl std::fmt::Debug for Hooks {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Hooks").field("after_fstat", &self.after_fstat.is_some()).finish()
    }
}

struct Entry {
    bytes: Box<[u8]>,
    meta: FileMeta,
    hash: ContentHash,
}

const SEGMENTS: usize = 32;

/// One kept string's slot.
type Kept = OnceLock<Box<[u8]>>;

/// The store. `Sync`, append-only.
pub struct Store {
    policy: StorePolicy,
    segments: [OnceLock<Box<[OnceLock<Entry>]>>; SEGMENTS],
    len: AtomicU32,
    by_inode: Mutex<HashMap<(u64, u64), Blob>>,
    hooks: Hooks,
    counters: Counters,
    /// Bytes the tables borrow that are no file's: found paths, member
    /// names of thin archives (§1: `Inputs<'s>` borrows from the store).
    kept: [OnceLock<Box<[Kept]>>; SEGMENTS],
    kept_len: AtomicU32,
}

#[derive(Default)]
struct Counters {
    stats: AtomicU64,
    opens: AtomicU64,
    fstats: AtomicU64,
    reads: AtomicU64,
    closes: AtomicU64,
    open_now: AtomicUsize,
    peak_open: AtomicUsize,
    bytes: AtomicU64,
}

impl std::fmt::Debug for Store {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Store")
            .field("policy", &self.policy)
            .field("blobs", &self.blob_count())
            .finish_non_exhaustive()
    }
}

/// Segment and slot of blob `index`: segment `k` holds `2^k` slots.
fn locate(index: u32) -> (usize, usize) {
    let n = u64::from(index) + 1;
    let k = n.ilog2();
    (k as usize, (n - (1 << k)) as usize)
}

impl Store {
    /// An empty store.
    #[must_use]
    pub fn new(policy: StorePolicy) -> Self {
        Self::with_hooks(policy, Hooks::default())
    }

    /// An empty store with test hooks.
    #[must_use]
    pub fn with_hooks(policy: StorePolicy, hooks: Hooks) -> Self {
        Self {
            policy,
            segments: std::array::from_fn(|_| OnceLock::new()),
            len: AtomicU32::new(0),
            by_inode: Mutex::new(HashMap::new()),
            hooks,
            counters: Counters::default(),
            kept: std::array::from_fn(|_| OnceLock::new()),
            kept_len: AtomicU32::new(0),
        }
    }

    /// The policy.
    #[must_use]
    pub fn policy(&self) -> StorePolicy {
        self.policy
    }

    /// The number of blobs.
    #[must_use]
    pub fn blob_count(&self) -> u32 {
        self.len.load(Ordering::Acquire)
    }

    fn entry(&self, b: Blob) -> &Entry {
        self.try_entry(b.0)
            .unwrap_or_else(|| unreachable!("blob {} was handed out by this store", b.0))
    }

    fn try_entry(&self, index: u32) -> Option<&Entry> {
        let (k, slot) = locate(index);
        self.segments.get(k)?.get().and_then(|segment| segment.get(slot)?.get())
    }

    /// The model's handle of a blob (`spec/model.md` §3.2 `FileBytes`).
    #[must_use]
    pub const fn model_blob(b: Blob) -> inputs::Blob {
        inputs::Blob(b.0)
    }

    fn insert(&self, entry: Entry) -> Blob {
        let index = self.len.fetch_add(1, Ordering::AcqRel);
        assert!(index < u32::MAX, "more than 2^32 - 1 files read (spec/model.md §1)");
        let (k, slot) = locate(index);
        let segment =
            self.segments[k].get_or_init(|| (0..1usize << k).map(|_| OnceLock::new()).collect());
        if segment[slot].set(entry).is_err() {
            unreachable!("blob slot {index} filled twice");
        }
        Blob(index)
    }

    /// Keeps `bytes` for as long as the store lives (append-only, like
    /// the blobs).
    ///
    /// # Panics
    ///
    /// After 2^32 - 1 calls.
    pub fn keep(&self, bytes: &[u8]) -> &[u8] {
        let index = self.kept_len.fetch_add(1, Ordering::AcqRel);
        assert!(index < u32::MAX, "more than 2^32 - 1 kept strings");
        let (k, slot) = locate(index);
        let segment =
            self.kept[k].get_or_init(|| (0..1usize << k).map(|_| OnceLock::new()).collect());
        segment[slot].get_or_init(|| bytes.into())
    }

    /// The size of the blob's file.
    #[must_use]
    pub fn len(&self, b: Blob) -> u64 {
        self.entry(b).bytes.len() as u64
    }

    /// Whether the blob's file is empty.
    #[must_use]
    pub fn is_empty(&self, b: Blob) -> bool {
        self.len(b) == 0
    }

    /// What `fstat` said when the blob's file was read.
    #[must_use]
    pub fn meta(&self, b: Blob) -> FileMeta {
        self.entry(b).meta
    }

    /// `Some` exactly when `r` lies inside one resident part (§11.3). In
    /// the resident store the one part is the whole file.
    #[must_use]
    pub fn resident(&self, b: Blob, r: Range<u64>) -> Option<Resident> {
        let len = self.len(b);
        (r.start <= r.end && r.end <= len).then_some(Resident {
            blob: b,
            start: r.start,
            end: r.end,
        })
    }

    /// The bytes of a resident range.
    #[must_use]
    pub fn bytes(&self, r: Resident) -> &[u8] {
        // In range: checked when the `Resident` was made, and entries never
        // change.
        &self.entry(r.blob).bytes[r.start as usize..r.end as usize]
    }

    /// The whole file: in the resident store, always one part.
    #[must_use]
    pub fn whole(&self, b: Blob) -> &[u8] {
        &self.entry(b).bytes
    }

    /// Copies range `r` of the file into `out`, which must be exactly as
    /// long as `r`. A range past the end of the file is an error before any
    /// copy (§11.3).
    pub fn copy_to(&self, b: Blob, r: Range<u64>, out: &mut [u8]) -> Result<(), ReadError> {
        let resident = self.resident(b, r.clone()).ok_or(ReadError::OutOfRange)?;
        if (r.end - r.start) != out.len() as u64 {
            return Err(ReadError::OutOfRange);
        }
        out.copy_from_slice(self.bytes(resident));
        Ok(())
    }

    /// The content hash of the whole file (§11.6), computed as it was
    /// read.
    #[must_use]
    pub fn hash(&self, b: Blob) -> ContentHash {
        self.entry(b).hash
    }

    /// The blob already holding the file with this device and inode, if
    /// one does (§9.3: found paths whose files are one inode share one
    /// entry).
    #[must_use]
    pub fn blob_of_inode(&self, dev: u64, ino: u64) -> Option<Blob> {
        self.by_inode.lock().unwrap_or_else(PoisonError::into_inner).get(&(dev, ino)).copied()
    }

    /// The system calls made so far.
    #[must_use]
    pub fn stats(&self) -> StoreStats {
        let c = &self.counters;
        StoreStats {
            stats: c.stats.load(Ordering::Relaxed),
            opens: c.opens.load(Ordering::Relaxed),
            fstats: c.fstats.load(Ordering::Relaxed),
            reads: c.reads.load(Ordering::Relaxed),
            closes: c.closes.load(Ordering::Relaxed),
            peak_open: c.peak_open.load(Ordering::Relaxed) as u64,
            files: u64::from(self.blob_count()),
            bytes: c.bytes.load(Ordering::Relaxed),
        }
    }

    /// `stat` of `path`, following symbolic links (§2.5, §4.4).
    pub fn stat(&self, path: &Path) -> Stat {
        self.counters.stats.fetch_add(1, Ordering::Relaxed);
        match std::fs::metadata(path) {
            Ok(m) => {
                let t = m.file_type();
                if t.is_file() {
                    Stat::Regular(FileMeta::of(&m))
                } else if t.is_dir() {
                    Stat::Directory
                } else if t.is_char_device() {
                    Stat::CharDevice(FileMeta::of(&m))
                } else if t.is_fifo() {
                    Stat::Fifo
                } else if t.is_socket() {
                    Stat::Socket
                } else {
                    Stat::BlockDevice
                }
            }
            Err(e)
                if matches!(e.kind(), io::ErrorKind::NotFound | io::ErrorKind::NotADirectory) =>
            {
                Stat::Absent
            }
            Err(e) => Stat::Error { errno: e.raw_os_error().unwrap_or(0), reason: os_reason(&e) },
        }
    }

    /// Reads every job, in parallel: files in parallel, and a file above
    /// [`CHUNK`] in chunks (§11.2, §12.1). The result of job `i` is at
    /// index `i`. A job whose `stat` says it cannot be read is answered
    /// without opening anything (§2.5). Jobs naming one inode should be
    /// grouped by the caller; a file whose inode the store already holds is
    /// not read again.
    pub fn read_many(&self, pool: &Pool, jobs: &[ReadJob]) -> Vec<Result<Blob, ReadError>> {
        let mut plans: Vec<Plan> = jobs.iter().map(|job| self.plan(job)).collect();
        // One item per chunk; an empty file is one item with no bytes.
        let mut buffers: Vec<Vec<u8>> = plans
            .iter()
            .map(|p| match p {
                Plan::Read { size, .. } => vec![0u8; *size as usize],
                Plan::Done(_) => Vec::new(),
            })
            .collect();
        let live: Vec<Live> = plans
            .iter()
            .map(|p| {
                let chunks = match p {
                    Plan::Read { size, .. } => size.div_ceil(CHUNK).max(1) as usize,
                    Plan::Done(_) => 0,
                };
                Live::new(chunks)
            })
            .collect();
        let mut items: Vec<(usize, usize, &mut [u8])> = Vec::new();
        for (i, buffer) in buffers.iter_mut().enumerate() {
            if live[i].chunks == 0 {
                continue;
            }
            if buffer.is_empty() {
                items.push((i, 0, &mut buffer[..]));
            } else {
                for (c, slice) in buffer.chunks_mut(CHUNK as usize).enumerate() {
                    items.push((i, c, slice));
                }
            }
        }
        pool.for_each_part(items, |_, (i, c, slice)| {
            let Plan::Read { meta, char_device, .. } = &plans[i] else {
                unreachable!("only reads have items")
            };
            self.read_chunk(&jobs[i].path, *meta, *char_device, &live[i], c, slice);
        });
        let mut out = Vec::with_capacity(jobs.len());
        for ((plan, buffer), live) in plans.iter_mut().zip(buffers).zip(live) {
            let result = match plan {
                Plan::Done(result) => result.clone(),
                Plan::Read { .. } => {
                    live.finish(buffer).map(|(bytes, meta, hash)| self.add(bytes, meta, hash))
                }
            };
            out.push(result);
        }
        out
    }

    fn plan(&self, job: &ReadJob) -> Plan {
        let (meta, char_device) = match &job.stat {
            Stat::Regular(meta) => (*meta, false),
            Stat::CharDevice(meta) => (*meta, true),
            Stat::Absent => return Plan::Done(Err(ReadError::Missing)),
            Stat::Directory => return Plan::Done(Err(ReadError::Directory)),
            Stat::Fifo => return Plan::Done(Err(ReadError::NotRegular("is a FIFO"))),
            Stat::Socket => return Plan::Done(Err(ReadError::NotRegular("is a socket"))),
            Stat::BlockDevice => {
                return Plan::Done(Err(ReadError::NotRegular("is a block device")));
            }
            Stat::Error { errno, reason } => {
                return Plan::Done(Err(ReadError::Os {
                    errno: Some(*errno),
                    reason: reason.clone(),
                }));
            }
        };
        if !char_device && let Some(b) = self.blob_of_inode(meta.dev, meta.ino) {
            return Plan::Done(Ok(b));
        }
        let size = if char_device { 0 } else { meta.size };
        if usize::try_from(size).is_err() {
            return Plan::Done(Err(ReadError::TooLarge(
                "input file larger than the address space",
            )));
        }
        if size >= MAX_INPUT {
            return Plan::Done(Err(ReadError::TooLarge("input file of 1 TiB or more")));
        }
        Plan::Read { size, meta, char_device }
    }

    /// `open` and `fstat` (§11.2 step 1); the descriptor is counted open
    /// until [`Store::closed`].
    fn open(
        &self,
        path: &Path,
        expected: FileMeta,
        char_device: bool,
    ) -> Result<(File, FileMeta), ReadError> {
        let file = File::open(path).map_err(|e| ReadError::os(&e))?;
        let c = &self.counters;
        c.opens.fetch_add(1, Ordering::Relaxed);
        let now = c.open_now.fetch_add(1, Ordering::Relaxed) + 1;
        c.peak_open.fetch_max(now, Ordering::Relaxed);
        c.fstats.fetch_add(1, Ordering::Relaxed);
        let meta = match file.metadata() {
            Ok(m) => FileMeta::of(&m),
            Err(e) => {
                drop(file);
                self.closed();
                return Err(ReadError::os(&e));
            }
        };
        if let Some(hook) = &self.hooks.after_fstat {
            hook(path);
        }
        // The file `stat` saw is the file opened, at the size `stat` saw:
        // otherwise it changed between the two (§11.5).
        let same = meta.dev == expected.dev && meta.ino == expected.ino;
        if !same || (!char_device && meta.size != expected.size) {
            drop(file);
            self.closed();
            return Err(ReadError::Changed);
        }
        Ok((file, meta))
    }

    fn read_chunk(
        &self,
        path: &Path,
        expected: FileMeta,
        char_device: bool,
        live: &Live,
        c: usize,
        slice: &mut [u8],
    ) {
        let opened = live.opened.get_or_init(|| {
            self.open(path, expected, char_device).map(|(file, meta)| {
                *live.file.lock().unwrap_or_else(PoisonError::into_inner) = Some(Arc::new(file));
                meta
            })
        });
        let file = live.file.lock().unwrap_or_else(PoisonError::into_inner).clone();
        if let (Ok(_), Some(file)) = (opened, &file) {
            if char_device {
                // §2.5: an empty character device reads end of file at once.
                let mut byte = [0u8; 1];
                self.counters.reads.fetch_add(1, Ordering::Relaxed);
                match (&**file).read(&mut byte) {
                    Ok(0) => {}
                    Ok(_) => live.fail(ReadError::NotRegular("not a regular file and not empty")),
                    Err(e) => live.fail(ReadError::os(&e)),
                }
            } else if !slice.is_empty() {
                self.counters.reads.fetch_add(1, Ordering::Relaxed);
                match file.read_exact_at(slice, c as u64 * CHUNK) {
                    Ok(()) => {
                        self.counters.bytes.fetch_add(slice.len() as u64, Ordering::Relaxed);
                        live.hashes[c].store(hash_bytes(slice), Ordering::Relaxed);
                    }
                    Err(e) if e.kind() == io::ErrorKind::UnexpectedEof => {
                        live.fail(ReadError::Changed);
                    }
                    Err(e) => live.fail(ReadError::os(&e)),
                }
            }
        }
        // This worker's handle goes before the count does: the last chunk
        // then holds the only one, and its drop closes the descriptor.
        drop(file);
        if live.remaining.fetch_sub(1, Ordering::AcqRel) != 1 {
            return;
        }
        let Some(file) = live.file.lock().unwrap_or_else(PoisonError::into_inner).take() else {
            return;
        };
        if let Ok(first) = opened
            && !char_device
        {
            // The checks of §11.5: nothing past the size, and the same
            // size and times at the end.
            self.counters.reads.fetch_add(1, Ordering::Relaxed);
            let mut byte = [0u8; 1];
            match file.read_at(&mut byte, first.size) {
                Ok(0) => {}
                Ok(_) => live.fail(ReadError::Changed),
                Err(e) => live.fail(ReadError::os(&e)),
            }
            self.counters.fstats.fetch_add(1, Ordering::Relaxed);
            match file.metadata() {
                Ok(m) => {
                    let last = FileMeta::of(&m);
                    if (last.size, last.mtime, last.ctime) != (first.size, first.mtime, first.ctime)
                    {
                        live.fail(ReadError::Changed);
                    }
                }
                Err(e) => live.fail(ReadError::os(&e)),
            }
        }
        drop(file);
        self.closed();
    }

    fn add(&self, bytes: Box<[u8]>, meta: FileMeta, hash: ContentHash) -> Blob {
        let mut map = self.by_inode.lock().unwrap_or_else(PoisonError::into_inner);
        if let Some(b) = map.get(&(meta.dev, meta.ino)) {
            return *b;
        }
        let b = self.insert(Entry { bytes, meta, hash });
        map.insert((meta.dev, meta.ino), b);
        b
    }

    fn closed(&self) {
        self.counters.closes.fetch_add(1, Ordering::Relaxed);
        self.counters.open_now.fetch_sub(1, Ordering::Relaxed);
    }
}

/// What to do with one job.
enum Plan {
    Done(Result<Blob, ReadError>),
    Read { size: u64, meta: FileMeta, char_device: bool },
}

/// A file being read: opened by the first chunk to arrive, closed by the
/// last to finish.
struct Live {
    chunks: usize,
    opened: OnceLock<Result<FileMeta, ReadError>>,
    file: Mutex<Option<Arc<File>>>,
    remaining: AtomicUsize,
    failed: Mutex<Option<ReadError>>,
    hashes: Vec<AtomicU64>,
}

impl Live {
    fn new(chunks: usize) -> Self {
        Self {
            chunks,
            opened: OnceLock::new(),
            file: Mutex::new(None),
            remaining: AtomicUsize::new(chunks),
            failed: Mutex::new(None),
            hashes: (0..chunks).map(|_| AtomicU64::new(0)).collect(),
        }
    }

    fn fail(&self, error: ReadError) {
        self.failed.lock().unwrap_or_else(PoisonError::into_inner).get_or_insert(error);
    }

    fn finish(self, buffer: Vec<u8>) -> Result<(Box<[u8]>, FileMeta, ContentHash), ReadError> {
        let failed = self.failed.into_inner().unwrap_or_else(PoisonError::into_inner);
        let opened = self.opened.into_inner();
        let meta = match opened {
            Some(Ok(meta)) => meta,
            Some(Err(e)) => return Err(e),
            None => unreachable!("every read job opens its file"),
        };
        if let Some(e) = failed {
            return Err(e);
        }
        let len = buffer.len() as u64;
        let hash = fold_hash(
            len,
            self.hashes.iter().take(chunk_count(len)).map(|h| h.load(Ordering::Relaxed)),
        );
        Ok((buffer.into_boxed_slice(), meta, hash))
    }
}

fn chunk_count(len: u64) -> usize {
    len.div_ceil(CHUNK) as usize
}

/// The content hash (§11.6) of bytes from the hashes of their [`CHUNK`]
/// blocks. Provisional: the function is `spec/incr.md`'s (M7); both stores
/// compute this one.
fn fold_hash(len: u64, blocks: impl Iterator<Item = u64>) -> ContentHash {
    let mut lo = hash_u64(len);
    let mut hi = hash_u64(!len);
    for h in blocks {
        lo = combine(lo, h);
        hi = combine(h, hi);
    }
    ContentHash((u128::from(hi) << 64) | u128::from(lo))
}

/// What the tables read through (`spec/model.md` §1, `InputStore`): a
/// handle this store did not give out, or a range outside its file, is
/// `None` or `OutOfRange`, never a panic.
impl inputs::InputStore for Store {
    fn resident(&self, blob: inputs::Blob, range: Range<u64>) -> Option<&[u8]> {
        let bytes = &self.try_entry(blob.0)?.bytes;
        let lo = usize::try_from(range.start).ok()?;
        let hi = usize::try_from(range.end).ok()?;
        bytes.get(lo..hi)
    }

    fn copy_to(
        &self,
        blob: inputs::Blob,
        range: Range<u64>,
        out: &mut [u8],
    ) -> Result<(), inputs::CopyError> {
        let bytes =
            inputs::InputStore::resident(self, blob, range).ok_or(inputs::CopyError::OutOfRange)?;
        if bytes.len() != out.len() {
            return Err(inputs::CopyError::OutOfRange);
        }
        out.copy_from_slice(bytes);
        Ok(())
    }
}

/// The content hash of `bytes`, as the store computes it while reading.
#[must_use]
pub fn content_hash(bytes: &[u8]) -> ContentHash {
    fold_hash(bytes.len() as u64, bytes.chunks(CHUNK as usize).map(hash_bytes))
}
