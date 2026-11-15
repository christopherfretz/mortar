//! The store (`spec/input.md` §11): it owns every byte the link reads from
//! its inputs, and is shared and immutable to its users.
//!
//! Two stores behind one interface (§11.1), chosen by [`StorePolicy`]:
//!
//! - [`StorePolicy::Staged`], the default: each file's **resident parts**
//!   (headers, symbol and string tables, groups, the relocations of
//!   allocated sections, merge and `.eh_frame` sections, small sections,
//!   archive fronts and member headers: [`crate::staged`]) are read into
//!   owned buffers; everything else stays on disk and is read later,
//!   straight into the caller's buffer, by [`Store::copy_to`] with
//!   positioned reads on a descriptor the store keeps open.
//! - [`StorePolicy::Resident`]: every file is read whole into one owned
//!   buffer, its one part.
//!
//! [`Store::resident`] hands out a [`Resident`] only for a range inside one
//! part, and [`Store::bytes`] cannot fail; [`Store::view`] is the parts
//! view the readers parse (`spec/elf.md` §3.0); [`Store::copy_to`] copies
//! any range of the file, from every worker at once.
//!
//! Entries live in an append-only arena of fixed segments (segment `k`
//! holds `2^k` once-cells, allocated on first use and never moved), so a
//! `&[u8]` handed out stays valid for as long as the store is borrowed.
//!
//! Reading (§11.2): `stat` (following symbolic links, before anything is
//! opened: a FIFO is never opened, §2.5), `open`, `fstat`, positioned
//! reads, a read at the end that must return nothing, a second `fstat`
//! that must agree. The resident store closes a file after its last read;
//! the staged store keeps a file with bytes on disk open until the store
//! is dropped, within a descriptor budget (§11.4).

use std::collections::HashMap;
use std::fs::File;
use std::io::{self, Read};
use std::ops::Range;
use std::os::unix::fs::{FileExt, FileTypeExt, MetadataExt};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU32, AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, OnceLock, PoisonError};

use mortar_core::hash::{combine, hash_bytes, hash_u64};
use mortar_core::inputs::{self, ContentHash, CopyError, Errno};
use mortar_core::par::Pool;
use mortar_diag::os_reason;
use mortar_elf::consts::{EI_CLASS, ELFCLASS32};
use mortar_elf::{Part, View};

use crate::staged::{self, Fetch, MemberSpan, PREFIX, SMALL, Shape};

/// The size of one chunk of a parallel read, and the largest part one
/// worker reads alone (§11.2 step 3). Also the block of the content hash.
pub const CHUNK: u64 = 4 << 20;

/// The smallest file size refused (§11.2): diagnostic keys hold 40 bits of
/// offset.
pub const MAX_INPUT: u64 = 1 << 40;

/// The staged store's first read of an object larger than
/// [`staged::SMALL`]: its first bytes (`spec/landed/input/WP-021b.md`
/// chose the size by measurement).
pub const HEAD: u64 = 4 << 10;

/// And of its last bytes, where compilers put the section header table,
/// the section names and the symbol tables.
pub const TAIL: u64 = 64 << 10;

/// Which store (§11.1).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum StorePolicy {
    /// Every file read whole: one part, the whole file.
    Resident,
    /// Resident parts read into owned buffers, the rest read on demand
    /// (the primary store, `spec/image.md` I-81).
    #[default]
    Staged,
}

impl StorePolicy {
    /// The policy `MORTAR_STORE` names (`resident` or `staged`), else the
    /// default: the switch the tests use to run under both stores.
    #[must_use]
    pub fn from_env() -> Self {
        match std::env::var("MORTAR_STORE").as_deref() {
            Ok("resident") => Self::Resident,
            _ => Self::default(),
        }
    }
}

/// Tunables of the store. [`StoreConfig::default`] is what a link uses.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct StoreConfig {
    /// The most descriptors the staged store keeps open after a file is
    /// read (§11.4); `None`: from `RLIMIT_NOFILE` ([`descriptor_budget`]).
    /// A file read when the budget is spent is made fully resident and
    /// closed; so is a held file when `open` fails with `EMFILE`.
    pub max_open: Option<usize>,
    /// The staged store's first read of a large object or member.
    pub head: u64,
    /// The staged store's read of the end of a large object or member.
    pub tail: u64,
    /// Whether the walk fills `File::hash` (§11.6). `None`: as the policy
    /// does it cheaply (resident: yes, as the bytes are read; staged: no,
    /// the hash needs a second read of every byte).
    pub hashes: Option<bool>,
    /// Whether a check that needs the Mason containers is on (`spec/meta.md`
    /// MT-42, `spec/odr.md` O-42; the switch WP-073 connects to `--odr`).
    /// `false` (the default, and every link today): the staged store makes
    /// exactly the reads it makes with `true` (a small file whole; a large
    /// object's head and tail windows, then its body ranges); container
    /// bytes inside those reads are dropped, never resident, and no other
    /// container byte is read, as a large object's body ranges are clipped
    /// around its containers. `true`: a small object stays resident whole,
    /// container included. Either way a container is never resident in an
    /// object larger than 64 KiB.
    pub containers: bool,
}

impl Default for StoreConfig {
    fn default() -> Self {
        Self { max_open: None, head: HEAD, tail: TAIL, hashes: None, containers: false }
    }
}

/// The descriptors the staged store may keep open: three quarters of the
/// soft `RLIMIT_NOFILE` less 32, at least 16. Safe Rust without
/// dependencies cannot call `getrlimit`; `/proc/self/limits` says the same
/// (`Max open files`). 1024 when it cannot be read.
#[must_use]
pub fn descriptor_budget() -> usize {
    let soft = std::fs::read_to_string("/proc/self/limits")
        .ok()
        .and_then(|text| {
            let line = text.lines().find(|l| l.starts_with("Max open files"))?;
            let value = line["Max open files".len()..].split_whitespace().next()?;
            if value == "unlimited" { Some(1 << 20) } else { value.parse::<usize>().ok() }
        })
        .unwrap_or(1024);
    (soft / 4 * 3).saturating_sub(32).max(16)
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
    /// `st_nlink`: a held file renamed over or removed drops a link even
    /// when `st_ctime` does not move (a filesystem with a coarse clock
    /// stamps the write and the unlink alike: §11.5).
    pub nlink: u64,
}

impl FileMeta {
    fn of(m: &std::fs::Metadata) -> Self {
        Self {
            size: m.len(),
            dev: m.dev(),
            ino: m.ino(),
            mtime: (m.mtime(), m.mtime_nsec()),
            ctime: (m.ctime(), m.ctime_nsec()),
            nlink: m.nlink(),
        }
    }

