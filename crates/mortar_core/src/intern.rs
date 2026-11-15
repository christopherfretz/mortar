//! Interning and the numbering of `spec/model.md` §2.3.
//!
//! [`Interner`] is a concurrent, insert-only table from keys (byte strings
//! borrowed from the input, [`SymName`] for symbols) to [`Slot`]s. Each slot
//! keeps the key's **first mention**, the minimum of the mentions inserted
//! for it (§6 rule 2), whatever the order of the inserts.
//!
//! [`number`] is §2.3's numbering built on it: given each unit's keys
//! (a file's non-local symbol-table entries, or its group signatures), it
//! gives every mention its id and returns the distinct keys in id order,
//! exactly as the sequential definition does (units in order, each key
//! taking the next id when first seen), for every thread count and
//! schedule.
//!
//! # Design
//!
//! One open-addressing table, sized up front, with no lock: an array of
//! cells, each a `OnceLock` holding an entry (key, hash, first mention as an
//! `AtomicU64`), probed linearly from a multiply-shift reduction of the
//! hash. A new key is written into the first empty cell of its probe
//! sequence by `OnceLock::set`; a worker that loses the race for a cell
//! reads the winner's entry and goes on probing if the keys differ. The
//! first mention is updated with `fetch_min`, after a plain load shows it
//! would lower it. A reader waits only for a cell being written at that
//! very moment (the 48 bytes of an entry), never for a lock held across a
//! probe: the sharded-mutex design this replaced lost its scaling on a
//! loaded host, where a preempted lock holder stalls every thread wanting
//! its shard (the numbers are in `spec/landed/model/WP-020b.md`, with the
//! other designs measured).
//!
//! A table of this kind cannot grow while it is shared, so [`number`]
//! sizes it from a `HyperLogLog` estimate of the distinct keys (one pass
//! hashing every key; mold sizes its symbol table the same way) and,
//! should the estimate fall short, runs pass 1 again with twice the size:
//! the result is the same either way. A slot is a cell's index: which slot
//! a key gets depends on the schedule, so slots are never observable; ids
//! are.

use std::marker::PhantomData;
use std::sync::OnceLock;
use std::sync::atomic::{self, AtomicBool, AtomicU8, AtomicU32, AtomicU64, AtomicUsize};

use crate::hash::{combine, hash_bytes};
use crate::id::Id;
use crate::par::{Pool, split_offsets};

/// A key an [`Interner`] holds: plain data, compared bytewise, hashed by
/// the fixed hash of [`crate::hash`].
pub trait InternKey: Copy + Eq + Send + Sync {
    /// The fixed hash of the key. Equal keys have equal hashes.
    fn hash(&self) -> u64;
}

impl InternKey for &[u8] {
    #[inline]
    fn hash(&self) -> u64 {
        hash_bytes(self)
    }
}

impl InternKey for u64 {
    #[inline]
    fn hash(&self) -> u64 {
        crate::hash::hash_u64(*self)
    }
}

/// The key of a global symbol (§2.3): the name before any `@`, and the
/// version if it is a non-default one. `foo` and `foo@@V2` are one key
/// (`name: foo`, no version); `foo@V1` is another (`name: foo`, version
/// `V1`). Both parts borrow the input's bytes: the version may come from
/// the name itself (a relocatable object's `foo@V1`) or from elsewhere (a
/// shared object's version definitions), and neither needs an allocation.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Debug)]
pub struct SymName<'a> {
    /// The name before the first `@`.
    pub name: &'a [u8],
    /// The non-default version, if any.
    pub version: Option<&'a [u8]>,
}

impl<'a> SymName<'a> {
    /// An unversioned (or default-version) name.
    pub fn plain(name: &'a [u8]) -> Self {
        Self { name, version: None }
    }

    /// A name with the non-default version `version`.
    pub fn versioned(name: &'a [u8], version: &'a [u8]) -> Self {
        Self { name, version: Some(version) }
    }

