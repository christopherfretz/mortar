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
//! [`SHARDS`] shards (a constant, §6 rule 6), chosen by the hash's top
//! bits, each an open-addressing table behind its own `Mutex`, padded to
//! its own cache lines. A shard holds its entries in insertion order (key,
//! hash, first mention: 48 bytes for a [`SymName`]) and a probe array of
//! `u64` cells, each the hash's upper half and the entry's index (linear
//! probing, at most 5/8 full), so that a probe compares key bytes only on a
//! 32-bit tag match. The hash is computed before the lock is taken; the
//! lock covers the probe, the key comparison and the minimum. A slot is the
//! shard and the entry's index in it: which slot a key gets depends on the
//! schedule, so slots are never observable; ids are. The measurements and
//! the designs rejected are in `spec/landed/model/WP-020b.md`.

use std::marker::PhantomData;
use std::sync::atomic::{self, AtomicBool, AtomicU32};
use std::sync::{Mutex, PoisonError};

use crate::hash::{combine, hash_bytes};
use crate::id::Id;
use crate::par::{Pool, split_offsets};

/// log2 of [`SHARDS`].
pub const SHARD_BITS: u32 = 8;

/// The number of shards of an [`Interner`]: a constant (§6 rule 6).
pub const SHARDS: usize = 1 << SHARD_BITS;

/// The bits of a slot that index an entry within its shard.
const LOCAL_BITS: u32 = 32 - SHARD_BITS;

/// The most entries one shard holds: 2^24 - 1, so that a slot fits 32 bits.
/// With the fixed hash a shard reaches it only when the keys number about
/// 2^32, beyond what §1 allows anyway.
pub const SHARD_CAPACITY: usize = (1 << LOCAL_BITS) - 1;

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

/// A key's place in an [`Interner`]: its shard and its entry there. Which
/// slot a key gets depends on the schedule: a slot is never observable.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Debug)]
pub struct Slot(pub u32);

impl Slot {
    fn new(shard: usize, local: usize) -> Self {
        Self(((shard as u32) << LOCAL_BITS) | local as u32)
    }
    fn shard(self) -> usize {
        (self.0 >> LOCAL_BITS) as usize
    }
    fn local(self) -> usize {
        (self.0 & ((1 << LOCAL_BITS) - 1)) as usize
    }
}

/// An interner shard grew beyond [`SHARD_CAPACITY`] entries, or a numbering
/// beyond 2^32 - 1 ids: the link is refused by name (§1).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TooManyKeys;

impl std::fmt::Display for TooManyKeys {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("more than 2^32 - 1 distinct keys (or 2^24 - 1 in one interner shard)")
    }
}

impl std::error::Error for TooManyKeys {}

#[derive(Clone, Copy, Debug)]
struct Entry<K> {
    key: K,
    hash: u64,
    first: u64,
}

#[derive(Debug)]
struct Shard<K> {
    /// 0, or the tag (the hash's upper half) and `local + 1`.
    cells: Vec<u64>,
    entries: Vec<Entry<K>>,
}

impl<K> Default for Shard<K> {
    fn default() -> Self {
        Self { cells: Vec::new(), entries: Vec::new() }
    }
}

#[inline]
fn cell(hash: u64, local: usize) -> u64 {
    (hash & 0xffff_ffff_0000_0000) | (local as u64 + 1)
}

impl<K: InternKey> Shard<K> {
    fn find(&self, key: &K, hash: u64) -> Result<usize, usize> {
        let mask = self.cells.len().wrapping_sub(1);
        let tag = hash >> 32;
        let mut i = hash as usize & mask;
        loop {
            let c = self.cells[i];
            if c == 0 {
                return Err(i);
            }
            if c >> 32 == tag {
                let local = (c as u32 - 1) as usize;
                if self.entries[local].key == *key {
                    return Ok(local);
                }
            }
            i = (i + 1) & mask;
        }
    }

    fn insert(&mut self, key: K, hash: u64, mention: u64) -> Result<usize, TooManyKeys> {
        if (self.entries.len() + 1) * 8 > self.cells.len() * 5 {
            self.grow();
        }
        match self.find(&key, hash) {
            Ok(local) => {
                let e = &mut self.entries[local];
                e.first = e.first.min(mention);
                Ok(local)
            }
            Err(at) => {
                let local = self.entries.len();
                if local >= SHARD_CAPACITY {
                    return Err(TooManyKeys);
                }
                self.entries.push(Entry { key, hash, first: mention });
                self.cells[at] = cell(hash, local);
                Ok(local)
            }
        }
    }