    fn same_version(&self, other: &Self) -> bool {
        (self.size, self.mtime, self.ctime, self.nlink)
            == (other.size, other.mtime, other.ctime, other.nlink)
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

    /// A failed positioned read: end of file early is a change (§11.5).
    fn read(error: &io::Error) -> Self {
        if error.kind() == io::ErrorKind::UnexpectedEof {
            return Self::Changed;
        }
        Self::os(error)
    }
}

/// A file to read: the path the system calls use and what `stat` said.
#[derive(Clone, Debug)]
pub struct ReadJob {
    pub path: PathBuf,
    pub stat: Stat,
}

/// The system calls the store made, for the measurements of
/// `spec/landed/input/WP-021.md` and `WP-021b.md`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct StoreStats {
    pub stats: u64,
    pub opens: u64,
    pub fstats: u64,
    /// Positioned reads while loading (and the reads at the end).
    pub reads: u64,
    pub closes: u64,
    /// The most descriptors the store held open at once.
    pub peak_open: u64,
    /// Descriptors open now (the staged store's held files).
    pub open_now: u64,
    pub files: u64,
    /// Bytes read while loading.
    pub bytes: u64,
    /// Bytes in resident parts.
    pub resident: u64,
    /// Files made fully resident because descriptors ran out (§11.4).
    pub fully_resident: u64,
    /// Positioned reads and bytes of [`Store::copy_to`] from disk.
    pub copy_reads: u64,
    pub copy_bytes: u64,
    /// Wall nanoseconds of the staged read's waves A, B, C, D and the
    /// entries' assembly, summed over batches (the bench's; decides
    /// nothing).
    pub wave_ns: [u64; 5],
}

/// A test hook, called with the path of the file.
pub type Hook = Box<dyn Fn(&Path) + Send + Sync>;

/// Where a positioned read of the store came from (the byte counter of
/// `spec/input.md` §11.1, T-MT-42).
#[derive(Clone, Copy, Debug)]
pub enum ReadSite<'a> {
    /// While loading the file at this path (every store and wave).
    Load(&'a Path),
    /// [`Store::copy_to`], [`Store::hash_range`] or a full copy, of a blob
    /// already loaded.
    Copy(Blob),
}

/// A test hook called with every range of a file the store reads.
pub type ReadHook = Box<dyn Fn(ReadSite<'_>, Range<u64>) + Send + Sync>;

/// Test hooks (`spec/input.md` §16: `s11_5_short_read`, the hostile cases
/// of the staged store).
#[derive(Default)]
pub struct Hooks {
    /// Called after a file is opened and `fstat`ed, before it is read.
    pub after_fstat: Option<Hook>,
    /// The staged store: called after a file's first reads (its head and
    /// tail, or an archive's headers), before its parts are read.
    pub after_head: Option<Hook>,
    /// Called with the file range of every positioned read that returns
    /// bytes (the test-only byte counter per file range of §11.1, which
    /// T-MT-42 reads). `None` in a link: one test of an `Option` per read.
    pub on_read: Option<ReadHook>,
}

impl std::fmt::Debug for Hooks {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Hooks")
            .field("after_fstat", &self.after_fstat.is_some())
            .field("after_head", &self.after_head.is_some())
            .field("on_read", &self.on_read.is_some())
            .finish()
    }
}

struct Entry {
    len: u64,
    /// Sorted, disjoint, not adjacent (§11.3).
    parts: Box<[Part]>,
    /// One part holds the whole file: nothing is read later.
    whole: bool,
    meta: FileMeta,
    /// The resident store's hash, computed as the file was read; the
    /// staged store's, computed when asked (§11.6).
    hash: OnceLock<Result<ContentHash, ReadError>>,
    /// The staged store's descriptor, while it holds one.
    file: Mutex<Option<Arc<File>>>,
    /// The whole file, when descriptors ran out (§11.4).
    full: OnceLock<Result<Box<[u8]>, ReadError>>,
}

const SEGMENTS: usize = 32;

/// One kept string's slot.
type Kept = OnceLock<Box<[u8]>>;

/// The store. `Sync`, append-only.
pub struct Store {
    policy: StorePolicy,
    config: StoreConfig,
    max_open: usize,
    segments: [OnceLock<Box<[OnceLock<Entry>]>>; SEGMENTS],
    len: AtomicU32,
    by_inode: Mutex<HashMap<(u64, u64), Blob>>,
    /// The blobs whose descriptor the store holds, in the order it took
    /// them (§11.4: the last is made resident first when `open` fails).
    held: Mutex<Vec<Blob>>,
    hooks: Hooks,
    counters: Counters,
    /// Bytes the tables borrow that are no file's: found paths, member
    /// names of thin archives (§1: `Inputs<'s>` borrows from the store).
    kept: [OnceLock<Box<[Kept]>>; SEGMENTS],
    kept_len: AtomicU32,
    /// The descriptors the process's table has room for, as far as the
    /// store made it (see [`Store::make_room`]).
    fd_room: AtomicUsize,
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
    resident: AtomicU64,
    fully_resident: AtomicU64,
    copy_reads: AtomicU64,
    copy_bytes: AtomicU64,
    wave_ns: [AtomicU64; 5],
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

/// `EMFILE` and `ENFILE` (Linux): the process or the system is out of
/// descriptors.
fn out_of_descriptors(e: &io::Error) -> bool {
    matches!(e.raw_os_error(), Some(23 | 24))
}

impl Store {
    /// An empty store.
    #[must_use]
    pub fn new(policy: StorePolicy) -> Self {
        Self::with_config(policy, StoreConfig::default(), Hooks::default())
    }

    /// An empty store with test hooks.
    #[must_use]
    pub fn with_hooks(policy: StorePolicy, hooks: Hooks) -> Self {
        Self::with_config(policy, StoreConfig::default(), hooks)
    }

    /// An empty store with its tunables and test hooks.
    #[must_use]
    pub fn with_config(policy: StorePolicy, config: StoreConfig, hooks: Hooks) -> Self {
        Self {
            policy,
            config,
            max_open: config.max_open.unwrap_or_else(descriptor_budget),
            segments: std::array::from_fn(|_| OnceLock::new()),
            len: AtomicU32::new(0),
            by_inode: Mutex::new(HashMap::new()),
            held: Mutex::new(Vec::new()),
            hooks,
            counters: Counters::default(),
            kept: std::array::from_fn(|_| OnceLock::new()),
            kept_len: AtomicU32::new(0),
            fd_room: AtomicUsize::new(FD_TABLE_START),
        }
    }

    /// The policy.
    #[must_use]
    pub fn policy(&self) -> StorePolicy {
        self.policy
    }

    /// The tunables.
    #[must_use]
    pub fn config(&self) -> StoreConfig {
        self.config
    }

    /// Whether the walk fills `File::hash` ([`StoreConfig::hashes`]).
    #[must_use]
    pub fn hashes(&self) -> bool {
        self.config.hashes.unwrap_or(self.policy == StorePolicy::Resident)
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
        self.entry(b).len
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

    /// The blob's resident parts (§11.3): sorted, disjoint, not adjacent.
    #[must_use]
    pub fn parts(&self, b: Blob) -> &[Part] {
        &self.entry(b).parts
    }

    /// The parts view of the blob's file, for the readers (`spec/elf.md`
    /// §3.0).
    #[must_use]
    pub fn view(&self, b: Blob) -> View<'_> {
        let e = self.entry(b);
        View::parts(&e.parts, e.len)
    }

