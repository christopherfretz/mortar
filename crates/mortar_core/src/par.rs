//! The deterministic parallel helpers of `spec/model.md` §6, on scoped
//! threads.
//!
//! Every helper has a sequential meaning, and its result is that meaning
//! for every thread count and every schedule:
//!
//! - rule 1, disjoint slices: [`split_ranges`], [`split_offsets`],
//!   [`Pool::for_each_part`], [`Pool::for_each_mut`];
//! - rule 3, ordered collection: [`Pool::map`], [`Pool::map_collect`],
//!   [`Pool::map_collect_vecs`], [`Pool::prefix_sum`];
//! - rule 4, sorting: [`Pool::sort_by`], [`Pool::sort_by_key`] (stable:
//!   the result is that of `slice::sort_by`);
//! - rule 6, fixed-size blocks: [`blocks`], [`Pool::for_each_block`];
//! - rule 2 (atomic minimum, maximum, or) needs no helper: `std`'s atomics
//!   are the helper.
//!
//! # Scheduling
//!
//! A call with work for more than one worker runs on at most
//! `min(threads, items)` workers, the calling thread being the first, and
//! returns when every one of them has. Items (units, or blocks of units)
//! are handed out through one atomic cursor: a worker takes the next item
//! when it is done with one. The caller spawns one scoped thread, and each
//! spawned thread spawns up to two more before it takes items, as long as
//! items are left untaken (a tree: no thread pays for every spawn, and a
//! region that is over before a spawned thread starts spawns no more).
//! [`Pool::unit_cost`] lets a call site cap the workers by its estimated
//! work. With one thread the work runs on the caller, in order, and
//! nothing is spawned or allocated beyond what a helper's result needs.
//!
//! With a **scheduling seed** ([`SEED_VAR`], or [`Pool::with_seed`]) items
//! are single units, handed out in an order permuted by the seed, every
//! worker is spawned, and every worker yields after each unit, so that a
//! test can run a phase under many schedules ([`check_schedules`]). The
//! seed changes the schedule only, never a result.
//!
//! # Panics
//!
//! A panic in a worker stops the hand-out of further items, the other
//! workers finish the item they hold, every thread is joined, and the
//! (first) panic resumes on the caller with its original payload. In a
//! release build a panic aborts (it is a bug of mortar's); tests unwind.
//!
//! # Cost
//!
//! A call that runs in parallel costs one spawn per extra worker (tens of
//! microseconds each, measured in `spec/landed/model/WP-020b.md` and
//! `spec/landed/perf/WP-032e.md`; the caller pays for one), and per helper
//! what its documentation states. Safe Rust has no way to hand borrowed
//! work to threads that outlive the call, so there is no persistent pool:
//! a phase should make few calls with much work each, not many small ones.
//! `MORTAR_STEPS` prints one line per region (`par <file>:<line>`) with its
//! units, workers and spawns.

use std::cmp::Ordering;
use std::fmt;
use std::ops::{Add, Range};
use std::sync::atomic::{self, AtomicBool, AtomicUsize};
use std::sync::{Mutex, OnceLock, PoisonError};

/// The environment variable holding the scheduling seed: a decimal `u64`.
/// Read once per process.
pub const SEED_VAR: &str = "MORTAR_SCHED_SEED";

/// Blocks per worker when units are batched: enough that uneven units
/// balance, few enough that the cursor is not contended. Not observable.
const BLOCKS_PER_THREAD: usize = 16;

/// The least estimated work, in nanoseconds, that one worker of a region
/// sized by [`Pool::unit_cost`] gets: about four times what spawning and
/// joining one thread costs on its own (`spec/landed/perf/WP-032e.md`:
/// 35–60 µs per thread at 8–12 threads).
pub const WORKER_NS: u128 = 200_000;

/// The block of [`Pool::prefix_sum`] (rule 6: a constant).
pub const SCAN_BLOCK: usize = 1 << 16;

/// The smallest run [`Pool::sort_by`] sorts in one piece, and the length up
/// to which it sorts sequentially.
pub const SORT_RUN: usize = 1 << 13;

/// The most runs [`Pool::sort_by`] cuts a slice into before merging.
pub const SORT_MAX_RUNS: usize = 64;

/// The output block of one merge task in [`Pool::sort_by`].
pub const MERGE_BLOCK: usize = 1 << 14;

/// A value of [`SEED_VAR`] that is not a decimal `u64`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BadSeed(pub String);

