//! The merge of pieces into fragments (`spec/synth.md` §3.3, §3.8): S-13
//! to S-15 by the partitioned method of S-28.
//!
//! 1. **cut**: per section in parallel, the piece offsets ([`cut::cut`])
//!    and one 64-bit hash per piece over its bytes, its output section,
//!    merge class and alignment;
//! 2. **partition**: the sections are cut into contiguous ranges; per
//!    range, each occurrence `(occurrence, row)` is appended to the bucket
//!    of its shard (the hash's high bits). A shard's buckets taken in range
//!    order list its occurrences in key order `(FileId, sh_index, piece)`,
//!    because the rows are in `SectionId` order and an occurrence's number
//!    is its position in the `Split` piece columns;
//! 3. **intern**: per shard, on one thread and with no lock, the
//!    occurrences in key order are looked up by `(hash, bytes, class)` in
//!    the shard's own open-addressing table; a miss makes the occurrence
//!    the representative of a new entry, and it is the first occurrence
//!    (S-14) because the walk is in key order. Each occurrence records its
//!    entry, and a representative sets its bit in one bit set over all
//!    occurrences (an atomic `or`, `spec/model.md` §6 rule 2);
//! 4. **number**: a representative's `FragId` is the number of
//!    representatives before it in key order (S-15): the popcount of the
//!    bit set below it, from a prefix sum of the per-word popcounts. Each
//!    shard numbers its own entries;
//! 5. **resolve**: every occurrence reads its entry's `FragId` into the
//!    `Split` row, and the `frags` rows are collected from the bit set in
//!    occurrence order, which is `FragId` order.
//!
//! The ranges and the shard count depend on the thread count; nothing
//! observable does (`spec/model.md` §6 rules 5 and 6): hashes only
//! propose candidates, bytes decide, and every number is a function of
//! key order.

use std::sync::atomic::{AtomicU32, AtomicU64, Ordering};

use mortar_core::hash::{combine, hash_bytes, hash_u64};
use mortar_core::par::{Pool, split_offsets};
use mortar_core::split::{Frag, SplitRow};
use mortar_core::{Column, FileId, FragId, OutSecId, SectionId, Span};

use crate::cut;
use crate::problem::{Problem, What};

/// One split `Merge` section, the input of [`merge`] (S-5, S-6 decided by
/// the caller: [`cut::placed_whole`]).
#[derive(Debug, Clone, Copy)]
pub struct MergeIn<'a> {
    pub section: SectionId,
    pub file: FileId,
    /// Its index in its file.
    pub index: u32,
    /// Its output section (pieces of different output sections never
    /// merge, S-13).
    pub out: OutSecId,
    /// `SHF_STRINGS`.
    pub strings: bool,
    /// `sh_entsize`, at most [`cut::MAX_ENTSIZE`].
    pub entsize: u32,
    /// log2 of `sh_addralign` (S-11: every piece has it).
    pub align: u8,
    pub bytes: &'a [u8],
}

/// What identifies a fragment besides its bytes (S-13): the output
/// section, the merge class and the alignment. One per `Split.merge_of`
/// row.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct MergeMeta {
    pub out: OutSecId,
    pub strings: bool,
    pub entsize: u32,
    /// log2 of the alignment.
    pub align: u8,
}

impl MergeMeta {
    fn of(s: &MergeIn<'_>) -> Self {
        Self { out: s.out, strings: s.strings, entsize: s.entsize, align: s.align }
    }

    /// The word hashed with a piece's bytes: every field, injectively
    /// (`entsize` ≤ 2^16, S-6).
    fn word(self) -> u64 {
        (u64::from(self.out.0) << 32)
            | (u64::from(self.entsize) << 8)
            | (u64::from(self.strings) << 7)
            | u64::from(self.align)
    }
}

/// The merge's result: the `Merge` half of `Split` (`spec/model.md` §3.5),
/// and the identity of each row's pieces.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Merged {
    /// The split sections with at least one piece, by id.
    pub merge_of: Box<[SplitRow]>,
    pub piece_off: Box<[u32]>,
    pub piece_frag: Box<[FragId]>,
    pub frags: Column<FragId, Frag>,
    /// Parallel to `merge_of`.
    pub meta: Box<[MergeMeta]>,
}

impl Merged {
    /// The identity of a fragment's pieces (through its representative's
    /// row).
    #[must_use]
    pub fn meta_of(&self, frag: FragId) -> Option<MergeMeta> {
        let section = self.frags.get(frag)?.section;
        let at = self.merge_of.binary_search_by_key(&section, |r| r.section).ok()?;
        Some(self.meta[at])
    }
}