    /// `Some` exactly when `r` lies inside one resident part (§11.3). In
    /// the resident store the one part is the whole file.
    #[must_use]
    pub fn resident(&self, b: Blob, r: Range<u64>) -> Option<Resident> {
        if r.start > r.end {
            return None;
        }
        self.view(b).get(r.start, r.end - r.start)?;
        Some(Resident { blob: b, start: r.start, end: r.end })
    }

    /// The bytes of a resident range.
    #[must_use]
    pub fn bytes(&self, r: Resident) -> &[u8] {
        // Checked when the `Resident` was made, and entries never change.
        self.view(r.blob)
            .get(r.start, r.end - r.start)
            .unwrap_or_else(|| unreachable!("a Resident lies inside one part"))
    }

    /// The whole file, when one part holds it: always in the resident
    /// store; in the staged store for files read whole (scripts, shared
    /// objects, small objects, anything that is neither an object nor an
    /// archive).
    #[must_use]
    pub fn whole(&self, b: Blob) -> Option<&[u8]> {
        self.view(b).as_slice()
    }

    /// Whether two blobs hold equal bytes (§8.2: two files of one soname).
    pub fn same_bytes(&self, first: Blob, second: Blob) -> Result<bool, ReadError> {
        if first == second {
            return Ok(true);
        }
        let len = self.len(first);
        if len != self.len(second) {
            return Ok(false);
        }
        if let (Some(one), Some(two)) = (self.whole(first), self.whole(second)) {
            return Ok(one == two);
        }
        let block = CHUNK.min(len) as usize;
        let (mut one, mut two) = (vec![0u8; block], vec![0u8; block]);
        let mut at = 0;
        while at < len {
            let n = (len - at).min(CHUNK) as usize;
            self.copy_to(first, at..at + n as u64, &mut one[..n])?;
            self.copy_to(second, at..at + n as u64, &mut two[..n])?;
            if one[..n] != two[..n] {
                return Ok(false);
            }
            at += n as u64;
        }
        Ok(true)
    }

    /// Copies range `r` of the file into `out`, which must be exactly as
    /// long as `r`. A range past the end of the file is an error before any
    /// copy (§11.3). Bytes inside parts come from memory, the others from
    /// one positioned read on the file's descriptor; callable from every
    /// worker at once.
    pub fn copy_to(&self, b: Blob, r: Range<u64>, out: &mut [u8]) -> Result<(), ReadError> {
        let e = self.entry(b);
        if r.start > r.end || r.end > e.len || (r.end - r.start) != out.len() as u64 {
            return Err(ReadError::OutOfRange);
        }
        if let Some(span) = uncovered(&e.parts, r.start, r.end) {
            self.read_uncovered(b, e, span.clone(), &mut out[slice(span, r.start)])?;
        }
        copy_parts(&e.parts, r.start, out);
        Ok(())
    }

    /// The bytes `span` of a file that are not all inside parts: from the
    /// full copy, or one positioned read.
    fn read_uncovered(
        &self,
        b: Blob,
        e: &Entry,
        span: Range<u64>,
        out: &mut [u8],
    ) -> Result<(), ReadError> {
        loop {
            if let Some(full) = e.full.get() {
                let full = full.as_ref().map_err(Clone::clone)?;
                out.copy_from_slice(&full[span.start as usize..span.end as usize]);
                return Ok(());
            }
            let file = e.file.lock().unwrap_or_else(PoisonError::into_inner).clone();
            let Some(file) = file else {
                // Made fully resident between the two looks: `full` is set
                // before the descriptor is taken.
                if e.full.get().is_none() {
                    unreachable!("a staged file has its descriptor or its full copy");
                }
                continue;
            };
            self.counters.copy_reads.fetch_add(1, Ordering::Relaxed);
            self.counters.copy_bytes.fetch_add(out.len() as u64, Ordering::Relaxed);
            prefault(out);
            if let Some(hook) = &self.hooks.on_read {
                hook(ReadSite::Copy(b), span.clone());
            }
            return file.read_exact_at(out, span.start).map_err(|e| ReadError::read(&e));
        }
    }

    /// [`Store::copy_to`] through the model's error.
    fn copy_model(&self, b: Blob, r: Range<u64>, out: &mut [u8]) -> Result<(), CopyError> {
        self.copy_to(b, r, out).map_err(|e| match e {
            ReadError::OutOfRange => CopyError::OutOfRange,
            ReadError::Changed | ReadError::Missing => CopyError::Changed,
            ReadError::Os { errno, .. } => CopyError::Read(Errno(errno.unwrap_or(0))),
            ReadError::Directory | ReadError::NotRegular(_) | ReadError::TooLarge(_) => {
                CopyError::Read(Errno(0))
            }
        })
    }

    /// The content hash of the whole file (§11.6): the resident store's,
    /// computed as it was read; the staged store's, computed on the first
    /// call by streaming the file and checking every resident part against
    /// the stream (a difference is [`ReadError::Changed`]).
    pub fn hash(&self, b: Blob) -> Result<ContentHash, ReadError> {
        let e = self.entry(b);
        e.hash.get_or_init(|| self.hash_range(b, 0..e.len)).clone()
    }

    /// The content hash of the bytes `r` of the blob's file (an archive
    /// member's, §11.6), checked as [`Store::hash`] checks.
    pub fn hash_range(&self, b: Blob, r: Range<u64>) -> Result<ContentHash, ReadError> {
        let e = self.entry(b);
        if r.start > r.end || r.end > e.len {
            return Err(ReadError::OutOfRange);
        }
        if e.whole {
            let bytes = &e.parts.first().map_or(&[][..], |p| &p.bytes[..]);
            return Ok(content_hash(&bytes[r.start as usize..r.end as usize]));
        }
        let len = r.end - r.start;
        let mut block = vec![0u8; CHUNK.min(len) as usize];
        let mut hashes = Vec::with_capacity(len.div_ceil(CHUNK) as usize);
        let mut at = r.start;
        while at < r.end {
            let n = (r.end - at).min(CHUNK);
            let out = &mut block[..n as usize];
            self.read_uncovered(b, e, at..at + n, out)?;
            if !parts_agree(&e.parts, at, out) {
                return Err(ReadError::Changed);
            }
            hashes.push(hash_bytes(out));
            at += n;
        }
        Ok(fold_hash(len, hashes.into_iter()))
    }

    /// The checks of §11.5 again, on a file the staged store still holds:
    /// the same size, `st_mtime`, `st_ctime` and `st_nlink` as when it was
    /// read. For the end of the image phase; a file the store does not hold (read
    /// whole, or made fully resident) needs none.
    pub fn recheck(&self, b: Blob) -> Result<(), ReadError> {
        let e = self.entry(b);
        let file = e.file.lock().unwrap_or_else(PoisonError::into_inner).clone();
        let Some(file) = file else { return Ok(()) };
        self.counters.fstats.fetch_add(1, Ordering::Relaxed);
        let now = FileMeta::of(&file.metadata().map_err(|e| ReadError::os(&e))?);
        if now.same_version(&e.meta) { Ok(()) } else { Err(ReadError::Changed) }
    }

