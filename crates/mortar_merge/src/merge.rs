//! The merge of pieces into fragments (`spec/synth.md` §3.3, §3.8): S-13
//! to S-15 by the partitioned method of S-28.
//!
//! 1. **cut**: per section in parallel, the piece offsets ([`cut::cut`])
//!    and one 64-bit hash per piece over its bytes, its output section,
//!    merge class and alignment;
//! 2. **partition**: the sections are cut into contiguous ranges; a
//!    counting sort by (shard, range) lays every occurrence out in one
//!    column, shard after shard (the shard: the hash's high bits), each
//!    element the occurrence and the low half of its hash. A shard's part
//!    of the column lists its occurrences in key order `(FileId,
//!    sh_index, piece)`, because the rows are in `SectionId` order and an
//!    occurrence's number is its position in the `Split` piece columns;
//! 3. **intern**: per shard, on one thread and with no lock, the
//!    occurrences in key order are looked up by `(hash, bytes, class)` in
//!    the shard's own open-addressing table (a slot: the hash's low half
//!    and the entry; an entry: its row, offset and length, so a candidate
//!    is compared without other lookups); a miss makes the occurrence
//!    the representative of a new entry, and it is the first occurrence
//!    (S-14) because the walk is in key order. Each column element is
//!    replaced by its entry, and a representative sets its bit in one bit
//!    set over all occurrences (an atomic `or`, `spec/model.md` §6 rule 2);
//! 4. **number**: a representative's `FragId` is the number of
//!    representatives before it in key order (S-15): the popcount of the
//!    bit set below it, from a prefix sum of the per-word popcounts. Each
//!    shard replaces its column's entries by their `FragId`s;
//! 5. **resolve**: per range, every occurrence in key order takes the next
//!    element of its (range, shard) bucket into the `Split` row, and the
//!    `frags` rows are collected from the bit set in occurrence order,
//!    which is `FragId` order.
//!
//! The ranges and the shard count depend on the thread count; nothing
//! observable does (`spec/model.md` §6 rules 5 and 6): hashes only
//! propose candidates, bytes decide, and every number is a function of
//! key order.

use std::sync::atomic::{AtomicU64, Ordering};

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
    /// The `Split` table (`spec/model.md` §3.5) of these merge rows and
    /// the `.eh_frame` rows (`EhFrames::split_rows`).
    #[must_use]
    pub fn into_split(self, eh: crate::eh::EhRows) -> mortar_core::Split {
        mortar_core::Split {
            merge_of: self.merge_of,
            piece_off: self.piece_off,
            piece_frag: self.piece_frag,
            frags: self.frags,
            eh_of: eh.0,
            eh_off: eh.1,
            eh_piece: eh.2,
        }
    }

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

/// A representative piece as its shard's table keeps it: its row and
/// bytes (offset and length in the row's section), so that comparing a
/// candidate reads the table, this, and the bytes.
#[derive(Debug, Clone, Copy)]
struct Rep {
    row: u32,
    off: u32,
    len: u32,
}

/// The bytes of a representative piece of a section holding `bytes`.
fn piece<'a>(bytes: &'a [u8], rep: &Rep) -> &'a [u8] {
    let off = rep.off as usize;
    &bytes[off..off + rep.len as usize]
}

/// One shard's table: open addressing over the entries' hashes, probed
/// linearly from the hash's low bits (the high bits chose the shard). A
/// slot holds the low 32 bits of its entry's hash (the probe start, and a
/// tag that rules out almost every other entry before its bytes are
/// read) and the entry + 1; 0 is empty.
struct Table {
    slots: Vec<u64>,
    reps: Vec<Rep>,
}

impl Table {
    /// A table for a shard of `occurrences` pieces: sized for one in
    /// sixteen being distinct (it grows past that), at least 64 slots.
    fn with_occurrences(occurrences: usize) -> Self {
        let slots = (occurrences / 8).next_power_of_two().max(64);
        Self { slots: vec![0; slots], reps: Vec::new() }
    }