/// The shard count for `threads` threads: a power of two, at least 8 per
/// thread (S-28), within [16, 1024]. Not observable.
fn shard_bits(threads: usize) -> u32 {
    (threads.saturating_mul(8)).clamp(16, 1024).next_power_of_two().trailing_zeros()
}

/// One shard's table: open addressing over the entries' hashes, probed
/// linearly from the hash's low bits (the high bits chose the shard).
struct Table {
    /// Entry + 1; 0 is empty.
    slots: Vec<u32>,
    hashes: Vec<u64>,
}

impl Table {
    fn new() -> Self {
        Self { slots: vec![0; 64], hashes: Vec::new() }
    }

    /// The entry with hash `h` that `same` accepts, or the empty slot to
    /// put a new one in.
    fn find(&self, h: u64, mut same: impl FnMut(u32) -> bool) -> Result<u32, usize> {
        let mask = self.slots.len() - 1;
        let mut at = h as usize & mask;
        loop {
            match self.slots[at] {
                0 => return Err(at),
                s => {
                    let e = s - 1;
                    if self.hashes[e as usize] == h && same(e) {
                        return Ok(e);
                    }
                }
            }
            at = (at + 1) & mask;
        }
    }

    /// Adds entry `hashes.len()` at `slot` (from [`Table::find`]).
    fn insert(&mut self, slot: usize, h: u64) -> u32 {
        let e = self.hashes.len() as u32;
        self.hashes.push(h);
        self.slots[slot] = e + 1;
        if self.hashes.len() * 2 > self.slots.len() {
            let mut slots = vec![0u32; self.slots.len() * 2];
            let mask = slots.len() - 1;
            for (i, &h) in self.hashes.iter().enumerate() {
                let mut at = h as usize & mask;
                while slots[at] != 0 {
                    at = (at + 1) & mask;
                }
                slots[at] = i as u32 + 1;
            }
            self.slots = slots;
        }
        e
    }
}

/// A section's piece offsets and piece hashes (step 1).
type Cut = (Vec<u32>, Vec<u64>);