    /// The key of a symbol-table name as a relocatable object writes it:
    /// `foo`, `foo@@V2` (default version) or `foo@V1` (non-default). The
    /// split is at the first `@`.
    pub fn parse(raw: &'a [u8]) -> Self {
        match find_at(raw) {
            None => Self::plain(raw),
            Some(at) => match raw[at + 1..].strip_prefix(b"@") {
                Some(_) => Self::plain(&raw[..at]),
                None => Self::versioned(&raw[..at], &raw[at + 1..]),
            },
        }
    }
}

/// The index of the first `@` of `raw`, eight bytes at a time (a byte loop
/// costs more than the hash: `spec/landed/model/WP-020b.md`).
#[inline]
fn find_at(raw: &[u8]) -> Option<usize> {
    const ONES: u64 = 0x0101_0101_0101_0101;
    const HIGHS: u64 = 0x8080_8080_8080_8080;
    const ATS: u64 = 0x4040_4040_4040_4040;
    let mut words = raw.chunks_exact(8);
    let mut base = 0;
    for word in words.by_ref() {
        let mut w = [0; 8];
        w.copy_from_slice(word);
        let x = u64::from_le_bytes(w) ^ ATS;
        // The lowest flagged byte is the first zero byte of `x` exactly
        // (flags above it may be false).
        let found = x.wrapping_sub(ONES) & !x & HIGHS;
        if found != 0 {
            return Some(base + found.trailing_zeros() as usize / 8);
        }
        base += 8;
    }
    words.remainder().iter().position(|&c| c == b'@').map(|i| base + i)
}

impl InternKey for SymName<'_> {
    #[inline]
    fn hash(&self) -> u64 {
        let h = hash_bytes(self.name);
        match self.version {
            None => h,
            Some(v) => combine(h, hash_bytes(v)),
        }
    }
}

/// Where a key is mentioned: the unit (`FileId`) and the ordinal of the
/// mention within the unit (the entry's index among the file's non-local
/// entries, which orders them as the symbol-table index does). Ordered as
/// `(unit, ordinal)`, packed as `unit << 32 | ordinal` (§2.3 pass 1).
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Debug)]
pub struct Mention {
    /// The unit.
    pub unit: u32,
    /// The ordinal within the unit.
    pub ordinal: u32,
}

impl Mention {
    /// The packed form, `unit << 32 | ordinal`.
    pub fn pack(self) -> u64 {
        (u64::from(self.unit) << 32) | u64::from(self.ordinal)
    }

    /// The mention `packed` holds.
    pub fn unpack(packed: u64) -> Self {
        Self { unit: (packed >> 32) as u32, ordinal: packed as u32 }
    }
}

/// A key's place in an [`Interner`]: the index of its entry. Which slot a
/// key gets depends on the schedule: a slot is never observable.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Debug)]
pub struct Slot(pub u32);

/// The interner is as full as its size allows (the key of the insert that
/// says so may or may not be in it): the table is to be dropped, and
/// [`number`] starts again with a larger one.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Full;

/// A numbering would need more than 2^32 - 1 ids, or units, or mentions in
/// one unit: the link is refused by name (§1).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TooManyKeys;

impl std::fmt::Display for TooManyKeys {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("more than 2^32 - 1 distinct keys")
    }
}

impl std::error::Error for TooManyKeys {}

#[derive(Debug)]
struct Entry<K> {
    key: K,
    hash: u64,
    first: AtomicU64,
}

/// The new keys an [`Inserter`] counts before it adds them to the shared
/// count.
const COUNT_BATCH: usize = 64;