    fn get(&self, key: &K, hash: u64) -> Option<usize> {
        if self.cells.is_empty() {
            return None;
        }
        self.find(key, hash).ok()
    }

    fn grow(&mut self) {
        let size = (self.cells.len() * 2).max(16);
        self.cells.clear();
        self.cells.resize(size, 0);
        let mask = size - 1;
        for (local, e) in self.entries.iter().enumerate() {
            let mut i = e.hash as usize & mask;
            while self.cells[i] != 0 {
                i = (i + 1) & mask;
            }
            self.cells[i] = cell(e.hash, local);
        }
    }
}

/// A shard on cache lines of its own.
#[derive(Debug, Default)]
#[repr(align(128))]
struct Padded<T>(T);

#[inline]
fn shard_of(hash: u64) -> usize {
    (hash >> (64 - SHARD_BITS)) as usize
}

/// A concurrent, insert-only table from keys to [`Slot`]s, keeping each
/// key's first mention. Shared by reference among workers; frozen into a
/// read-only [`Frozen`] when the parallel phase ends.
///
/// Cost: one lock of one shard per insert; per distinct key one entry (48
/// bytes for [`SymName`]) and 1.6 to 4 probe cells of 8 bytes, with
/// vector growth on top (measured in `spec/landed/model/WP-020b.md`).
#[derive(Debug)]
pub struct Interner<K> {
    shards: Box<[Padded<Mutex<Shard<K>>>]>,
}

impl<K: InternKey> Default for Interner<K> {
    fn default() -> Self {
        Self::new()
    }
}

impl<K: InternKey> Interner<K> {
    /// An empty interner. Allocates the shard headers only.
    pub fn new() -> Self {
        Self { shards: (0..SHARDS).map(|_| Padded::default()).collect() }
    }

    /// Interns `key`, mentioned at `mention`: its slot, the first mention
    /// becoming the minimum of the old one and `mention`.
    pub fn insert(&self, key: K, mention: Mention) -> Result<Slot, TooManyKeys> {
        let hash = key.hash();
        let s = shard_of(hash);
        let local = self.shards[s].0.lock().unwrap_or_else(PoisonError::into_inner).insert(
            key,
            hash,
            mention.pack(),
        )?;
        Ok(Slot::new(s, local))
    }

    /// [`Interner::insert`] without locking, for a sequential caller.
    pub fn insert_mut(&mut self, key: K, mention: Mention) -> Result<Slot, TooManyKeys> {
        let hash = key.hash();
        let s = shard_of(hash);
        let shard = self.shards[s].0.get_mut().unwrap_or_else(PoisonError::into_inner);
        Ok(Slot::new(s, shard.insert(key, hash, mention.pack())?))
    }

    /// The table, read-only: lookups without locks, and a dense index of
    /// slots.
    pub fn freeze(self) -> Frozen<K> {
        let shards: Box<[Shard<K>]> = self
            .shards
            .into_vec()
            .into_iter()
            .map(|p| p.0.into_inner().unwrap_or_else(PoisonError::into_inner))
            .collect();
        let mut bases = Vec::with_capacity(SHARDS + 1);
        let mut total = 0;
        for shard in &shards {
            bases.push(total);
            total += shard.entries.len();
        }
        bases.push(total);
        Frozen { shards, bases: bases.into_boxed_slice() }
    }
}

/// A frozen [`Interner`]: read-only, shared freely.
#[derive(Debug)]
pub struct Frozen<K> {
    shards: Box<[Shard<K>]>,
    /// `SHARDS + 1` prefix sums of the shard sizes: the dense index of slot
    /// `(s, l)` is `bases[s] + l`.
    bases: Box<[usize]>,
}

impl<K: InternKey> Frozen<K> {
    /// The number of distinct keys.
    pub fn len(&self) -> usize {
        self.bases[SHARDS]
    }

    /// Whether no key was interned.
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// The slot of `key`, if it was interned.
    pub fn get(&self, key: &K) -> Option<Slot> {
        let hash = key.hash();
        let s = shard_of(hash);
        self.shards[s].get(key, hash).map(|l| Slot::new(s, l))
    }