/// S-13 to S-15, S-28: splits `sections` (sorted by `SectionId`, each
/// once) into pieces and merges equal pieces into fragments numbered by
/// first occurrence. The problems of S-7, S-8 and the size limits are
/// returned sorted, and then nothing else is.
#[allow(clippy::too_many_lines)] // the five steps of S-28, in order
pub fn merge(pool: &Pool, sections: &[MergeIn<'_>]) -> Result<Merged, Vec<Problem>> {
    debug_assert!(
        sections.windows(2).all(|w| w[0].section < w[1].section),
        "merge: sections not in SectionId order"
    );
    // 1. cut and hash.
    let cuts: Vec<Result<Cut, Problem>> = pool.map(sections.len(), |i| {
        let s = &sections[i];
        let mut offs = Vec::new();
        cut::cut(s.bytes, s.strings, s.entsize as usize, &mut offs).map_err(|(what, offset)| {
            Problem { file: s.file, section: s.index, piece: 0, offset, what }
        })?;
        let meta = hash_u64(MergeMeta::of(s).word());
        let hashes = offs
            .iter()
            .enumerate()
            .map(|(j, &o)| {
                let end = offs.get(j + 1).map_or(s.bytes.len(), |&e| e as usize);
                combine(hash_bytes(&s.bytes[o as usize..end]), meta)
            })
            .collect();
        Ok((offs, hashes))
    });
    let mut problems: Vec<Problem> =
        cuts.iter().filter_map(|c| c.as_ref().err().cloned()).collect();
    if !problems.is_empty() {
        problems.sort();
        return Err(problems);
    }
    let mut rows_in: Vec<usize> = Vec::new(); // input index of each row
    let mut offs: Vec<Vec<u32>> = Vec::new();
    let mut hashes: Vec<Vec<u64>> = Vec::new();
    for (i, c) in cuts.into_iter().enumerate() {
        let Ok((o, h)) = c else { unreachable!("problems returned above") };
        if !o.is_empty() {
            rows_in.push(i);
            offs.push(o);
            hashes.push(h);
        }
    }
    let mut starts: Vec<usize> = Vec::with_capacity(offs.len() + 1);
    let mut total = 0usize;
    for o in &offs {
        starts.push(total);
        total += o.len();
    }
    starts.push(total);
    if u32::try_from(total).is_err() {
        let s = &sections[rows_in[rows_in.len() - 1]];
        return Err(vec![Problem {
            file: s.file,
            section: s.index,
            piece: 0,
            offset: 0,
            what: What::TooManyPiecesInLink,
        }]);
    }
    let nrows = offs.len();
    let meta: Vec<MergeMeta> = rows_in.iter().map(|&i| MergeMeta::of(&sections[i])).collect();
    let merge_of: Box<[SplitRow]> = (0..nrows)
        .map(|r| SplitRow {
            section: sections[rows_in[r]].section,
            pieces: Span { start: starts[r] as u32, len: offs[r].len() as u32 },
        })
        .collect();
    let mut piece_off = vec![0u32; total];
    pool.for_each_part(split_offsets(&mut piece_off, &starts), |r, out| {
        out.copy_from_slice(&offs[r]);
    });
    drop(offs);
    let bytes_of = |row: usize, occ: usize| -> &[u8] {
        let b = sections[rows_in[row]].bytes;
        let end = if occ + 1 < starts[row + 1] { piece_off[occ + 1] as usize } else { b.len() };
        &b[piece_off[occ] as usize..end]
    };

    // 2. partition.
    let bits = shard_bits(pool.threads());
    let nshards = 1usize << bits;
    let shard = |h: u64| (h >> (64 - bits)) as usize;
    let nranges = nrows.min(pool.threads() * 8).max(1);
    let per = nrows.div_ceil(nranges).max(1);
    let buckets: Vec<Vec<Vec<(u32, u32)>>> = pool.map(nranges, |c| {
        let mut b: Vec<Vec<(u32, u32)>> = (0..nshards).map(|_| Vec::new()).collect();
        for row in c * per..((c + 1) * per).min(nrows) {
            for (j, &h) in hashes[row].iter().enumerate() {
                b[shard(h)].push(((starts[row] + j) as u32, row as u32));
            }
        }
        b
    });

    // 3. intern.
    let entry_of: Vec<AtomicU32> = (0..total).map(|_| AtomicU32::new(0)).collect();
    let is_rep: Vec<AtomicU64> = (0..total.div_ceil(64)).map(|_| AtomicU64::new(0)).collect();
    let reps: Vec<Vec<u32>> = pool.map(nshards, |s| {
        let mut table = Table::new();
        let mut reps: Vec<(u32, u32)> = Vec::new();
        for range in &buckets {
            for &(occ, row) in &range[s] {
                let (o, r) = (occ as usize, row as usize);
                let h = hashes[r][o - starts[r]];
                let bytes = bytes_of(r, o);
                let found = table.find(h, |e| {
                    let (o2, r2) = reps[e as usize];
                    meta[r2 as usize] == meta[r] && bytes_of(r2 as usize, o2 as usize) == bytes
                });
                let e = match found {
                    Ok(e) => e,
                    Err(slot) => {
                        reps.push((occ, row));
                        is_rep[o / 64].fetch_or(1 << (o % 64), Ordering::Relaxed);
                        table.insert(slot, h)
                    }
                };
                entry_of[o].store(e, Ordering::Relaxed);
            }
        }
        reps.into_iter().map(|(occ, _)| occ).collect()
    });
    drop(buckets);

    // 4. number.
    let words: Vec<u64> = is_rep.into_iter().map(AtomicU64::into_inner).collect();
    let mut below: Vec<u32> = words.iter().map(|w| w.count_ones()).collect();
    let nfrags = pool.prefix_sum(&mut below);
    let rank = |occ: usize| {
        let (w, b) = (occ / 64, occ % 64);
        FragId(below[w] + (words[w] & ((1u64 << b) - 1)).count_ones())
    };
    let shard_frag: Vec<Vec<FragId>> =
        pool.map(nshards, |s| reps[s].iter().map(|&occ| rank(occ as usize)).collect());
    drop(reps);

    // 5. resolve.
    let mut piece_frag = vec![FragId(0); total];
    pool.for_each_part(split_offsets(&mut piece_frag, &starts), |r, out| {
        for (j, slot) in out.iter_mut().enumerate() {
            let e = entry_of[starts[r] + j].load(Ordering::Relaxed);
            *slot = shard_frag[shard(hashes[r][j])][e as usize];
        }
    });
    let frags = pool.map_collect(nrows, |r, out| {
        let s = &sections[rows_in[r]];
        for occ in starts[r]..starts[r + 1] {
            if words[occ / 64] >> (occ % 64) & 1 == 1 {
                out.push(Frag {
                    section: s.section,
                    offset: piece_off[occ],
                    size: bytes_of(r, occ).len() as u32,
                    align: s.align,
                });
            }
        }
    });
    debug_assert_eq!(frags.items.len(), nfrags as usize, "one row per representative");
    let frags = Column::from_vec(frags.items)
        .unwrap_or_else(|_| unreachable!("fragments are at most the pieces, which fit u32"));
    Ok(Merged {
        merge_of,
        piece_off: piece_off.into_boxed_slice(),
        piece_frag: piece_frag.into_boxed_slice(),
        frags,
        meta: meta.into_boxed_slice(),
    })
}