impl fmt::Display for BadSeed {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{SEED_VAR} must be a decimal integer below 2^64, not `{}`", self.0)
    }
}

impl std::error::Error for BadSeed {}

/// Parses a scheduling seed as [`SEED_VAR`] holds it.
pub fn parse_seed(text: &str) -> Result<u64, BadSeed> {
    text.trim().parse().map_err(|_| BadSeed(text.to_owned()))
}

/// The seed of [`SEED_VAR`], read once.
pub fn env_seed() -> Result<Option<u64>, BadSeed> {
    static SEED: OnceLock<Result<Option<u64>, BadSeed>> = OnceLock::new();
    SEED.get_or_init(|| match std::env::var_os(SEED_VAR) {
        None => Ok(None),
        Some(value) => match value.to_str() {
            Some(text) => parse_seed(text).map(Some),
            None => Err(BadSeed(value.to_string_lossy().into_owned())),
        },
    })
    .clone()
}

/// The thread count and the scheduling seed of the helpers. Cheap to copy;
/// holds no threads.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Pool {
    threads: usize,
    seed: Option<u64>,
    /// The caller's estimate of one unit's cost in nanoseconds
    /// ([`Pool::unit_cost`]); 0: unknown.
    unit_ns: u32,
}

impl Default for Pool {
    /// [`Pool::available`] threads, with the environment's seed.
    fn default() -> Self {
        Self::new(Self::available())
    }
}

impl Pool {
    /// `threads` threads (`--threads`) and the seed of [`SEED_VAR`].
    ///
    /// # Panics
    ///
    /// If `threads` is 0, or the environment holds a malformed seed (the
    /// driver reports that through [`Pool::try_new`] first).
    pub fn new(threads: usize) -> Self {
        Self::try_new(threads).unwrap_or_else(|e| panic!("{e}"))
    }

    /// `threads` threads and the seed of [`SEED_VAR`], or why the seed is
    /// malformed.
    ///
    /// # Panics
    ///
    /// If `threads` is 0.
    pub fn try_new(threads: usize) -> Result<Self, BadSeed> {
        assert!(threads >= 1, "a pool has at least one thread");
        Ok(Self { threads, seed: env_seed()?, unit_ns: 0 })
    }

    /// One thread, no seed: everything runs on the caller, in order.
    pub fn sequential() -> Self {
        Self { threads: 1, seed: None, unit_ns: 0 }
    }

    /// The default thread count: `std::thread::available_parallelism`, or 1.
    pub fn available() -> usize {
        std::thread::available_parallelism().map_or(1, std::num::NonZeroUsize::get)
    }

    /// The same pool with `seed` as its scheduling seed (`None`: none),
    /// whatever the environment says.
    #[must_use]
    pub fn with_seed(self, seed: Option<u64>) -> Self {
        Self { seed, ..self }
    }

    /// The same pool with `threads` threads.
    ///
    /// # Panics
    ///
    /// If `threads` is 0.
    #[must_use]
    pub fn with_threads(self, threads: usize) -> Self {
        assert!(threads >= 1, "a pool has at least one thread");
        Self { threads, ..self }
    }

    /// The same pool for one region whose units cost about `ns`
    /// nanoseconds each, on one thread (a constant of the call site, from
    /// its measured cost: `spec/landed/perf/WP-032g.md`).
    ///
    /// The region then runs on at most `units * ns / `[`WORKER_NS`] workers
    /// (at least one, at most [`Pool::threads`]): a region too small to
    /// repay a thread's spawn runs on the caller, without one. The count
    /// depends on the number of units and the constant only, never on a
    /// measurement, and like every schedule it changes no result. With a
    /// scheduling seed the estimate is ignored (every worker runs: the
    /// seed's purpose is to exercise schedules).
    #[must_use]
    pub fn unit_cost(self, ns: u32) -> Self {
        Self { unit_ns: ns, ..self }
    }

    /// The pool one region of `units` units runs on ([`Pool::unit_cost`]).
    fn sized(&self, units: usize) -> Self {
        let mut pool = Self { unit_ns: 0, ..*self };
        if self.unit_ns != 0 && self.seed.is_none() {
            let work = (units as u128).saturating_mul(u128::from(self.unit_ns));
            let workers = usize::try_from(work / WORKER_NS).unwrap_or(usize::MAX).max(1);
            pool.threads = self.threads.min(workers);
        }
        pool
    }

    /// The thread count.
    pub fn threads(&self) -> usize {
        self.threads
    }