    /// The blob already holding the file with this device and inode, if
    /// one does (§9.3: found paths whose files are one inode share one
    /// entry).
    #[must_use]
    pub fn blob_of_inode(&self, dev: u64, ino: u64) -> Option<Blob> {
        self.by_inode.lock().unwrap_or_else(PoisonError::into_inner).get(&(dev, ino)).copied()
    }

    /// The store's handle of a model handle (`FileBytes::blob`), when this
    /// store gave it out: the direct way back for [`Store::recheck`]
    /// (WP-027c; the image's end, input.md §11.5).
    #[must_use]
    pub fn blob_of_model(&self, b: inputs::Blob) -> Option<Blob> {
        self.try_entry(b.0).map(|_| Blob(b.0))
    }

    /// The `(st_dev, st_ino)` of every file read, in ascending order (the
    /// output file's checks against its inputs: image.md I-62, I-66,
    /// I-113).
    #[must_use]
    pub fn inodes(&self) -> Vec<(u64, u64)> {
        let map = self.by_inode.lock().unwrap_or_else(PoisonError::into_inner);
        let mut v: Vec<(u64, u64)> = map.keys().copied().collect();
        v.sort_unstable();
        v
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
            open_now: c.open_now.load(Ordering::Relaxed) as u64,
            files: u64::from(self.blob_count()),
            bytes: c.bytes.load(Ordering::Relaxed),
            resident: c.resident.load(Ordering::Relaxed),
            fully_resident: c.fully_resident.load(Ordering::Relaxed),
            copy_reads: c.copy_reads.load(Ordering::Relaxed),
            copy_bytes: c.copy_bytes.load(Ordering::Relaxed),
            wave_ns: std::array::from_fn(|i| c.wave_ns[i].load(Ordering::Relaxed)),
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

    /// Reads every job, in parallel (§11.2, §12.1). The result of job `i`
    /// is at index `i`. A job whose `stat` says it cannot be read is
    /// answered without opening anything (§2.5). Jobs naming one inode
    /// should be grouped by the caller; a file whose inode the store
    /// already holds is not read again.
    pub fn read_many(&self, pool: &Pool, jobs: &[ReadJob]) -> Vec<Result<Blob, ReadError>> {
        match self.policy {
            StorePolicy::Resident => self.read_resident(pool, jobs),
            StorePolicy::Staged => self.read_staged(pool, jobs),
        }
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
    /// until [`Store::closed`]. When the system is out of descriptors, the
    /// staged store makes a file it holds fully resident and tries again
    /// (§11.4).
    fn open(
        &self,
        path: &Path,
        expected: FileMeta,
        char_device: bool,
    ) -> Result<(File, FileMeta), ReadError> {
        let file = loop {
            match File::open(path) {
                Ok(file) => break file,
                Err(e) if out_of_descriptors(&e) && self.release_one() => {}
                Err(e) => return Err(ReadError::os(&e)),
            }
        };
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

    fn closed(&self) {
        self.counters.closes.fetch_add(1, Ordering::Relaxed);
        self.counters.open_now.fetch_sub(1, Ordering::Relaxed);
    }

    /// The checks of §11.5 at a file's last read while loading: nothing
    /// past the size, and the same size and times as at the first `fstat`.
    fn final_checks(&self, file: &File, first: &FileMeta) -> Result<(), ReadError> {
        self.counters.reads.fetch_add(1, Ordering::Relaxed);
        let mut byte = [0u8; 1];
        match file.read_at(&mut byte, first.size) {
            Ok(0) => {}
            Ok(_) => return Err(ReadError::Changed),
            Err(e) => return Err(ReadError::os(&e)),
        }
        self.counters.fstats.fetch_add(1, Ordering::Relaxed);
        match file.metadata() {
            Ok(m) if FileMeta::of(&m).same_version(first) => Ok(()),
            Ok(_) => Err(ReadError::Changed),
            Err(e) => Err(ReadError::os(&e)),
        }
    }

    fn add(&self, entry: Entry) -> Blob {
        let mut map = self.by_inode.lock().unwrap_or_else(PoisonError::into_inner);
        let key = (entry.meta.dev, entry.meta.ino);
        if let Some(b) = map.get(&key) {
            let b = *b;
            drop(map);
            self.drop_entry(entry);
            return b;
        }
        let held = entry.file.lock().unwrap_or_else(PoisonError::into_inner).is_some();
        let resident: u64 = entry.parts.iter().map(|p| p.bytes.len() as u64).sum();
        self.counters.resident.fetch_add(resident, Ordering::Relaxed);
        let b = self.insert(entry);
        map.insert(key, b);
        drop(map);
        if held {
            self.held.lock().unwrap_or_else(PoisonError::into_inner).push(b);
        }
        b
    }

    /// An entry that lost the race for its inode: its descriptor closes.
    fn drop_entry(&self, entry: Entry) {
        if entry.file.into_inner().unwrap_or_else(PoisonError::into_inner).is_some() {
            self.closed();
        }
    }

    /// Makes the last file the store holds open fully resident and closes
    /// it (§11.4); `false` when it holds none.
    fn release_one(&self) -> bool {
        let Some(b) = self.held.lock().unwrap_or_else(PoisonError::into_inner).pop() else {
            return false;
        };
        let e = self.entry(b);
        let file = e.file.lock().unwrap_or_else(PoisonError::into_inner).clone();
        if let Some(file) = file {
            self.make_full(e, &file, ReadSite::Copy(b));
            *e.file.lock().unwrap_or_else(PoisonError::into_inner) = None;
            drop(file);
            self.closed();
        }
        true
    }

    /// Reads the whole of a held file into `full`, checked against its
    /// parts (the same bytes, or [`ReadError::Changed`] for every later
    /// copy).
    fn make_full(&self, e: &Entry, file: &File, site: ReadSite<'_>) {
        let read = || -> Result<Box<[u8]>, ReadError> {
            let mut all = vec![0u8; e.len as usize];
            prefault(&mut all);
            self.counters.reads.fetch_add(1, Ordering::Relaxed);
            if let Some(hook) = &self.hooks.on_read {
                hook(site, 0..e.len);
            }
            file.read_exact_at(&mut all, 0).map_err(|e| ReadError::read(&e))?;
            self.counters.bytes.fetch_add(e.len, Ordering::Relaxed);
            if parts_agree(&e.parts, 0, &all) {
                Ok(all.into_boxed_slice())
            } else {
                Err(ReadError::Changed)
            }
        };
        let _ = e.full.set(read());
        self.counters.fully_resident.fetch_add(1, Ordering::Relaxed);
    }

    // -- the resident store ----------------------------------------------

    /// Files in parallel, and a file above [`CHUNK`] in chunks.
    fn read_resident(&self, pool: &Pool, jobs: &[ReadJob]) -> Vec<Result<Blob, ReadError>> {
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
                Plan::Read { .. } => live.finish(buffer).map(|(bytes, meta, hash)| {
                    let len = bytes.len() as u64;
                    let parts: Box<[Part]> =
                        if len == 0 { Box::new([]) } else { Box::new([Part { offset: 0, bytes }]) };
                    self.add(Entry {
                        len,
                        parts,
                        whole: true,
                        meta,
                        hash: OnceLock::from(Ok(hash)),
                        file: Mutex::new(None),
                        full: OnceLock::new(),
                    })
                }),
            };
            out.push(result);
        }
        out
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
                if let Err(e) = self.char_device_empty(file) {
                    live.fail(e);
                }
            } else if !slice.is_empty() {
                prefault(slice);
                self.counters.reads.fetch_add(1, Ordering::Relaxed);
                if let Some(hook) = &self.hooks.on_read {
                    let at = c as u64 * CHUNK;
                    hook(ReadSite::Load(path), at..at + slice.len() as u64);
                }
                match file.read_exact_at(slice, c as u64 * CHUNK) {
                    Ok(()) => {
                        self.counters.bytes.fetch_add(slice.len() as u64, Ordering::Relaxed);
                        live.hashes[c].store(hash_bytes(slice), Ordering::Relaxed);
                    }
                    Err(e) => live.fail(ReadError::read(&e)),
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
            && let Err(e) = self.final_checks(&file, first)
        {
            live.fail(e);
        }
        drop(file);
        self.closed();
    }

    /// §2.5: an empty character device reads end of file at once.
    fn char_device_empty(&self, file: &File) -> Result<(), ReadError> {
        let mut byte = [0u8; 1];
        self.counters.reads.fetch_add(1, Ordering::Relaxed);
        match (&*file).read(&mut byte) {
            Ok(0) => Ok(()),
            Ok(_) => Err(ReadError::NotRegular("not a regular file and not empty")),
            Err(e) => Err(ReadError::os(&e)),
        }
    }

    // -- the staged store ------------------------------------------------

    /// Four parallel waves: (A) per file, `open`, the first reads and the
    /// plan of an object, or the chain of an archive's headers; (B) per
    /// large archive member, its plan; (C) per part, or [`CHUNK`] of a
    /// large part, its bytes (from what the plans read already, the rest
    /// with one positioned read); (D) per file, the checks of §11.5.
    ///
    /// The files of one batch are open from (A) to (D): batches of a
    /// quarter of the descriptor budget keep the files open at once below
    /// the limit with the held ones (§11.4).
    fn read_staged(&self, pool: &Pool, jobs: &[ReadJob]) -> Vec<Result<Blob, ReadError>> {
        let batch = (self.max_open / 4).clamp(16, 1024);
        jobs.chunks(batch).flat_map(|jobs| self.read_batch(pool, jobs)).collect()
    }

    #[allow(clippy::too_many_lines, reason = "the four waves of §11.9 in order")]
    fn read_batch(&self, pool: &Pool, jobs: &[ReadJob]) -> Vec<Result<Blob, ReadError>> {
        let mut clock = std::time::Instant::now();
        let mut wave = |w: usize| {
            let now = std::time::Instant::now();
            let ns = u64::try_from((now - clock).as_nanos()).unwrap_or(u64::MAX);
            self.counters.wave_ns[w].fetch_add(ns, Ordering::Relaxed);
            clock = now;
        };
        let plans: Vec<Plan> = jobs.iter().map(|job| self.plan(job)).collect();
        let reads: Vec<usize> =
            (0..plans.len()).filter(|&i| matches!(plans[i], Plan::Read { .. })).collect();
        self.make_room(reads.len());

        // (A)
        let mut staging: Vec<Result<Staging, ReadError>> = pool.map(reads.len(), |k| {
            let i = reads[k];
            let Plan::Read { size, meta, char_device } = plans[i] else {
                unreachable!("only reads are staged")
            };
            self.stage(&jobs[i].path, size, meta, char_device)
        });

        wave(0);
        // (B)
        let members: Vec<(usize, MemberSpan)> = staging
            .iter()
            .enumerate()
            .filter_map(|(k, s)| s.as_ref().ok().map(|s| (k, s)))
            .flat_map(|(k, s)| s.members.iter().map(move |m| (k, *m)))
            .collect();
        let planned = pool.map(members.len(), |m| {
            let (k, span) = members[m];
            let Ok(s) = &staging[k] else { unreachable!("members of staged files") };
            self.plan_member(s, span)
        });
        for ((k, _), result) in members.iter().zip(planned) {
            let Ok(s) = &mut staging[*k] else { continue };
            match result {
                Ok((ranges, scratch, containers)) => {
                    s.ranges.extend(ranges);
                    s.scratch.extend(scratch);
                    s.forbidden.extend(containers);
                }
                Err(e) => {
                    // Its descriptor closes with it.
                    staging[*k] = Err(e);
                    self.closed();
                }
            }
        }

        wave(1);
        // (C)
        let mut filled = vec![false; staging.len()];
        let mut parts: Vec<Vec<(u64, Vec<u8>)>> = staging
            .iter_mut()
            .enumerate()
            .map(|(k, s)| match s {
                Ok(s) => {
                    s.scratch.sort_by_key(|(at, _)| *at);
                    s.longest = s.scratch.iter().map(|(_, b)| b.len() as u64).max().unwrap_or(0);
                    s.forbidden.sort_by_key(|r| (r.start, r.end));
                    let merged = staged::merge(std::mem::take(&mut s.ranges), &s.forbidden);
                    // A file read whole in wave (A) (a small one with no
                    // container, `stage_small_without_containers`): that
                    // read is its one part, with no copy.
                    if let ([r], [(0, b)]) = (&merged[..], &s.scratch[..])
                        && r.start == 0
                        && r.end == s.len
                        && b.len() as u64 == s.len
                    {
                        let (_, bytes) = s.scratch.pop().unwrap_or_else(|| unreachable!());
                        s.longest = 0;
                        filled[k] = true;
                        return vec![(0, bytes)];
                    }
                    merged
                        .into_iter()
                        .map(|r| (r.start, vec![0u8; (r.end - r.start) as usize]))
                        .collect()
                }
                Err(_) => Vec::new(),
            })
            .collect();
        let failed: Vec<OnceLock<ReadError>> = staging.iter().map(|_| OnceLock::new()).collect();
        let mut items: Vec<(usize, u64, &mut [u8])> = Vec::new();
        for (k, file_parts) in parts.iter_mut().enumerate() {
            if filled[k] {
                continue;
            }
            for (offset, bytes) in file_parts.iter_mut() {
                for (c, slice) in bytes.chunks_mut(CHUNK as usize).enumerate() {
                    items.push((k, *offset + c as u64 * CHUNK, slice));
                }
            }
        }
        pool.for_each_part(items, |_, (k, at, out)| {
            let Ok(s) = &staging[k] else { return };
            if let Err(e) = self.fill(s, at, out) {
                let _ = failed[k].set(e);
            }
        });

        wave(2);
        // (D)
        let checked = pool.unit_cost(3000).map(staging.len(), |k| match &staging[k] {
            Ok(s) => {
                if let Some(e) = failed[k].get() {
                    return Err(e.clone());
                }
                if s.char_device { Ok(()) } else { self.final_checks(&s.file, &s.meta) }
            }
            Err(e) => Err(e.clone()),
        });

        wave(3);
        let mut results: Vec<Option<Result<Blob, ReadError>>> = plans
            .iter()
            .map(|p| match p {
                Plan::Done(r) => Some(r.clone()),
                Plan::Read { .. } => None,
            })
            .collect();
        for (((k, s), file_parts), check) in staging.into_iter().enumerate().zip(parts).zip(checked)
        {
            let result = match (s, check) {
                (Ok(s), Ok(())) => Ok(self.finish_staged(s, file_parts)),
                (Ok(s), Err(e)) => {
                    drop(s.file);
                    self.closed();
                    Err(e)
                }
                (Err(e), _) => Err(e),
            };
            results[reads[k]] = Some(result);
        }
        wave(4);
        results
            .into_iter()
            .map(|r| r.unwrap_or_else(|| unreachable!("every job answered")))
            .collect()
    }

    /// Grows the process's descriptor table, before a batch's files are
    /// opened, to room for `n` more descriptors than the store has open.
    ///
    /// The batch keeps its files open from (A) to (D), so a batch of
    /// hundreds of files grows the kernel's table (64 slots, then doubling)
    /// while the workers open them; and the kernel waits for an RCU grace
    /// period at every growth of the table of a process with more than one
    /// thread (`expand_fdtable`), with every other `open` of the process
    /// waiting behind it. Measured on `synthetic-objects` (1 006 files, one
    /// batch): 25–40 ms of wave (A) at every thread count. Called between
    /// regions, when the process has one thread, the growth needs no grace
    /// period: `n` duplicates of one descriptor, closed at once (~1 µs
    /// each; the table never shrinks). Purely a cost: a duplicate that
    /// fails ends the growth, and nothing read depends on it.
    fn make_room(&self, n: usize) {
        let open = self.counters.open_now.load(Ordering::Relaxed);
        let want = open + n + FD_SLACK;
        if want <= self.fd_room.load(Ordering::Relaxed) {
            return;
        }
        let Ok(first) = File::open("/dev/null") else { return };
        let mut dups = Vec::with_capacity(n + FD_SLACK);
        while dups.len() < n + FD_SLACK {
            match first.try_clone() {
                Ok(f) => dups.push(f),
                Err(_) => break,
            }
        }
        if dups.len() == n + FD_SLACK {
            self.fd_room.fetch_max(want, Ordering::Relaxed);
        }
    }

    /// Wave (A) for one file.
    fn stage<'p>(
        &self,
        path: &'p Path,
        size: u64,
        meta: FileMeta,
        char_device: bool,
    ) -> Result<Staging<'p>, ReadError> {
        let (file, first) = self.open(path, meta, char_device)?;
        let file = Arc::new(file);
        let mut s = Staging {
            path,
            file,
            meta: first,
            len: size,
            char_device,
            ranges: Vec::new(),
            scratch: Vec::new(),
            longest: 0,
            members: Vec::new(),
            forbidden: Vec::new(),
        };
        let result = self.stage_file(&mut s);
        if let Err(e) = result {
            drop(s);
            self.closed();
            return Err(e);
        }
        if let Some(hook) = &self.hooks.after_head {
            hook(path);
        }
        Ok(s)
    }