    /// The entry with hash `h` that `same` accepts, or the empty slot to
    /// put a new one in.
    fn find(&self, h: u64, mut same: impl FnMut(&Rep) -> bool) -> Result<u32, usize> {
        let mask = self.slots.len() - 1;
        let tag = h & 0xffff_ffff;
        let mut at = h as usize & mask;
        loop {
            match self.slots[at] {
                0 => return Err(at),
                s => {
                    let e = (s & 0xffff_ffff) as u32 - 1;
                    if s >> 32 == tag && same(&self.reps[e as usize]) {
                        return Ok(e);
                    }
                }
            }
            at = (at + 1) & mask;
        }
    }

    /// Adds entry `reps.len()` at `slot` (from [`Table::find`]).
    fn insert(&mut self, slot: usize, h: u64, rep: Rep) -> u32 {
        let e = self.reps.len() as u32;
        self.reps.push(rep);
        self.slots[slot] = (h << 32) | u64::from(e + 1);
        if self.reps.len() * 2 > self.slots.len() {
            let mut slots = vec![0u64; self.slots.len() * 2];
            let mask = slots.len() - 1;
            for &s in &self.slots {
                if s == 0 {
                    continue;
                }
                let mut at = (s >> 32) as usize & mask;
                while slots[at] != 0 {
                    at = (at + 1) & mask;
                }
                slots[at] = s;
            }
            self.slots = slots;
        }
        e
    }
}