    /// The scheduling seed, if any.
    pub fn seed(&self) -> Option<u64> {
        self.seed
    }

    /// Whether every helper runs on the caller in unit order (one thread,
    /// no seed). A phase may take a lock-free path then.
    pub fn is_inline(&self) -> bool {
        self.threads == 1 && self.seed.is_none()
    }

    /// The number of consecutive units one item holds when `units` units
    /// are batched: all of them inline, one with a seed. Not observable.
    fn grain(&self, units: usize) -> usize {
        if self.is_inline() {
            units.max(1)
        } else if self.seed.is_some() {
            1
        } else {
            units.div_ceil(self.threads * BLOCKS_PER_THREAD).max(1)
        }
    }

    /// Runs `work(state, item)` for every item in `0..items`, each exactly
    /// once, `state` made by `init` once per worker.
    #[track_caller]
    fn drive<S, I, W>(&self, units: usize, items: usize, init: I, work: W)
    where
        I: Fn() -> S + Sync,
        W: Fn(&mut S, usize) + Sync,
    {
        if items == 0 {
            return;
        }
        let mut region = Region::start(units, self.threads.min(items));
        let perm = self.seed.map(|seed| permutation(items, seed));
        let workers = self.threads.min(items);
        if workers == 1 {
            let mut state = init();
            for c in 0..items {
                work(&mut state, perm.as_ref().map_or(c, |p| p[c]));
                if perm.is_some() {
                    std::thread::yield_now();
                }
            }
            return;
        }
        let team = Team {
            items,
            workers,
            cursor: AtomicUsize::new(0),
            started: AtomicUsize::new(1),
            stop: AtomicBool::new(false),
            payload: Mutex::new(None),
            init: &init,
            work: &work,
            perm: perm.as_deref(),
        };
        std::thread::scope(|scope| team.run(scope, 1));
        region.spawned = team.started.into_inner() - 1;
        if let Some(p) = team.payload.into_inner().unwrap_or_else(PoisonError::into_inner) {
            std::panic::resume_unwind(p);
        }
    }

    /// Runs `work(state, index, part)` for every part, each exactly once.
    #[track_caller]
    fn drive_parts<X, S, P, I, W>(&self, units: Option<usize>, parts: P, init: I, work: W)
    where
        X: Send,
        P: IntoIterator<Item = X>,
        I: Fn() -> S + Sync,
        W: Fn(&mut S, usize, X) + Sync,
    {
        let parts = parts.into_iter();
        let this = self.sized(units.unwrap_or_else(|| parts.size_hint().0));
        if this.is_inline() {
            let mut region = Region::start(0, 1);
            let mut state = init();
            let mut n = 0;
            for (i, x) in parts.enumerate() {
                work(&mut state, i, x);
                n += 1;
            }
            region.items = units.unwrap_or(n);
            return;
        }
        let cells: Vec<Mutex<Option<X>>> = parts.map(|x| Mutex::new(Some(x))).collect();
        this.drive(units.unwrap_or(cells.len()), cells.len(), init, |state, i| {
            let x = cells[i]
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .take()
                .expect("each part is handed out once");
            work(state, i, x);
        });
    }

    /// Runs `f(unit)` for every unit of `0..units`.
    ///
    /// Cost: batches of consecutive units per item; no allocation beyond
    /// the call's (with a seed: the permutation, one `usize` per unit).
    #[track_caller]
    pub fn for_each<F>(&self, units: usize, f: F)
    where
        F: Fn(usize) + Sync,
    {
        self.for_each_init(units, || (), |(), u| f(u));
    }

    /// Runs `f(state, unit)` for every unit of `0..units`, `state` made by
    /// `init` once per worker (a scratch buffer reused across units). What
    /// `state` holds at the end is dropped: it must not carry a result.
    #[track_caller]
    pub fn for_each_init<S, I, F>(&self, units: usize, init: I, f: F)
    where
        I: Fn() -> S + Sync,
        F: Fn(&mut S, usize) + Sync,
    {
        let this = self.sized(units);
        let g = this.grain(units);
        this.drive(units, units.div_ceil(g), init, |state, b| {
            for u in b * g..((b + 1) * g).min(units) {
                f(state, u);
            }
        });
    }