/// A concurrent, insert-only, fixed-size table from keys to [`Slot`]s,
/// keeping each key's first mention. Shared by reference among workers,
/// each inserting through an [`Inserter`] of its own; frozen into a
/// read-only [`Frozen`] when the parallel phase ends.
///
/// Cost: 5/3 cells per key it is sized for, each an entry in a
/// `OnceLock` (56 bytes for [`SymName`]); per insert one hash and a probe
/// (one hash compare per cell visited, key bytes compared on a match); for
/// a new key one `OnceLock::set`.
#[derive(Debug)]
pub struct Interner<K> {
    cells: Box<[OnceLock<Entry<K>>]>,
    /// The keys inserted, give or take what the inserters have not added
    /// yet.
    used: AtomicUsize,
    /// The most keys: three quarters of the cells, so that a probe ends.
    limit: usize,
}

impl<K: InternKey> Interner<K> {
    /// A table for about `keys` distinct keys (it is full at 1.25 times
    /// `keys`). Allocates its cells.
    pub fn with_capacity(keys: usize) -> Self {
        let cells = (keys.saturating_mul(5) / 3).clamp(64, u32::MAX as usize);
        Self {
            cells: (0..cells).map(|_| OnceLock::new()).collect(),
            used: AtomicUsize::new(0),
            limit: cells / 4 * 3,
        }
    }

    /// An inserter for one worker.
    pub fn inserter(&self) -> Inserter<'_, K> {
        Inserter { table: self, fresh: 0 }
    }

    /// The first probe cell of `hash`.
    #[inline]
    fn home(&self, hash: u64) -> usize {
        ((u128::from(hash) * self.cells.len() as u128) >> 64) as usize
    }

    #[inline]
    fn next(&self, i: usize) -> usize {
        if i + 1 == self.cells.len() { 0 } else { i + 1 }
    }

    #[inline]
    fn entry(&self, slot: Slot) -> &Entry<K> {
        self.cells[slot.0 as usize].get().expect("a slot names a full cell")
    }

    /// The slot of `key`, if it was inserted.
    pub fn get(&self, key: &K) -> Option<Slot> {
        let hash = key.hash();
        let mut i = self.home(hash);
        for _ in 0..self.cells.len() {
            let e = self.cells[i].get()?;
            if e.hash == hash && e.key == *key {
                return Some(Slot(i as u32));
            }
            i = self.next(i);
        }
        None
    }

    /// The table, read-only.
    pub fn freeze(self) -> Frozen<K> {
        Frozen { table: self }
    }
}

/// One worker's handle on an [`Interner`]: it counts the keys it adds in
/// batches, so that workers do not contend for one counter.
#[derive(Debug)]
pub struct Inserter<'t, K> {
    table: &'t Interner<K>,
    fresh: usize,
}

impl<K: InternKey> Inserter<'_, K> {
    /// Interns `key`, mentioned at `mention`: its slot, the first mention
    /// becoming the minimum of the old one and `mention`.
    pub fn insert(&mut self, key: K, mention: Mention) -> Result<Slot, Full> {
        let table = self.table;
        let hash = key.hash();
        let mention = mention.pack();
        let mut i = table.home(hash);
        for _ in 0..table.cells.len() {
            let cell = &table.cells[i];
            let e = match cell.get() {
                Some(e) => e,
                None => match cell.set(Entry { key, hash, first: AtomicU64::new(mention) }) {
                    Ok(()) => {
                        self.count()?;
                        return Ok(Slot(i as u32));
                    }
                    // Another worker filled the cell first.
                    Err(_) => cell.get().expect("a cell that refused a value holds one"),
                },
            };
            if e.hash == hash && e.key == key {
                if mention < e.first.load(atomic::Ordering::Relaxed) {
                    e.first.fetch_min(mention, atomic::Ordering::Relaxed);
                }
                return Ok(Slot(i as u32));
            }
            i = table.next(i);
        }
        Err(Full)
    }

    fn count(&mut self) -> Result<(), Full> {
        self.fresh += 1;
        if self.fresh == COUNT_BATCH {
            self.fresh = 0;
            let used = self.table.used.fetch_add(COUNT_BATCH, atomic::Ordering::Relaxed);
            if used + COUNT_BATCH > self.table.limit {
                return Err(Full);
            }
        }
        Ok(())
    }
}