    fn stage_file(&self, s: &mut Staging<'_>) -> Result<(), ReadError> {
        if s.char_device {
            return self.char_device_empty(&s.file);
        }
        let len = s.len;
        if len == 0 {
            return Ok(());
        }
        if len <= SMALL {
            if self.config.containers {
                s.ranges.push(0..len);
                return Ok(());
            }
            return self.stage_small_without_containers(s);
        }
        let config = self.config;
        let head = config.head.clamp(PREFIX, len);
        let mut src =
            Reader { store: self, path: s.path, file: &s.file, shared: &[], own: Vec::new(), len };
        src.read_into_own(0, head)?;
        let shape = staged::shape(src.fetch(0, PREFIX)?);
        match shape {
            Shape::Object => {
                let tail_start = len.saturating_sub(config.tail).max(head);
                if tail_start < len {
                    src.read_into_own(tail_start, len - tail_start)?;
                }
                let mut ranges = Vec::new();
                if staged::object_ranges(&mut src, 0, len, &mut ranges, &mut s.forbidden)? {
                    s.ranges = ranges;
                } else {
                    s.ranges.push(0..len);
                }
            }
            Shape::Archive => {
                s.scratch = src.own;
                return self.stage_archive(s);
            }
            Shape::Other => s.ranges.push(0..len),
        }
        s.scratch = src.own;
        Ok(())
    }