    /// Runs `f(index, part)` for every part, each part a unit (rule 1: the
    /// parts are disjoint, e.g. from [`split_offsets`], or `chunks_mut`).
    ///
    /// Cost: unless inline, one `Mutex<Option<X>>` per part, allocated once.
    #[track_caller]
    pub fn for_each_part<X, P, F>(&self, parts: P, f: F)
    where
        X: Send,
        P: IntoIterator<Item = X>,
        F: Fn(usize, X) + Sync,
    {
        self.drive_parts(None, parts, || (), |(), i, x| f(i, x));
    }

    /// [`Pool::for_each_part`] with a per-worker state made by `init`.
    #[track_caller]
    pub fn for_each_part_init<X, S, P, I, F>(&self, parts: P, init: I, f: F)
    where
        X: Send,
        P: IntoIterator<Item = X>,
        I: Fn() -> S + Sync,
        F: Fn(&mut S, usize, X) + Sync,
    {
        self.drive_parts(None, parts, init, f);
    }

    /// Runs `f(index, item)` for every element of `items`, each element a
    /// unit (rule 1).
    ///
    /// Cost: batches of consecutive elements per item; one `Mutex` per
    /// batch unless inline.
    #[track_caller]
    pub fn for_each_mut<T, F>(&self, items: &mut [T], f: F)
    where
        T: Send,
        F: Fn(usize, &mut T) + Sync,
    {
        let this = self.sized(items.len());
        let g = this.grain(items.len());
        this.drive_parts(
            Some(items.len()),
            items.chunks_mut(g),
            || (),
            |(), b, chunk| {
                for (i, x) in chunk.iter_mut().enumerate() {
                    f(b * g + i, x);
                }
            },
        );
    }

    /// Runs `f(block_index, range)` for every block of [`blocks`]`(len,
    /// block)`, each block a unit (rule 6).
    #[track_caller]
    pub fn for_each_block<F>(&self, len: usize, block: usize, f: F)
    where
        F: Fn(usize, Range<usize>) + Sync,
    {
        let n = block_count(len, block);
        self.sized(len).for_each(n, |b| f(b, b * block..((b + 1) * block).min(len)));
    }

    /// `(0..units).map(f).collect()` (rule 3).
    ///
    /// Cost: one buffer per batch and one copy of the results into the
    /// returned vector (none inline).
    #[track_caller]
    pub fn map<R, F>(&self, units: usize, f: F) -> Vec<R>
    where
        R: Send,
        F: Fn(usize) -> R + Sync,
    {
        let this = self.sized(units);
        if this.is_inline() {
            let _region = Region::start(units, 1);
            return (0..units).map(f).collect();
        }
        let g = this.grain(units);
        let mut outs: Vec<Vec<R>> = (0..units.div_ceil(g)).map(|_| Vec::new()).collect();
        this.drive_parts(
            Some(units),
            outs.iter_mut(),
            || (),
            |(), b, out| {
                let range = b * g..((b + 1) * g).min(units);
                out.reserve_exact(range.len());
                out.extend(range.map(&f));
            },
        );
        concat(outs, units)
    }

    /// Ordered map-and-collect (rule 3): `f(unit, out)` appends unit's rows
    /// to `out`; the result holds the rows of every unit in unit order,
    /// with the offsets of each unit's rows.
    ///
    /// Cost: one buffer per batch (one in all inline), one copy of the rows
    /// into the result (none inline), `units + 1` offsets.
    #[track_caller]
    pub fn map_collect<R, F>(&self, units: usize, f: F) -> Collected<R>
    where
        R: Send,
        F: Fn(usize, &mut Vec<R>) + Sync,
    {
        let this = self.sized(units);
        let g = this.grain(units);
        let nb = units.div_ceil(g);
        let mut outs: Vec<Vec<R>> = (0..nb).map(|_| Vec::new()).collect();
        let mut offsets = vec![0usize; units + 1];
        this.drive_parts(
            Some(units),
            outs.iter_mut().zip(offsets[1..].chunks_mut(g)),
            || (),
            |(), b, (out, ends)| {
                for (i, end) in ends.iter_mut().enumerate() {
                    f(b * g + i, out);
                    *end = out.len();
                }
            },
        );
        // Each block's ends are relative to the block: make them absolute.
        let mut base = 0;
        for (out, ends) in outs.iter().zip(offsets[1..].chunks_mut(g)) {
            for end in ends {
                *end += base;
            }
            base += out.len();
        }
        Collected { items: concat(outs, base), offsets }
    }