/// A frozen [`Interner`]: read-only, shared freely.
#[derive(Debug)]
pub struct Frozen<K> {
    table: Interner<K>,
}

impl<K: InternKey> Frozen<K> {
    /// One more than the greatest slot (empty cells included): the size of
    /// an array indexed by slot.
    pub fn slots(&self) -> usize {
        self.table.cells.len()
    }

    /// The slot of `key`, if it was interned.
    pub fn get(&self, key: &K) -> Option<Slot> {
        self.table.get(key)
    }

    /// The key of `slot`.
    pub fn key(&self, slot: Slot) -> K {
        self.table.entry(slot).key
    }

    /// The hash of the key of `slot`.
    pub fn hash(&self, slot: Slot) -> u64 {
        self.table.entry(slot).hash
    }

    /// The first mention of the key of `slot`.
    pub fn first(&self, slot: Slot) -> Mention {
        Mention::unpack(self.table.entry(slot).first.load(atomic::Ordering::Relaxed))
    }

    /// The bytes the table holds.
    pub fn heap_bytes(&self) -> usize {
        self.table.cells.len() * size_of::<OnceLock<Entry<K>>>()
    }
}

/// `HyperLogLog` with 2^12 registers (standard error 1.6 %).
const HLL_BITS: u32 = 12;

/// One worker's `HyperLogLog` registers, merged into the shared ones (by
/// maximum, §6 rule 2) when the worker is done.
struct Sketch<'a> {
    local: Vec<u8>,
    shared: &'a [AtomicU8],
}

impl Sketch<'_> {
    #[inline]
    fn add(&mut self, hash: u64) {
        let register = (hash >> (64 - HLL_BITS)) as usize;
        let rank = ((hash << HLL_BITS) | (1 << (HLL_BITS - 1))).leading_zeros() as u8 + 1;
        let r = &mut self.local[register];
        *r = (*r).max(rank);
    }
}

impl Drop for Sketch<'_> {
    fn drop(&mut self) {
        for (shared, &local) in self.shared.iter().zip(&self.local) {
            if local > shared.load(atomic::Ordering::Relaxed) {
                shared.fetch_max(local, atomic::Ordering::Relaxed);
            }
        }
    }
}

/// The `HyperLogLog` estimate of the distinct keys of `units` units.
fn estimate<K, F, It>(pool: &Pool, units: usize, keys: &F) -> usize
where
    K: InternKey,
    F: Fn(usize) -> It + Sync,
    It: IntoIterator<Item = K>,
{
    let m = 1usize << HLL_BITS;
    let shared: Vec<AtomicU8> = (0..m).map(|_| AtomicU8::new(0)).collect();
    pool.unit_cost(600).for_each_init(
        units,
        || Sketch { local: vec![0; m], shared: &shared },
        |sketch, u| {
            for key in keys(u) {
                sketch.add(key.hash());
            }
        },
    );
    let registers: Vec<u8> = shared.into_iter().map(AtomicU8::into_inner).collect();
    let m = f64::from(1u32 << HLL_BITS);
    let sum: f64 = registers.iter().map(|&r| 2f64.powi(-i32::from(r))).sum();
    let zeros: u32 = registers.iter().map(|&r| u32::from(r == 0)).sum();
    let raw = 0.7213 / (1.0 + 1.079 / m) * m * m / sum;
    let e = if raw <= 2.5 * m && zeros > 0 { m * (m / f64::from(zeros)).ln() } else { raw };
    e.ceil() as usize
}

/// The distinct keys of a [`number`]ing, in id order, with lookup by key.
#[derive(Debug)]
pub struct Names<K, I> {
    table: Frozen<K>,
    /// Slot to id (holes: 0, never read).
    id_of: Vec<u32>,
    /// Id to slot.
    slot_of: Vec<Slot>,
    ids: PhantomData<fn() -> I>,
}