/// The row of occurrence `occ`, searching forward from row `from` (whose
/// first occurrence is at or before `occ`): `starts` is the rows' prefix
/// sum. A shard walks its occurrences in key order, so its row only moves
/// forward; a far move is a galloping search.
fn row_of(starts: &[usize], from: usize, occ: usize) -> usize {
    if occ < starts[from + 1] {
        return from;
    }
    let mut lo = from + 1;
    let mut step = 1;
    let last = starts.len() - 2;
    let mut hi = (lo + step).min(last);
    while hi < last && starts[hi + 1] <= occ {
        lo = hi;
        step *= 2;
        hi = (lo + step).min(last);
    }
    // The row is in lo..=hi: the last row starting at or before occ.
    lo + starts[lo + 1..=hi + 1].partition_point(|&s| s <= occ)
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
    let mut steps = mortar_core::wp032b_steps::Steps::new("merge");
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
    if steps.on() {
        let bytes: usize = sections.iter().map(|s| s.bytes.len()).sum();
        steps.step("cut+hash", bytes as u64, "byte");
    }
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
    steps.step("rows", total as u64, "piece");
    let bytes_of = |row: usize, occ: usize| -> &[u8] {
        let b = sections[rows_in[row]].bytes;
        let end = if occ + 1 < starts[row + 1] { piece_off[occ + 1] as usize } else { b.len() };
        &b[piece_off[occ] as usize..end]
    };

    // 2. partition: a counting sort of the occurrences by (shard, range)
    // into one column, shard-major, each shard's occurrences in key order.
    // An element is the occurrence and its hash's low half (all the
    // shard's table uses: the high bits chose the shard).
    let bits = shard_bits(pool.threads());
    let nshards = 1usize << bits;
    let shard = |h: u64| (h >> (64 - bits)) as usize;
    let nranges = nrows.min(pool.threads() * 8).max(1);
    let per = nrows.div_ceil(nranges).max(1);
    let range_rows = |c: usize| (c * per).min(nrows)..((c + 1) * per).min(nrows);
    // The hashes of range c's occurrences, in key order.
    let range_hashes = |c: usize| range_rows(c).flat_map(|r| hashes[r].iter().copied());
    let counts: Vec<Vec<u32>> = pool.map(nranges, |c| {
        let mut n = vec![0u32; nshards];
        for h in range_hashes(c) {
            n[shard(h)] += 1;
        }
        n
    });
    // at[c][s]: where bucket (c, s) starts in the column; base[s]: where
    // shard s does.
    let mut at = vec![vec![0usize; nshards]; nranges];
    let mut base = Vec::with_capacity(nshards + 1);
    let mut next = 0usize;
    for s in 0..nshards {
        base.push(next);
        for c in 0..nranges {
            at[c][s] = next;
            next += counts[c][s] as usize;
        }
    }
    base.push(next);
    drop(counts);
    let mut column = vec![0u64; total];
    {
        // The buckets in column order (shard, range), regrouped by range.
        let mut bounds: Vec<usize> = Vec::with_capacity(nshards * nranges + 1);
        for s in 0..nshards {
            for row in &at {
                bounds.push(row[s]);
            }
        }
        bounds.push(total);
        let mut by_range: Vec<Vec<&mut [u64]>> = (0..nranges).map(|_| Vec::new()).collect();
        for (k, part) in split_offsets(&mut column, &bounds).into_iter().enumerate() {
            by_range[k % nranges].push(part);
        }
        pool.for_each_part(by_range, |c, mut parts| {
            let mut fill = vec![0usize; nshards];
            let first = starts[range_rows(c).start];
            for (o, h) in (first..).zip(range_hashes(c)) {
                let s = shard(h);
                parts[s][fill[s]] = (o as u64) << 32 | (h & 0xffff_ffff);
                fill[s] += 1;
            }
        });
    }
    steps.step("partition", total as u64, "piece");

    // 3. intern: per shard, its occurrences in key order; each column
    // element is replaced by the occurrence's entry.
    let is_rep: Vec<AtomicU64> = (0..total.div_ceil(64)).map(|_| AtomicU64::new(0)).collect();
    let mut reps: Vec<Vec<u32>> = (0..nshards).map(|_| Vec::new()).collect();
    let parts = split_offsets(&mut column, &base).into_iter().zip(reps.iter_mut());
    pool.for_each_part(parts, |_, (elems, reps)| {
        let mut table = Table::with_occurrences(elems.len());
        let mut row = 0usize;
        for elem in elems.iter_mut() {
            let (o, h) = ((*elem >> 32) as usize, *elem & 0xffff_ffff);
            row = row_of(&starts, row, o);
            let bytes = bytes_of(row, o);
            let found = table.find(h, |rep| {
                rep.len as usize == bytes.len()
                    && meta[rep.row as usize] == meta[row]
                    && piece(sections[rows_in[rep.row as usize]].bytes, rep) == bytes
            });
            let e = match found {
                Ok(e) => e,
                Err(slot) => {
                    reps.push(o as u32);
                    is_rep[o / 64].fetch_or(1 << (o % 64), Ordering::Relaxed);
                    let rep = Rep { row: row as u32, off: piece_off[o], len: bytes.len() as u32 };
                    table.insert(slot, h, rep)
                }
            };
            *elem = u64::from(e);
        }
    });
    steps.step("intern", total as u64, "piece");

    // 4. number: each shard's entries become their fragments, in place.
    let words: Vec<u64> = is_rep.into_iter().map(AtomicU64::into_inner).collect();
    let mut below: Vec<u32> = words.iter().map(|w| w.count_ones()).collect();
    let nfrags = pool.unit_cost(1).prefix_sum(&mut below);
    let rank = |occ: usize| {
        let (w, b) = (occ / 64, occ % 64);
        below[w] + (words[w] & ((1u64 << b) - 1)).count_ones()
    };
    let parts = split_offsets(&mut column, &base).into_iter().zip(reps.iter_mut());
    pool.for_each_part(parts, |_, (elems, reps)| {
        let frag: Vec<u32> = reps.iter().map(|&occ| rank(occ as usize)).collect();
        *reps = Vec::new();
        for e in elems.iter_mut() {
            *e = u64::from(frag[*e as usize]);
        }
    });
    drop(reps);
    steps.step("number", total as u64, "piece");

    // 5. resolve: per range, the occurrences in key order, each taking the
    // next fragment of its bucket (range, shard).
    let mut piece_frag = vec![FragId(0); total];
    let range_starts: Vec<usize> = (0..=nranges).map(|c| starts[(c * per).min(nrows)]).collect();
    pool.for_each_part(split_offsets(&mut piece_frag, &range_starts), |c, out| {
        let mut cursor = at[c].clone();
        for (slot, h) in out.iter_mut().zip(range_hashes(c)) {
            let s = shard(h);
            *slot = FragId(column[cursor[s]] as u32);
            cursor[s] += 1;
        }
    });
    drop(column);
    drop(hashes);
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
    steps.step("resolve", u64::from(nfrags), "frag");
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