    /// `(0..units).map(|u| { let mut v = Vec::new(); f(u, &mut v); v })`:
    /// one vector per unit (rule 3). Cost: as [`Pool::map`], plus each
    /// unit's own vector.
    #[track_caller]
    pub fn map_collect_vecs<R, F>(&self, units: usize, f: F) -> Vec<Vec<R>>
    where
        R: Send,
        F: Fn(usize, &mut Vec<R>) + Sync,
    {
        self.map(units, |u| {
            let mut v = Vec::new();
            f(u, &mut v);
            v
        })
    }

    /// Replaces `values` by their exclusive prefix sums and returns the
    /// total: `values[i]` becomes the sum of the elements before `i`. The
    /// caller guarantees the total fits `T`.
    ///
    /// Cost: sequential up to [`SCAN_BLOCK`] elements; beyond, two passes
    /// over blocks of [`SCAN_BLOCK`] and one allocation per block sum.
    #[track_caller]
    pub fn prefix_sum<T>(&self, values: &mut [T]) -> T
    where
        T: Copy + Default + Add<Output = T> + Send + Sync,
    {
        let this = self.sized(values.len());
        if values.len() <= SCAN_BLOCK || this.is_inline() {
            let _region = Region::start(values.len(), 1);
            return scan(values, T::default());
        }
        let shared: &[T] = values;
        let mut sums = this.map(block_count(shared.len(), SCAN_BLOCK), |b| {
            let start = b * SCAN_BLOCK;
            let end = (start + SCAN_BLOCK).min(shared.len());
            shared[start..end].iter().fold(T::default(), |acc, &x| acc + x)
        });
        let total = scan(&mut sums, T::default());
        this.for_each_part(values.chunks_mut(SCAN_BLOCK), |b, chunk| {
            scan(chunk, sums[b]);
        });
        total
    }

    /// Sorts `v` by `cmp` (rule 4), stably: the result is exactly that of
    /// `v.sort_by(cmp)` for every thread count and schedule. Rows `cmp`
    /// calls equal keep their order in `v`, so when the order of `v` itself
    /// comes from a schedule, `cmp` must be total on the rows.
    ///
    /// Design: runs of at least [`SORT_RUN`] and at most [`SORT_MAX_RUNS`]
    /// of them (a function of the length) are sorted in parallel by
    /// `slice::sort_by`; then rounds merge neighbouring runs, each merge cut
    /// into output blocks of [`MERGE_BLOCK`] located by binary search
    /// ("merge path"), so that every round is parallel to the end.
    ///
    /// Cost: up to [`SORT_RUN`] elements, or inline, `slice::sort_by`.
    /// Beyond: a scratch copy of `v` (hence `T: Copy`: rows are plain data,
    /// `spec/model.md` §1), at most `log2(SORT_MAX_RUNS)` merge rounds each
    /// moving every element once, and a final copy when the rounds are odd.
    #[track_caller]
    pub fn sort_by<T, F>(&self, v: &mut [T], cmp: F)
    where
        T: Copy + Send + Sync,
        F: Fn(&T, &T) -> Ordering + Sync,
    {
        let n = v.len();
        let this = self.sized(n);
        if n <= SORT_RUN || this.is_inline() {
            let _region = Region::start(n, 1);
            v.sort_by(cmp);
            return;
        }
        let run = SORT_RUN.max(n.div_ceil(SORT_MAX_RUNS));
        this.for_each_part(v.chunks_mut(run), |_, chunk| chunk.sort_by(&cmp));
        if run >= n {
            return;
        }
        let mut scratch = v.to_vec();
        let mut width = run;
        let mut in_v = true;
        while width < n {
            if in_v {
                this.merge_round(v, &mut scratch, width, &cmp);
            } else {
                this.merge_round(&scratch, v, width, &cmp);
            }
            in_v = !in_v;
            width = width.saturating_mul(2);
        }
        if !in_v {
            this.for_each_part(
                v.chunks_mut(MERGE_BLOCK).zip(scratch.chunks(MERGE_BLOCK)),
                |_, (to, from)| {
                    to.copy_from_slice(from);
                },
            );
        }
    }

    /// [`Pool::sort_by`] comparing `key(row)`; the result is that of
    /// `v.sort_by_key(key)`.
    #[track_caller]
    pub fn sort_by_key<T, K, F>(&self, v: &mut [T], key: F)
    where
        T: Copy + Send + Sync,
        K: Ord,
        F: Fn(&T) -> K + Sync,
    {
        self.sort_by(v, |a, b| key(a).cmp(&key(b)));
    }