    /// Wave (A) of a file of at most [`SMALL`] bytes with no check that
    /// needs a container (`spec/meta.md` MT-42, amendment A-8 to §11.1):
    /// the read main makes, the whole file in one read, made here rather
    /// than in wave (C) so that a relocatable object's containers are
    /// found in its bytes and left out of its ranges. Wave (C) takes this
    /// read as the file's one part when it has no container, and otherwise
    /// fills the parts from it; the container bytes in it are dropped with
    /// the scratch, never resident, never parsed.
    fn stage_small_without_containers(&self, s: &mut Staging<'_>) -> Result<(), ReadError> {
        let len = s.len;
        let mut src =
            Reader { store: self, path: s.path, file: &s.file, shared: &[], own: Vec::new(), len };
        src.read_into_own(0, len)?;
        let head = src.fetch(0, PREFIX.min(len))?;
        let shape = staged::shape(head);
        // The file header stays resident whatever a section claims (a
        // container overlapping it is malformed, and the reader says so):
        // 52 bytes of an ELF32 object, whose first container GNU as puts
        // at 52; 64 of an ELF64 one.
        let header = if head.get(EI_CLASS) == Some(&ELFCLASS32) { 52 } else { PREFIX };
        // Every fetch below lies inside the file, so inside that one read.
        let found: Vec<Range<u64>> = if shape == Shape::Object {
            let all = staged::container_ranges(&mut src, 0, len)?.unwrap_or_default();
            all.into_iter()
                .filter_map(|r| (r.end > header).then(|| r.start.max(header)..r.end))
                .collect()
        } else {
            Vec::new()
        };
        s.ranges.extend(staged::clip(0..len, &found));
        s.forbidden = found;
        s.scratch = src.own;
        Ok(())
    }

    /// Wave (A) of a regular archive larger than 64 KiB: the chain of its
    /// headers from the first `head` bytes (in `s.scratch`). Its members'
    /// bytes may be read whatever they hold (a Mason container included:
    /// the windows and the member's own reads are made before its section
    /// headers are known; WP-071's record); a container of a member larger
    /// than 64 KiB is never resident.
    fn stage_archive(&self, s: &mut Staging<'_>) -> Result<(), ReadError> {
        let len = s.len;
        let own = std::mem::take(&mut s.scratch);
        let src = Reader { store: self, path: s.path, file: &s.file, shared: &[], own, len };
        let base = self.config.head + PREFIX;
        let mut chain = Chain { reader: src, base, window: base, last_end: 0 };
        let mut spans = Vec::new();
        staged::archive_ranges(&mut chain, len, &mut s.ranges, &mut spans)?;
        let mut src = chain.reader;
        for span in spans {
            if span.size <= SMALL {
                s.ranges.push(span.content..span.content + span.size);
                continue;
            }
            // The member's head is in the window read with its header.
            let prefix = PREFIX.min(span.size);
            let shape = staged::shape(src.fetch(span.content, prefix)?);
            if shape == Shape::Object {
                s.members.push(span);
            } else {
                s.ranges.push(span.content..span.content + prefix);
            }
        }
        s.scratch = src.own;
        Ok(())
    }