    fn entry(&self, slot: Slot) -> &Entry<K> {
        &self.shards[slot.shard()].entries[slot.local()]
    }

    /// The key of `slot`.
    pub fn key(&self, slot: Slot) -> K {
        self.entry(slot).key
    }

    /// The hash of the key of `slot`.
    pub fn hash(&self, slot: Slot) -> u64 {
        self.entry(slot).hash
    }

    /// The first mention of the key of `slot`.
    pub fn first(&self, slot: Slot) -> Mention {
        Mention::unpack(self.entry(slot).first)
    }

    /// The slot's index in `0..len()`, dense (the order is not observable).
    #[inline]
    pub fn dense(&self, slot: Slot) -> usize {
        self.bases[slot.shard()] + slot.local()
    }

    /// The bytes the table holds: shards, entries and probe cells, by
    /// capacity.
    pub fn heap_bytes(&self) -> usize {
        let shard = size_of::<Shard<K>>();
        self.shards
            .iter()
            .map(|s| {
                shard + s.cells.capacity() * 8 + s.entries.capacity() * size_of::<Entry<K>>()
            })
            .sum::<usize>()
            + self.bases.len() * size_of::<usize>()
    }
}

/// The distinct keys of a [`number`]ing, in id order, with lookup by key.
#[derive(Debug)]
pub struct Names<K, I> {
    table: Frozen<K>,
    /// Dense slot index to id.
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
        self.table.get(key).map(|s| I::from_index(self.id_of[self.table.dense(s)]))
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
pub fn number<K, I, F, It>(pool: &Pool, counts: &[u32], keys: F) -> Result<Numbering<K, I>, TooManyKeys>
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
    let mut interner = Interner::new();
    if pool.is_inline() {
        for (u, out) in split_offsets(&mut map, &offsets).into_iter().enumerate() {
            fill(keys(u), u, out, |k, m| interner.insert_mut(k, m))?;
        }
    } else {
        let failed = AtomicBool::new(false);
        let shared = &interner;
        pool.for_each_part(split_offsets(&mut map, &offsets), |u, out| {
            if fill(keys(u), u, out, |k, m| shared.insert(k, m)).is_err() {
                failed.store(true, atomic::Ordering::Relaxed);
            }
        });
        if failed.into_inner() {
            return Err(TooManyKeys);
        }
    }
    let table = interner.freeze();
    if u32::try_from(table.len()).is_err() {
        return Err(TooManyKeys);
    }

    // Pass 2: the first mentions, in unit order, are the ids in order.
    let collected = pool.map_collect(counts.len(), |u, out| {
        for (n, s) in map[offsets[u]..offsets[u + 1]].iter().enumerate() {
            let slot = Slot(s.index());
            if table.first(slot) == (Mention { unit: u as u32, ordinal: n as u32 }) {
                out.push(slot);
            }
        }
    });
    let slot_of = collected.items;
    debug_assert_eq!(slot_of.len(), table.len(), "every key has one first mention");

    // Pass 3: ids by slot, then the map from slots to ids.
    let ids: Vec<AtomicU32> = (0..table.len()).map(|_| AtomicU32::new(0)).collect();
    pool.for_each_block(slot_of.len(), REWRITE_BLOCK, |_, range| {
        for id in range {
            ids[table.dense(slot_of[id])].store(id as u32, atomic::Ordering::Relaxed);
        }
    });
    pool.for_each_part(map.chunks_mut(REWRITE_BLOCK), |_, chunk| {
        for x in chunk {
            *x = I::from_index(ids[table.dense(Slot(x.index()))].load(atomic::Ordering::Relaxed));
        }
    });
    let id_of = ids.into_iter().map(AtomicU32::into_inner).collect();
    Ok(Numbering { map, offsets, names: Names { table, id_of, slot_of, ids: PhantomData } })
}

/// Pass 1 for unit `u`: interns its keys, leaving their slots in `out`.
fn fill<K, I, It, F>(keys: It, u: usize, out: &mut [I], mut intern: F) -> Result<(), TooManyKeys>
where
    I: Id,
    It: IntoIterator<Item = K>,
    F: FnMut(K, Mention) -> Result<Slot, TooManyKeys>,
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