    /// Merges each pair of neighbouring sorted runs of `width` in `src` into
    /// `dst`.
    #[track_caller]
    fn merge_round<T, F>(&self, src: &[T], dst: &mut [T], width: usize, cmp: &F)
    where
        T: Copy + Send + Sync,
        F: Fn(&T, &T) -> Ordering + Sync,
    {
        let n = src.len();
        let mut pieces = Vec::new();
        let mut rest: &mut [T] = dst;
        let mut start = 0;
        while start < n {
            let mid = (start + width).min(n);
            let end = (start + width.saturating_mul(2)).min(n);
            let (a, b) = (&src[start..mid], &src[mid..end]);
            let mut k = 0;
            while k < end - start {
                let k1 = (k + MERGE_BLOCK).min(end - start);
                let (out, tail) = std::mem::take(&mut rest).split_at_mut(k1 - k);
                rest = tail;
                pieces.push(Piece { a, b, k0: k, out });
                k = k1;
            }
            start = end;
        }
        self.for_each_part(pieces, |_, piece| piece.merge(cmp));
    }
}

/// One output block of a merge: outputs `k0..k0 + out.len()` of the stable
/// merge of `a` and `b`.
struct Piece<'a, T> {
    a: &'a [T],
    b: &'a [T],
    k0: usize,
    out: &'a mut [T],
}

impl<T: Copy> Piece<'_, T> {
    fn merge<F: Fn(&T, &T) -> Ordering>(self, cmp: &F) {
        let k1 = self.k0 + self.out.len();
        let i0 = merge_split(self.a, self.b, self.k0, cmp);
        let i1 = merge_split(self.a, self.b, k1, cmp);
        let (mut a, mut b) = (&self.a[i0..i1], &self.b[self.k0 - i0..k1 - i1]);
        for slot in self.out.iter_mut() {
            // Ties take from `a`, the earlier run: the merge is stable.
            let take_a = match (a.first(), b.first()) {
                (Some(x), Some(y)) => cmp(y, x) != Ordering::Less,
                (Some(_), None) => true,
                (None, _) => false,
            };
            if take_a {
                *slot = a[0];
                a = &a[1..];
            } else {
                *slot = b[0];
                b = &b[1..];
            }
        }
    }
}

/// The number of elements of `a` among the first `k` outputs of the stable
/// merge of `a` and `b` (ties to `a`): the least `i` with `k - i == 0`, or
/// `i == a.len()`, or `b[k - i - 1] < a[i]`.
fn merge_split<T, F: Fn(&T, &T) -> Ordering>(a: &[T], b: &[T], k: usize, cmp: &F) -> usize {
    let mut lo = k.saturating_sub(b.len());
    let mut hi = k.min(a.len());
    while lo < hi {
        let i = lo + (hi - lo) / 2;
        if cmp(&b[k - i - 1], &a[i]) == Ordering::Less {
            hi = i;
        } else {
            lo = i + 1;
        }
    }
    lo
}

/// Concatenates `parts` (`total` rows in all) in order.
fn concat<R>(mut parts: Vec<Vec<R>>, total: usize) -> Vec<R> {
    if parts.len() == 1 {
        return parts.pop().unwrap_or_default();
    }
    let mut all = Vec::with_capacity(total);
    for part in parts {
        all.extend(part);
    }
    all
}

/// Exclusive prefix sums of `values` from `start`; returns the total.
fn scan<T: Copy + Add<Output = T>>(values: &mut [T], start: T) -> T {
    let mut acc = start;
    for x in values {
        let next = acc + *x;
        *x = acc;
        acc = next;
    }
    acc
}

/// One parallel region's line under `MORTAR_STEPS` (WP-032g), printed when
/// the region ends:
///
/// `mortar: steps: par <file>:<line>: <ms> ms, <u> item, <w> worker, <s> spawned`
///
/// `<file>:<line>` is the helper's caller, `u` its units, `w` the most
/// workers it may use and `s` the threads it did spawn.
/// Off, one branch on a cached flag.
struct Region {
    at: &'static std::panic::Location<'static>,
    items: usize,
    workers: usize,
    spawned: usize,
    start: Option<std::time::Instant>,
}

impl Region {
    #[track_caller]
    fn start(items: usize, workers: usize) -> Self {
        let start = crate::wp032b_steps::enabled().then(std::time::Instant::now);
        Self { at: std::panic::Location::caller(), items, workers, spawned: 0, start }
    }
}