    /// Wave (B) for one large member of an archive: its plan, with its
    /// tail read; the member's head is in the archive's scratch.
    fn plan_member(&self, s: &Staging<'_>, span: MemberSpan) -> Result<PlannedMember, ReadError> {
        let mut src = Reader {
            store: self,
            path: s.path,
            file: &s.file,
            shared: &s.scratch,
            own: Vec::new(),
            len: s.len,
        };
        let end = span.content + span.size;
        let head_end = span.content + self.config.head.min(span.size);
        let tail_start = end.saturating_sub(self.config.tail).max(head_end);
        if tail_start < end {
            src.read_into_own(tail_start, end - tail_start)?;
        }
        let mut ranges = Vec::new();
        let mut containers = Vec::new();
        if !staged::object_ranges(&mut src, span.content, span.size, &mut ranges, &mut containers)?
        {
            ranges.clear();
            ranges.push(span.content..end);
        }
        Ok((ranges, src.own, containers))
    }

    /// Wave (C): the bytes `at..at + out.len()` of a part, from what was
    /// read already, the rest with one positioned read.
    fn fill(&self, s: &Staging, at: u64, out: &mut [u8]) -> Result<(), ReadError> {
        let end = at + out.len() as u64;
        // What the scratch buffers cover, merged.
        let mut covered: Vec<(u64, u64)> = Vec::new();
        // Sorted by start; none is longer than `longest`, so none before
        // `first` reaches `at`.
        let first = s.scratch.partition_point(|(o, _)| o + s.longest <= at);
        for (o, b) in s.scratch.iter().skip(first) {
            if *o >= end {
                // Sorted by start; later ones start later still.
                break;
            }
            let lo = (*o).max(at);
            let hi = (o + b.len() as u64).min(end);
            if lo < hi {
                covered.push((lo, hi));
            }
        }
        covered.sort_unstable();
        let mut gaps: Vec<(u64, u64)> = Vec::new();
        let mut pos = at;
        for &(lo, hi) in &covered {
            if lo > pos {
                gaps.push((pos, lo));
            }
            pos = pos.max(hi);
        }
        if pos < end {
            gaps.push((pos, end));
        }
        // One read per run of gaps less than [`REREAD`] apart; what a read
        // takes of covered bytes is replaced below by the bytes the plan
        // read (one version of every byte).
        let mut runs: Vec<(u64, u64)> = Vec::new();
        for (lo, hi) in gaps {
            match runs.last_mut() {
                Some(last) if lo < last.1 + REREAD => last.1 = hi,
                _ => runs.push((lo, hi)),
            }
        }
        for (lo, hi) in runs {
            let dst = &mut out[(lo - at) as usize..(hi - at) as usize];
            prefault(dst);
            self.counters.reads.fetch_add(1, Ordering::Relaxed);
            if let Some(hook) = &self.hooks.on_read {
                hook(ReadSite::Load(s.path), lo..hi);
            }
            s.file.read_exact_at(dst, lo).map_err(|e| ReadError::read(&e))?;
            self.counters.bytes.fetch_add(hi - lo, Ordering::Relaxed);
        }
        for (o, b) in s.scratch.iter().skip(first) {
            if *o >= end {
                break;
            }
            let lo = (*o).max(at);
            let hi = (o + b.len() as u64).min(end);
            if lo < hi {
                out[(lo - at) as usize..(hi - at) as usize]
                    .copy_from_slice(&b[(lo - o) as usize..(hi - o) as usize]);
            }
        }
        Ok(())
    }

    /// The entry of a file whose waves all succeeded.
    fn finish_staged(&self, s: Staging<'_>, parts: Vec<(u64, Vec<u8>)>) -> Blob {
        let parts: Box<[Part]> = parts
            .into_iter()
            .map(|(offset, bytes)| Part { offset, bytes: bytes.into_boxed_slice() })
            .collect();
        let whole = s.len == 0
            || (parts.len() == 1 && parts[0].offset == 0 && parts[0].bytes.len() as u64 == s.len);
        let entry = Entry {
            len: s.len,
            parts,
            whole,
            meta: s.meta,
            hash: OnceLock::new(),
            file: Mutex::new(None),
            full: OnceLock::new(),
        };
        if whole {
            drop(s.file);
            self.closed();
        } else {
            let budget = self.held.lock().unwrap_or_else(PoisonError::into_inner).len();
            if budget >= self.max_open {
                self.make_full(&entry, &s.file, ReadSite::Load(s.path));
                drop(s.file);
                self.closed();
            } else {
                *entry.file.lock().unwrap_or_else(PoisonError::into_inner) = Some(s.file);
            }
        }
        self.add(entry)
    }
}

/// Touches every page of a buffer a positioned read is about to fill.
///
/// A fresh buffer's pages are not mapped until first written. Written by
/// `read`, each page faults inside the kernel's copy, and on this kernel
/// (Linux 6.8, AArch64) a fault taken in kernel mode does not use the
/// per-VMA lock that a user-mode fault uses: it takes the process's
/// `mmap_lock`, which every worker's faults (and the allocator's heap
/// growth) then share. Measured on `synthetic-objects`' 1 006 head-and-tail
/// reads: 30 ms at 1 thread, 40–60 ms at 8 and 12 when the read faults;
/// 5–6 ms at 8 when the pages were touched first (`spec/landed/perf/
/// WP-032h.md`). The writes are of the value the read then overwrites, and
/// cost nothing where the pages are mapped already.
fn prefault(buf: &mut [u8]) {
    const PAGE: usize = 4096;
    let len = buf.len();
    if len == 0 {
        return;
    }
    let mut i = 0;
    while i < len {
        // An opaque zero: a store of a known zero into zeroed memory may be
        // removed by the optimiser, and the touch with it.
        buf[i] = std::hint::black_box(0);
        i += PAGE;
    }
    buf[len - 1] = std::hint::black_box(0);
}

/// The descriptors a process's table holds before it first grows (the
/// kernel's `NR_OPEN_DEFAULT` on a 64-bit target), less the standard
/// streams and a margin: a batch that fits needs no [`Store::make_room`].
const FD_TABLE_START: usize = 48;

/// What [`Store::make_room`] adds to a batch: the descriptors open that the
/// store does not count (standard streams, the output, the walk's).
const FD_SLACK: usize = 16;

/// The largest window of [`Chain`]: what a miss may read of a large
/// member's bytes that are not resident.
const WINDOW: u64 = 1 << 20;

/// Wave (C) reads the bytes between two gaps of a part again rather than
/// make a second read, when they are fewer than this.
const REREAD: u64 = 16 << 10;

/// A member's planned ranges, the reads its plan made, and its containers.
type PlannedMember = (Vec<Range<u64>>, Vec<(u64, Vec<u8>)>, Vec<Range<u64>>);

/// A file between the waves of the staged read.
struct Staging<'p> {
    path: &'p Path,
    file: Arc<File>,
    meta: FileMeta,
    len: u64,
    char_device: bool,
    /// Candidate resident ranges (§11.3), merged into parts in wave (C).
    ranges: Vec<Range<u64>>,
    /// Bytes read so far, by offset.
    scratch: Vec<(u64, Vec<u8>)>,
    /// The longest of them (wave (C) searches them by start).
    longest: u64,
    /// Large archive members whose plan is wave (B)'s.
    members: Vec<MemberSpan>,
    /// The Mason containers found so far (`spec/meta.md` MT-6): never in a
    /// part, and no merge joins across one (A-8).
    forbidden: Vec<Range<u64>>,
}