impl<K: InternKey, I: Id> Names<K, I> {
    /// The number of ids.
    pub fn len(&self) -> usize {
        self.slot_of.len()
    }

    /// Whether there are none.
    pub fn is_empty(&self) -> bool {
        self.slot_of.is_empty()
    }

    /// The key of `id`.
    pub fn key(&self, id: I) -> K {
        self.table.key(self.slot_of[id.slot()])
    }

    /// The fixed hash of the key of `id`.
    pub fn hash(&self, id: I) -> u64 {
        self.table.hash(self.slot_of[id.slot()])
    }

    /// The first mention of the key of `id`.
    pub fn first(&self, id: I) -> Mention {
        self.table.first(self.slot_of[id.slot()])
    }

    /// The id of `key`, if some unit mentions it.
    pub fn get(&self, key: &K) -> Option<I> {
        self.table.get(key).map(|s| I::from_index(self.id_of[s.0 as usize]))
    }

    /// The ids and their keys, in id order.
    pub fn iter(&self) -> impl ExactSizeIterator<Item = (I, K)> + '_ {
        self.slot_of.iter().enumerate().map(|(i, &s)| (I::from_index(i as u32), self.table.key(s)))
    }

    /// The bytes held: the frozen table and the two id maps.
    pub fn heap_bytes(&self) -> usize {
        self.table.heap_bytes() + self.id_of.capacity() * 4 + self.slot_of.capacity() * 4
    }
}

/// The result of [`number`].
#[derive(Debug)]
pub struct Numbering<K, I> {
    /// Per unit, per mention: the id of its key (`sym_map`, §3.2), unit
    /// after unit.
    pub map: Vec<I>,
    /// `units + 1` offsets: unit `u`'s mentions are
    /// `map[offsets[u]..offsets[u + 1]]`.
    pub offsets: Vec<usize>,
    /// The distinct keys in id order.
    pub names: Names<K, I>,
}

impl<K, I> Numbering<K, I> {
    /// The ids of unit `u`'s mentions.
    pub fn unit(&self, u: usize) -> &[I] {
        &self.map[self.offsets[u]..self.offsets[u + 1]]
    }
}

/// The numbering of `spec/model.md` §2.3: unit `u` (a `FileId`) mentions
/// `counts[u]` keys, which `keys(u)` yields in order (its non-local
/// symbol-table entries in table order; its group signatures in section
/// order). The result is the sequential definition's, for every thread
/// count and schedule: units in order, each mention in order, a key seen
/// for the first time taking the next id.
///
/// `keys` is called twice per unit: once to estimate the number of
/// distinct keys (the size of the table), once to intern them; a third
/// time for every unit in the rare case the estimate falls short.
///
/// The three passes of §2.3: (1) per unit, intern each key and leave its
/// slot in the map; (2) per unit, collect the slots whose first mention is
/// this mention, in order, and concatenate in unit order: that is the id
/// order; (3) give each slot its id, and per block rewrite the map from
/// slots to ids. Pass 3 of the spec is two steps here: an id is written by
/// its first mention's unit and read by every unit, which needs a join in
/// between.
///
/// Cost: the map (4 bytes per mention), its offsets, the interner, 8
/// bytes per distinct key for the id maps; no per-mention buffer beyond the
/// map.
///
/// # Panics
///
/// If `keys(u)` yields a number of keys other than `counts[u]` (a bug of
/// the caller's).
pub fn number<K, I, F, It>(
    pool: &Pool,
    counts: &[u32],
    keys: F,
) -> Result<Numbering<K, I>, TooManyKeys>
where
    K: InternKey,
    I: Id,
    F: Fn(usize) -> It + Sync,
    It: IntoIterator<Item = K>,
{
    let size = estimate(pool, counts.len(), &keys) + 16;
    number_sized(pool, counts, keys, size)
}