impl Drop for Region {
    #[allow(clippy::cast_precision_loss)] // a printed duration
    fn drop(&mut self) {
        if let Some(start) = self.start {
            let ms = start.elapsed().as_nanos() as f64 / 1e6;
            let file = self.at.file().rsplit("crates/").next().unwrap_or(self.at.file());
            mortar_diag::stream::to_stderr(&format!(
                "mortar: steps: par {file}:{}: {ms:.3} ms, {} item, {} worker, {} spawned\n",
                self.at.line(),
                self.items,
                self.workers,
                self.spawned
            ));
        }
    }
}

/// The workers of one region (`drive`). The caller is the first; it spawns
/// one more, and every spawned worker spawns up to [`FANOUT`] more before
/// it takes items, while fewer than `workers` have started. Then each takes
/// the next item from `cursor` until none is left.
///
/// Without a seed a worker spawns only while items are left untaken: in a
/// region that is over before a spawned thread starts, no further thread
/// is spawned. That is a schedule the eager spawn already allowed (threads
/// that start after the last item is taken do nothing), so it changes no
/// result. With a seed every worker is spawned.
struct Team<'a, I, W> {
    items: usize,
    workers: usize,
    cursor: AtomicUsize,
    /// Workers started, the caller included.
    started: AtomicUsize,
    /// Set by a worker that panicked: the others take no further item.
    stop: AtomicBool,
    /// The first panic's payload, resumed on the caller.
    payload: Mutex<Option<Box<dyn std::any::Any + Send>>>,
    init: &'a I,
    work: &'a W,
    perm: Option<&'a [usize]>,
}

/// The threads a spawned worker of a [`Team`] spawns (a tree: no thread
/// pays for every spawn).
const FANOUT: usize = 2;

impl<S, I, W> Team<'_, I, W>
where
    I: Fn() -> S + Sync,
    W: Fn(&mut S, usize) + Sync,
{
    fn run<'scope>(&'scope self, scope: &'scope std::thread::Scope<'scope, '_>, fanout: usize) {
        for _ in 0..fanout {
            if self.perm.is_none() && self.cursor.load(atomic::Ordering::Relaxed) >= self.items {
                break;
            }
            let claimed = self
                .started
                .fetch_update(atomic::Ordering::Relaxed, atomic::Ordering::Relaxed, |n| {
                    (n < self.workers).then_some(n + 1)
                })
                .is_ok();
            if !claimed {
                break;
            }
            // A thread that cannot be spawned (a resource limit) leaves its
            // share to the others: the result does not depend on it.
            if std::thread::Builder::new()
                .name("mortar-par".to_owned())
                .spawn_scoped(scope, move || self.run(scope, FANOUT))
                .is_err()
            {
                self.started.fetch_sub(1, atomic::Ordering::Relaxed);
                break;
            }
        }
        let caught = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            let mut state = (self.init)();
            while !self.stop.load(atomic::Ordering::Relaxed) {
                let c = self.cursor.fetch_add(1, atomic::Ordering::Relaxed);
                if c >= self.items {
                    break;
                }
                (self.work)(&mut state, self.perm.map_or(c, |p| p[c]));
                if self.perm.is_some() {
                    std::thread::yield_now();
                }
            }
        }));
        if let Err(p) = caught {
            self.stop.store(true, atomic::Ordering::Relaxed);
            self.payload.lock().unwrap_or_else(PoisonError::into_inner).get_or_insert(p);
        }
    }
}