/// [`Fetch`] over a file: what was read already (the archive's reads for
/// a member, then its own), else one positioned read kept for later.
struct Reader<'a> {
    store: &'a Store,
    /// For the read hook.
    path: &'a Path,
    file: &'a File,
    shared: &'a [(u64, Vec<u8>)],
    own: Vec<(u64, Vec<u8>)>,
    /// The file's size at `fstat` (a read that finds it shorter fails as
    /// a change).
    len: u64,
}

impl Reader<'_> {
    fn read_into_own(&mut self, at: u64, size: u64) -> Result<(), ReadError> {
        let mut buf = vec![0u8; size as usize];
        prefault(&mut buf);
        self.store.counters.reads.fetch_add(1, Ordering::Relaxed);
        if let Some(hook) = &self.store.hooks.on_read {
            hook(ReadSite::Load(self.path), at..at + size);
        }
        self.file.read_exact_at(&mut buf, at).map_err(|e| ReadError::read(&e))?;
        self.store.counters.bytes.fetch_add(size, Ordering::Relaxed);
        self.own.push((at, buf));
        Ok(())
    }

    fn find(&self, at: u64, size: u64) -> Option<&[u8]> {
        let inside = |(o, b): &(u64, Vec<u8>)| at >= *o && at + size <= o + b.len() as u64;
        let found = self.own.iter().rev().find(|x| inside(x)).or_else(|| {
            let i = self.shared.partition_point(|(o, _)| *o <= at);
            self.shared[..i].iter().rev().find(|x| inside(x))
        })?;
        let lo = (at - found.0) as usize;
        Some(&found.1[lo..lo + size as usize])
    }
}

impl Fetch for Reader<'_> {
    fn fetch(&mut self, at: u64, size: u64) -> Result<&[u8], ReadError> {
        if self.find(at, size).is_none() {
            self.read_into_own(at, size)?;
        }
        Ok(self.find(at, size).unwrap_or_else(|| unreachable!("just read")))
    }
}

/// [`Fetch`] for an archive's chain of headers: a miss reads a window of
/// a header and the start of its member, so that a member's head comes
/// with the read of its header. A miss soon after the last read's end (a
/// run of small members, all of them resident) doubles the window, up to
/// [`WINDOW`]; any other resets it: the reads of a run of small members are
/// not wasted, as wave (C) takes their bytes from them.
struct Chain<'a> {
    reader: Reader<'a>,
    base: u64,
    window: u64,
    last_end: u64,
}

impl Fetch for Chain<'_> {
    fn fetch(&mut self, at: u64, size: u64) -> Result<&[u8], ReadError> {
        if self.reader.find(at, size).is_none() {
            let len = self.reader.len;
            self.window = if self.last_end != 0 && at < self.last_end + 2 * SMALL {
                (self.window * 2).min(WINDOW)
            } else {
                self.base
            };
            let n = size.max(self.window).min(len.saturating_sub(at)).max(size);
            self.reader.read_into_own(at, n)?;
            self.last_end = at + n;
        }
        Ok(self.reader.find(at, size).unwrap_or_else(|| unreachable!("just read")))
    }
}

/// What to do with one job.
enum Plan {
    Done(Result<Blob, ReadError>),
    Read { size: u64, meta: FileMeta, char_device: bool },
}

/// A file being read by the resident store: opened by the first chunk to
/// arrive, closed by the last to finish.
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

/// The part (by index) holding `at`, or the first after it.
fn first_part_ending_after(parts: &[Part], at: u64) -> usize {
    parts.partition_point(|p| p.end() <= at)
}

/// The smallest range covering every byte of `a..b` that is not inside a
/// part, or `None` when parts cover all of it.
fn uncovered(parts: &[Part], a: u64, b: u64) -> Option<Range<u64>> {
    if a >= b {
        return None;
    }
    let mut lo = a;
    let mut i = first_part_ending_after(parts, a);
    while i < parts.len() && parts[i].offset <= lo && lo < b {
        lo = parts[i].end();
        i += 1;
    }
    if lo >= b {
        return None;
    }
    let mut hi = b;
    let mut j = parts.partition_point(|p| p.offset < b);
    while j > 0 && parts[j - 1].end() >= hi && parts[j - 1].offset < hi && hi > lo {
        hi = parts[j - 1].offset;
        j -= 1;
    }
    Some(lo..hi.max(lo))
}

/// The slice of a buffer that starts at file offset `base` holding `r`.
fn slice(r: Range<u64>, base: u64) -> Range<usize> {
    (r.start - base) as usize..(r.end - base) as usize
}

/// Copies the bytes of every part overlapping `at..at + out.len()` into
/// `out`.
fn copy_parts(parts: &[Part], at: u64, out: &mut [u8]) {
    let end = at + out.len() as u64;
    for p in &parts[first_part_ending_after(parts, at)..] {
        if p.offset >= end {
            break;
        }
        let lo = p.offset.max(at);
        let hi = p.end().min(end);
        out[slice(lo..hi, at)].copy_from_slice(&p.bytes[slice(lo..hi, p.offset)]);
    }
}

/// Whether the parts agree with `bytes`, the file's bytes from `at`.
fn parts_agree(parts: &[Part], at: u64, bytes: &[u8]) -> bool {
    let end = at + bytes.len() as u64;
    parts[first_part_ending_after(parts, at)..].iter().take_while(|p| p.offset < end).all(|p| {
        let lo = p.offset.max(at);
        let hi = p.end().min(end);
        bytes[slice(lo..hi, at)] == p.bytes[slice(lo..hi, p.offset)]
    })
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
        let e = self.try_entry(blob.0)?;
        if range.start > range.end {
            return None;
        }
        View::parts(&e.parts, e.len).get(range.start, range.end - range.start)
    }

    fn copy_to(
        &self,
        blob: inputs::Blob,
        range: Range<u64>,
        out: &mut [u8],
    ) -> Result<(), CopyError> {
        if self.try_entry(blob.0).is_none() {
            return Err(CopyError::OutOfRange);
        }
        self.copy_model(Blob(blob.0), range, out)
    }
}

/// The content hash of `bytes`, as the store computes it.
#[must_use]
pub fn content_hash(bytes: &[u8]) -> ContentHash {
    fold_hash(bytes.len() as u64, bytes.chunks(CHUNK as usize).map(hash_bytes))
}