/// [`number`] with the table first sized for `size` keys instead of the
/// estimate. Test support: a test makes it start too small, to exercise
/// the retry.
#[doc(hidden)]
pub fn number_sized<K, I, F, It>(
    pool: &Pool,
    counts: &[u32],
    keys: F,
    mut size: usize,
) -> Result<Numbering<K, I>, TooManyKeys>
where
    K: InternKey,
    I: Id,
    F: Fn(usize) -> It + Sync,
    It: IntoIterator<Item = K>,
{
    if u32::try_from(counts.len()).is_err() {
        return Err(TooManyKeys);
    }
    let mut offsets = Vec::with_capacity(counts.len() + 1);
    let mut total = 0usize;
    for &c in counts {
        offsets.push(total);
        total += c as usize;
    }
    offsets.push(total);
    let mut map = vec![I::from_index(0); total];

    // Pass 1: intern, leaving slots in the map.
    let table = loop {
        let interner = Interner::with_capacity(size);
        let full = AtomicBool::new(false);
        pool.unit_cost(800).for_each_part_init(
            split_offsets(&mut map, &offsets),
            || interner.inserter(),
            |ins, u, out| {
                if !full.load(atomic::Ordering::Relaxed)
                    && fill(keys(u), u, out, |k, m| ins.insert(k, m)).is_err()
                {
                    full.store(true, atomic::Ordering::Relaxed);
                }
            },
        );
        if !full.into_inner() {
            break interner.freeze();
        }
        if interner.cells.len() >= u32::MAX as usize {
            return Err(TooManyKeys);
        }
        size = size.saturating_mul(2).max(64);
    };

    // Pass 2: the first mentions, in unit order, are the ids in order.
    let collected = pool.unit_cost(400).map_collect(counts.len(), |u, out| {
        for (n, s) in map[offsets[u]..offsets[u + 1]].iter().enumerate() {
            let slot = Slot(s.index());
            if table.first(slot) == (Mention { unit: u as u32, ordinal: n as u32 }) {
                out.push(slot);
            }
        }
    });
    let slot_of = collected.items;
    if u32::try_from(slot_of.len()).is_err() {
        return Err(TooManyKeys);
    }

    // Pass 3: ids by slot, then the map from slots to ids.
    let ids: Vec<AtomicU32> = (0..table.slots()).map(|_| AtomicU32::new(0)).collect();
    pool.unit_cost(11600).for_each_block(slot_of.len(), REWRITE_BLOCK, |_, range| {
        for id in range {
            ids[slot_of[id].0 as usize].store(id as u32, atomic::Ordering::Relaxed);
        }
    });
    pool.unit_cost(6300).for_each_part(map.chunks_mut(REWRITE_BLOCK), |_, chunk| {
        for x in chunk {
            *x = I::from_index(ids[x.slot()].load(atomic::Ordering::Relaxed));
        }
    });
    let id_of = ids.into_iter().map(AtomicU32::into_inner).collect();
    Ok(Numbering { map, offsets, names: Names { table, id_of, slot_of, ids: PhantomData } })
}

/// Pass 1 for unit `u`: interns its keys, leaving their slots in `out`.
fn fill<K, I, It, F>(keys: It, u: usize, out: &mut [I], mut intern: F) -> Result<(), Full>
where
    I: Id,
    It: IntoIterator<Item = K>,
    F: FnMut(K, Mention) -> Result<Slot, Full>,
{
    let mut n = 0;
    for key in keys {
        assert!(n < out.len(), "unit {u} yields more than its {} keys", out.len());
        let slot = intern(key, Mention { unit: u as u32, ordinal: n as u32 })?;
        out[n] = I::from_index(slot.0);
        n += 1;
    }
    assert!(n == out.len(), "unit {u} yields {n} keys, not its {}", out.len());
    Ok(())
}

/// The block of pass 3 (rule 6: a constant; not observable here).
const REWRITE_BLOCK: usize = 1 << 14;