/// A permutation of `0..n` drawn from `seed` (splitmix64, Fisher-Yates).
fn permutation(n: usize, seed: u64) -> Vec<usize> {
    let mut state = seed ^ (n as u64).wrapping_mul(0x9e37_79b9_7f4a_7c15);
    let mut next = move || {
        state = state.wrapping_add(0x9e37_79b9_7f4a_7c15);
        let mut z = state;
        z = (z ^ (z >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
        z ^ (z >> 31)
    };
    let mut perm: Vec<usize> = (0..n).collect();
    for i in (1..n).rev() {
        let j = (next() % (i as u64 + 1)) as usize;
        perm.swap(i, j);
    }
    perm
}

/// The rows of [`Pool::map_collect`]: every unit's rows in unit order.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Collected<R> {
    /// The rows, unit after unit.
    pub items: Vec<R>,
    /// `units + 1` offsets: unit `u`'s rows are `items[offsets[u]..offsets[u + 1]]`.
    pub offsets: Vec<usize>,
}

impl<R> Collected<R> {
    /// The number of units.
    pub fn units(&self) -> usize {
        self.offsets.len() - 1
    }

    /// The rows of unit `u`.
    pub fn unit(&self, u: usize) -> &[R] {
        &self.items[self.offsets[u]..self.offsets[u + 1]]
    }
}

/// The number of blocks of `block` elements covering `len` (rule 6).
///
/// # Panics
///
/// If `block` is 0.
pub fn block_count(len: usize, block: usize) -> usize {
    assert!(block > 0, "a block holds at least one element");
    len.div_ceil(block)
}

/// The fixed-size blocks covering `0..len` (rule 6): `block` elements each,
/// the last one shorter. A function of `len` and `block` only.
///
/// # Panics
///
/// If `block` is 0.
pub fn blocks(len: usize, block: usize) -> impl ExactSizeIterator<Item = Range<usize>> + Clone {
    (0..block_count(len, block)).map(move |b| b * block..((b + 1) * block).min(len))
}

/// The disjoint mutable slices `slice[r]` for each range of `ranges` (rule
/// 1). The ranges must be ascending and disjoint; gaps are allowed.
///
/// # Panics
///
/// If a range is reversed, overlaps the one before, or lies beyond the
/// slice.
pub fn split_ranges<'a, T>(slice: &'a mut [T], ranges: &[Range<usize>]) -> Vec<&'a mut [T]> {
    let mut parts = Vec::with_capacity(ranges.len());
    let mut rest = slice;
    let mut pos = 0;
    for r in ranges {
        assert!(
            pos <= r.start && r.start <= r.end,
            "split_ranges: range {r:?} is reversed or overlaps the one before (which ends at {pos})"
        );
        assert!(r.end - pos <= rest.len(), "split_ranges: range {r:?} lies beyond the slice");
        let (_, tail) = std::mem::take(&mut rest).split_at_mut(r.start - pos);
        let (part, tail) = tail.split_at_mut(r.len());
        parts.push(part);
        rest = tail;
        pos = r.end;
    }
    parts
}

/// The disjoint mutable slices `slice[offsets[i]..offsets[i + 1]]` (rule 1):
/// `offsets` is a prefix sum, e.g. [`Collected::offsets`].
///
/// # Panics
///
/// If `offsets` decreases or ends beyond the slice.
pub fn split_offsets<'a, T>(slice: &'a mut [T], offsets: &[usize]) -> Vec<&'a mut [T]> {
    let ranges: Vec<Range<usize>> = offsets.windows(2).map(|w| w[0]..w[1]).collect();
    split_ranges(slice, &ranges)
}

/// Test support: the thread counts every helper and phase is tested
/// under (`spec/model.md` §11).
pub const TEST_THREADS: [usize; 4] = [1, 2, 7, 64];

/// Test support: the scheduling seeds tests run under besides none.
pub const TEST_SEEDS: [u64; 3] = [1, 0x005e_ed0f_0dd5, 0xffff_ffff_ffff_ffff];

/// Test support (not for the linker's own use): runs `run` under every
/// thread count of [`TEST_THREADS`] with no seed and with each seed of
/// [`TEST_SEEDS`], requires every result to equal the first (one thread, no
/// seed: the sequential meaning), and returns it.
///
/// # Panics
///
/// If two results differ: the panic names the schedule.
pub fn check_schedules<R, F>(run: F) -> R
where
    R: PartialEq + fmt::Debug,
    F: Fn(&Pool) -> R,
{
    let reference = run(&Pool::sequential());
    for threads in TEST_THREADS {
        for seed in std::iter::once(None).chain(TEST_SEEDS.map(Some)) {
            let pool = Pool::sequential().with_threads(threads).with_seed(seed);
            if pool.is_inline() {
                continue;
            }
            let got = run(&pool);
            assert!(
                got == reference,
                "the result depends on the schedule: {threads} threads, seed {seed:?} gave\n\
                 {}\nwhere one thread gave\n{}",
                shown(&got),
                shown(&reference),
            );
        }
    }
    reference
}

/// `value`'s `Debug` form, cut to a readable length.
fn shown<R: fmt::Debug>(value: &R) -> String {
    const MAX: usize = 2000;
    let mut text = format!("{value:?}");
    if text.len() > MAX {
        let mut cut = MAX;
        while !text.is_char_boundary(cut) {
            cut -= 1;
        }
        text.truncate(cut);
        text.push_str(" ...");
    }
    text
}
