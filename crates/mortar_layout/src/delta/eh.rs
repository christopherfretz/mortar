//! IN-72 item 5: `.eh_frame` (`spec/incr.md` IN-34a, WP-239 S4-3 and R-3;
//! WP-243). The update's per-file ranges of the `.eh_frame` chunk from
//! the stored ones (`LayoutRows::eh_range_of`) and the new records of the
//! touched files (`LayoutInput::eh`).
//!
//! In place: every touched file with records now had a range and its
//! records fit its capacity. Its range keeps its offset and capacity, its
//! records are laid from the range's start, and the rest of the range is
//! an `EhPad` hole (4, 8 or 12 bytes: `DW_CFA_nop` padding of the last
//! record, as the full link's `EhPlan::from_records_padded` takes a short
//! pad; 16 or more: one padding CIE). A touched file with no record now
//! leaves its whole range a hole. Adjacent holes are merged into one (two
//! pads at one boundary are not a full link's).
//!
//! Whole: anything else (a range that does not fit, a file with records
//! and no range: appeared, newly extracted, drawn in with its first
//! records; a hole the in-place rule cannot make). Every file with records
//! is laid from offset 0 in full-link order (S-46: by CURRENT rank, the
//! internal file last; never by `FILES` row number), each with capacity
//! `size + Slack::eh_pad(size)`, then the terminator. An unchanged file's
//! size is its range's bytes before its trailing hole (the bytes U-6 walks
//! and moves). The whole rewrite reads every `FILES` row with a range,
//! which `LayoutRows` does not give (WP-243's finding F-1): `layout`
//! passes no enumeration and the whole rewrite is `update-not-implemented
//! (layout)` until it does.
//!
//! What `EhLayout::ranges` tells U-6: in place, the rewritten ranges only
//! (each with its record offsets); whole, every range, a rewritten one
//! with its record offsets and an unchanged one with NONE (its records are
//! moved from the previous output's bytes: walked from its stored offset,
//! the stored record count, Δ = new offset − stored offset).

use std::collections::{BTreeMap, BTreeSet};

use mortar_core::column::Column;
use mortar_core::id::{ChunkId, FileId};
use mortar_core::incr::{
    EhFile, EhLayout, EhRange, EhRecordIn, IncrReason, StoredEhRange, StoredLayout,
};
use mortar_core::layout::{Chunk, ChunkKind, Hole, HoleKind};

use super::{corrupt, not_implemented};
use crate::slack::Slack;

/// The `.eh_frame` chunk's name.
const EH_FRAME: &[u8] = b".eh_frame";
/// The `.eh_frame_hdr` chunk's name.
const EH_FRAME_HDR: &[u8] = b".eh_frame_hdr";
/// The terminator (S-48): the last 4 bytes of the chunk.
const TERMINATOR: u64 = 4;
/// The smallest padding CIE (IN-34a).
const CIE_MIN: u64 = 16;

/// Every `FILES` row with an `.eh_frame` range: `(file, stored rank,
/// range)`. The whole rewrite's one extra read (WP-243 F-1).
pub(super) type AllRanges<'a> =
    &'a dyn Fn() -> Result<Vec<(FileId, u64, StoredEhRange)>, IncrReason>;

/// Item 5's result.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct EhOut {
    pub(super) layout: EhLayout,
    /// The chunk's size after the update (the terminator's end).
    pub(super) size: u64,
    /// Every hole of the chunk after the update, by offset.
    pub(super) holes: Vec<Hole>,
    /// The holes the update made (not stored before), by offset.
    pub(super) made: Vec<Hole>,
}

/// A touched file with records now or a range before.
struct Now<'a> {
    file: FileId,
    rank: u64,
    records: &'a [EhRecordIn],
    /// Its records' bytes.
    size: u64,
    old: Option<StoredEhRange>,
}

/// Item 5 over the files of the update (`files`: those of `changes`; the
/// files of `eh` are added). `None` when no touched file has records now
/// or had a range. `all` gives the whole rewrite's rows; without it a
/// whole rewrite is `update-not-implemented (layout)`.
///
/// Reads: `eh_range_of` once per touched file; `holes_in` of the
/// `.eh_frame` chunk once when some touched file has records or a range;
/// `all` once when the section is rewritten whole.
pub(super) fn item_5(
    stored: &StoredLayout<'_>,
    eh: &[EhFile],
    files: &[FileId],
    chunks: &Column<ChunkId, Chunk>,
    all: Option<AllRanges<'_>>,
) -> Result<Option<EhOut>, IncrReason> {
    let mut given: BTreeMap<FileId, &EhFile> = BTreeMap::new();
    for f in eh {
        if given.insert(f.file, f).is_some() {
            debug_assert!(false, "LayoutInput::eh names file {} twice", f.file.0);
            return Err(not_implemented());
        }
    }
    let touched: BTreeSet<FileId> = files.iter().copied().chain(given.keys().copied()).collect();
    let mut now = Vec::new();
    for file in touched {
        let old = stored.eh_range_of(file)?;
        let new = given.get(&file);
        // A file of the update with a range and no `EhFile`: its new
        // records are not known (the join gives every such file one).
        if old.is_some() && new.is_none() {
            return Err(not_implemented());
        }
        let Some(new) = new else { continue };
        let records = new.records.as_slice();
        if records.is_empty() && old.is_none() {
            continue;
        }
        // Records are whole words (S-47), and a file's range starts with
        // a CIE (its FDEs point back into it, S-44 per file).
        if records.iter().any(|r| r.size == 0 || r.size % 4 != 0)
            || records.first().is_some_and(|r| !r.cie)
        {
            return Err(not_implemented());
        }
        let size = records.iter().try_fold(0u64, |s, r| s.checked_add(r.size));
        let Some(size) = size else { return Err(not_implemented()) };
        now.push(Now { file, rank: new.rank, records, size, old });
    }
    if now.is_empty() {
        return Ok(None);
    }
    let (chunk, c) = match find(chunks, EH_FRAME)? {
        Some(x) => x,
        None if now.iter().any(|n| n.old.is_some()) => {
            return Err(corrupt("an .eh_frame range and no .eh_frame chunk"));
        }
        None => return Err(IncrReason::NewOutputSection(".eh_frame".into())),
    };
    // `.eh_frame_hdr`'s size follows the FDE count, which no stored row
    // gives (WP-243 F-4).
    if find(chunks, EH_FRAME_HDR)?.is_some() {
        return Err(not_implemented());
    }
    if c.size < TERMINATOR || c.size % 4 != 0 || c.capacity < c.size {
        return Err(corrupt(".eh_frame chunk size"));
    }
    let end = c.size - TERMINATOR;
    for n in &now {
        if let Some(o) = n.old {
            check_range(n.file, o, end)?;
        }
    }
    let stored_holes = stored.holes_in(chunk)?;
    check_holes(&stored_holes, end)?;

    let fits =
        now.iter().all(|n| n.size == 0 || n.old.is_some_and(|o| n.size <= u64::from(o.capacity)));
    if fits && let Some(out) = in_place(chunk, c.size, &now, &stored_holes)? {
        return Ok(Some(out));
    }
    let Some(all) = all else { return Err(not_implemented()) };
    whole(chunk, c, &now, &stored_holes, &all()?).map(Some)
}

/// The one chunk named `name`, if any.
fn find<'c>(
    chunks: &'c Column<ChunkId, Chunk>,
    name: &[u8],
) -> Result<Option<(ChunkId, &'c Chunk)>, IncrReason> {
    let mut it =
        chunks.iter().filter(|(_, c)| matches!(c.kind, ChunkKind::Synthetic(s) if s.name == name));
    let first = it.next();
    if it.next().is_some() {
        return Err(corrupt(format!("two {} chunks", String::from_utf8_lossy(name))));
    }
    Ok(first)
}

/// A stored range: whole words, records in it, before the terminator.
fn check_range(file: FileId, r: StoredEhRange, end: u64) -> Result<(), IncrReason> {
    let (o, cap) = (u64::from(r.offset), u64::from(r.capacity));
    if o % 4 != 0 || cap % 4 != 0 || cap == 0 || r.records == 0 || o + cap > end {
        return Err(corrupt(format!("file {}'s .eh_frame range", file.0)));
    }
    Ok(())
}

/// Stored holes: padding, whole words, disjoint, before the terminator.
fn check_holes(holes: &[Hole], end: u64) -> Result<(), IncrReason> {
    let mut at = 0;
    for h in holes {
        if h.kind != HoleKind::EhPad
            || h.size == 0
            || h.offset % 4 != 0
            || h.size % 4 != 0
            || h.offset < at
            || h.offset + h.size > end
        {
            return Err(corrupt(".eh_frame holes"));
        }
        at = h.offset + h.size;
    }
    Ok(())
}

/// The in-place rule, or `None` when it would leave a hole no full link
/// makes (a short hole with no record before it).
fn in_place(
    chunk: ChunkId,
    size: u64,
    now: &[Now<'_>],
    stored: &[Hole],
) -> Result<Option<EhOut>, IncrReason> {
    let cut: Vec<(u64, u64)> = now
        .iter()
        .filter_map(|n| n.old)
        .map(|o| (u64::from(o.offset), u64::from(o.offset) + u64::from(o.capacity)))
        .collect();
    let mut pieces: Vec<(u64, u64)> = Vec::new();
    for h in stored {
        let mut parts = vec![(h.offset, h.offset + h.size)];
        for &(s, e) in &cut {
            parts = parts
                .into_iter()
                .flat_map(|(a, b)| {
                    let mut v = Vec::new();
                    if a < s.min(b) {
                        v.push((a, s.min(b)));
                    }
                    if e.max(a) < b {
                        v.push((e.max(a), b));
                    }
                    v
                })
                .collect();
        }
        pieces.extend(parts);
    }
    let mut ranges = Vec::new();
    for n in now {
        let Some(o) = n.old else { continue };
        let (off, cap) = (u64::from(o.offset), u64::from(o.capacity));
        if n.size < cap {
            pieces.push((off + n.size, off + cap));
        }
        if n.size > 0 {
            ranges.push(range(n.file, off, cap, n.records));
        }
    }
    let holes = merge(chunk, pieces);
    if holes.iter().any(|h| h.size < CIE_MIN && h.offset == 0) {
        return Ok(None);
    }
    if holes.first().is_some_and(|h| h.offset == 0 && h.size == size - TERMINATOR) {
        return Err(no_record_left());
    }
    ranges.sort_by_key(|r| r.offset);
    let made = holes.iter().filter(|h| !stored.contains(h)).copied().collect();
    Ok(Some(EhOut { layout: EhLayout { chunk, whole: false, ranges }, size, holes, made }))
}

/// A range of the whole rewrite: (the internal file, rank, file, size,
/// the new records or `None`: moved). Sorted, the first three are S-46's
/// order.
type Laid<'a> = (bool, u64, FileId, u64, Option<&'a [EhRecordIn]>);

/// The whole rewrite in full-link order.
fn whole(
    chunk: ChunkId,
    c: &Chunk,
    now: &[Now<'_>],
    stored: &[Hole],
    rows: &[(FileId, u64, StoredEhRange)],
) -> Result<EhOut, IncrReason> {
    let end = c.size - TERMINATOR;
    let mine: BTreeMap<FileId, &Now<'_>> = now.iter().map(|n| (n.file, n)).collect();
    let mut list: Vec<Laid<'_>> = Vec::new();
    let mut seen = BTreeSet::new();
    for &(file, rank, r) in rows {
        if !seen.insert(file) {
            return Err(corrupt(format!("file {} has two .eh_frame ranges", file.0)));
        }
        if let Some(n) = mine.get(&file) {
            if n.old != Some(r) {
                return Err(corrupt(format!(
                    "file {}'s .eh_frame range read twice differs",
                    file.0
                )));
            }
            continue;
        }
        check_range(file, r, end)?;
        let size = bytes_of(file, r, stored)?;
        list.push((file == FileId::INTERNAL, rank, file, size, None));
    }
    for n in now {
        if n.old.is_some() && !seen.contains(&n.file) {
            return Err(corrupt(format!("file {}'s .eh_frame range is not listed", n.file.0)));
        }
        if n.size > 0 {
            list.push((n.file == FileId::INTERNAL, n.rank, n.file, n.size, Some(n.records)));
        }
    }
    if list.is_empty() {
        return Err(no_record_left());
    }
    list.sort_by_key(|e| (e.0, e.1, e.2));
    let mut at = 0u64;
    let mut ranges = Vec::with_capacity(list.len());
    let mut holes = Vec::new();
    for (_, _, file, size, records) in list {
        let cap = size + Slack::INCREMENTAL.eh_pad(size);
        holes.push(Hole { chunk, offset: at + size, size: cap - size, kind: HoleKind::EhPad });
        ranges.push(match records {
            Some(r) => range(file, at, cap, r),
            None => EhRange { file, offset: at, capacity: cap, records: Vec::new() },
        });
        at = at.checked_add(cap).ok_or_else(|| exhausted(u64::MAX, c.capacity))?;
    }
    let size = at.checked_add(TERMINATOR).ok_or_else(|| exhausted(u64::MAX, c.capacity))?;
    if size > c.capacity {
        return Err(exhausted(size, c.capacity));
    }
    let made = holes.iter().filter(|h| !stored.contains(h)).copied().collect();
    Ok(EhOut { layout: EhLayout { chunk, whole: true, ranges }, size, holes, made })
}

/// An unchanged range's bytes: its capacity less the hole that ends it
/// (the only hole a range holds is its tail, which may run on into the
/// next range's place when that file left).
fn bytes_of(file: FileId, r: StoredEhRange, holes: &[Hole]) -> Result<u64, IncrReason> {
    let (s, e) = (u64::from(r.offset), u64::from(r.offset) + u64::from(r.capacity));
    let mut inside = holes.iter().filter(|h| h.offset < e && h.offset + h.size > s);
    let Some(h) = inside.next() else { return Ok(e - s) };
    if inside.next().is_some() || h.offset <= s || h.offset + h.size < e {
        return Err(corrupt(format!(
            "file {}'s .eh_frame range holds a hole not at its end",
            file.0
        )));
    }
    Ok(h.offset - s)
}

/// A rewritten range: its records laid from its start.
fn range(file: FileId, offset: u64, capacity: u64, records: &[EhRecordIn]) -> EhRange {
    let mut at = offset;
    let records = records
        .iter()
        .map(|r| {
            let o = at;
            at += r.size;
            o
        })
        .collect();
    EhRange { file, offset, capacity, records }
}

/// `pieces` sorted and joined where they touch.
fn merge(chunk: ChunkId, mut pieces: Vec<(u64, u64)>) -> Vec<Hole> {
    pieces.retain(|p| p.1 > p.0);
    pieces.sort_unstable();
    let mut out: Vec<Hole> = Vec::new();
    for (a, b) in pieces {
        match out.last_mut() {
            Some(h) if h.offset + h.size >= a => h.size = h.size.max(b - h.offset),
            _ => out.push(Hole { chunk, offset: a, size: b - a, kind: HoleKind::EhPad }),
        }
    }
    out
}

/// The last record gone: a full link writes the terminator alone or no
/// `.eh_frame` at all (S-48a), and no `.eh_frame_hdr` (WP-243 F-6).
fn no_record_left() -> IncrReason {
    IncrReason::NewOutputSection(".eh_frame left with no record".into())
}

fn exhausted(need: u64, holds: u64) -> IncrReason {
    IncrReason::ReserveExhausted(format!(
        ".eh_frame rewritten whole needs {need} bytes, its chunk holds {holds}"
    ))
}

#[cfg(test)]
mod tests {
    //! The exactness series (WP-243): every hop's delta applied to the
    //! stored rows (as U-2 applies it) against a from-scratch model of "a
    //! full link with the update's placement" (IN-78): every file with
    //! records has one range, the ranges in full-link order (rank, the
    //! internal file last), each range's records from its start, every
    //! other byte before the terminator a hole, holes merged and none
    //! short without a record before it; a whole rewrite equal to the
    //! model's fresh layout with the capacity rule written out again here.
    use super::*;
    use mortar_core::id::SectionId;
    use mortar_core::incr::{MemoryLayout, Unimplemented};
    use mortar_core::layout::Synth;

    const EH: ChunkId = ChunkId(0);

    /// The rule, written out independently of `Slack::eh_pad`.
    fn cap_of(n: u64) -> u64 {
        n + 16u64.max(n.div_ceil(8).div_ceil(4) * 4)
    }

    fn synth(name: &'static [u8], size: u64) -> Chunk {
        Chunk {
            kind: ChunkKind::Synthetic(Synth { name, address_dependent: false }),
            sh_type: 1,
            flags: 2,
            align: 3,
            size,
            capacity: size + size / 8,
            addr: Some(0x40_0000),
            offset: 0x1000,
            shndx: Some(1),
            relro: false,
        }
    }

    /// A file's records now: sizes, the first a CIE.
    #[derive(Debug, Clone)]
    struct F {
        file: FileId,
        rank: u64,
        sizes: Vec<u64>,
    }

    fn f(file: u32, rank: u64, sizes: &[u64]) -> F {
        F { file: FileId(file), rank, sizes: sizes.to_vec() }
    }

    /// The stored link, and what the model knows besides: each file's
    /// rank and the bytes its range holds (its records, a short pad
    /// absorbed into the last one included).
    #[derive(Debug, Clone)]
    struct State {
        rows: MemoryLayout,
        ranks: BTreeMap<FileId, u64>,
        bytes: BTreeMap<FileId, u64>,
    }

    fn order_key(file: FileId, rank: u64) -> (bool, u64, FileId) {
        (file == FileId::INTERNAL, rank, file)
    }

    /// The model's fresh layout of `(file, rank, bytes)`: ranges, holes,
    /// chunk size.
    fn fresh(mut v: Vec<(FileId, u64, u64)>) -> (Vec<(FileId, u64, u64)>, Vec<Hole>, u64) {
        v.retain(|e| e.2 > 0);
        v.sort_by_key(|e| order_key(e.0, e.1));
        let (mut at, mut ranges, mut holes) = (0, Vec::new(), Vec::new());
        for (file, _, n) in v {
            let cap = cap_of(n);
            ranges.push((file, at, cap));
            holes.push(Hole { chunk: EH, offset: at + n, size: cap - n, kind: HoleKind::EhPad });
            at += cap;
        }
        (ranges, holes, at + 4)
    }

    /// The capture: an incremental-mode full link of `files`, the chunk
    /// with `extra` bytes of capacity past the packed slack.
    fn first(files: &[F], extra: u64) -> State {
        let v: Vec<_> = files.iter().map(|f| (f.file, f.rank, f.sizes.iter().sum())).collect();
        let (ranges, holes, size) = fresh(v);
        let mut c = synth(EH_FRAME, size);
        c.capacity += extra;
        let mut rows = MemoryLayout { chunks: vec![c], order: vec![EH], ..MemoryLayout::default() };
        rows.holes = holes;
        let by: BTreeMap<FileId, &F> = files.iter().map(|f| (f.file, f)).collect();
        rows.eh_ranges = ranges
            .iter()
            .map(|&(file, offset, cap)| {
                let n = by[&file].sizes.len() as u32;
                (file, StoredEhRange { offset: offset as u32, capacity: cap as u32, records: n })
            })
            .collect();
        State {
            rows,
            ranks: files.iter().map(|f| (f.file, f.rank)).collect(),
            bytes: files
                .iter()
                .map(|f| (f.file, f.sizes.iter().sum()))
                .filter(|e| e.1 > 0)
                .collect(),
        }
    }

    fn eh_files(touched: &[F]) -> Vec<EhFile> {
        let mut v: Vec<EhFile> = touched
            .iter()
            .map(|t| EhFile {
                file: t.file,
                rank: t.rank,
                records: t
                    .sizes
                    .iter()
                    .enumerate()
                    .map(|(i, &size)| EhRecordIn {
                        section: SectionId(t.file.0 << 8),
                        piece: i as u32,
                        size,
                        cie: i == 0,
                    })
                    .collect(),
            })
            .collect();
        v.sort_by_key(|e| e.file);
        v
    }

    fn run(s: &State, touched: &[F], enumerate: bool) -> Result<Option<EhOut>, IncrReason> {
        let stored = StoredLayout::new(&s.rows);
        let eh = eh_files(touched);
        let files: Vec<FileId> = eh.iter().map(|e| e.file).collect();
        let chunks = Column::from_vec(s.rows.chunks.clone()).unwrap();
        let rows =
            || Ok(s.rows.eh_ranges.iter().map(|&(f, r)| (f, s.ranks[&f], r)).collect::<Vec<_>>());
        item_5(&stored, &eh, &files, &chunks, enumerate.then_some(&rows as AllRanges<'_>))
    }

    /// One hop: the delta, applied as U-2 applies it, checked against
    /// the model. Returns the next state and whether it was whole.
    fn hop(s: &State, touched: &[F]) -> (State, bool) {
        let out = run(s, touched, true).unwrap().expect("a touched .eh_frame");
        let mut next = s.clone();
        let old: BTreeMap<FileId, StoredEhRange> = s.rows.eh_ranges.iter().copied().collect();
        let by: BTreeMap<FileId, &F> = touched.iter().map(|t| (t.file, t)).collect();
        for t in touched {
            next.ranks.insert(t.file, t.rank);
            let n: u64 = t.sizes.iter().sum();
            if n == 0 {
                next.bytes.remove(&t.file);
            } else {
                next.bytes.insert(t.file, n);
            }
        }
        if out.layout.whole {
            next.rows.eh_ranges = Vec::new();
        } else {
            next.rows.eh_ranges.retain(|(f, _)| !by.contains_key(f));
        }
        for r in &out.layout.ranges {
            let records =
                if r.records.is_empty() { old[&r.file].records } else { r.records.len() as u32 };
            let row = StoredEhRange {
                offset: u32::try_from(r.offset).unwrap(),
                capacity: u32::try_from(r.capacity).unwrap(),
                records,
            };
            next.rows.eh_ranges.push((r.file, row));
        }
        next.rows.eh_ranges.sort_by_key(|e| e.0);
        next.rows.holes = out.holes.clone();
        next.rows.chunks[0].size = out.size;
        exact(&next, &out, &by);
        (next, out.layout.whole)
    }

    /// The model's check of a hop's result.
    fn exact(s: &State, out: &EhOut, touched: &BTreeMap<FileId, &F>) {
        let c = s.rows.chunks[0];
        assert_eq!(c.size, out.size);
        assert!(c.size <= c.capacity);
        // Every file with records has one range, no other.
        let with: BTreeSet<FileId> = s.bytes.keys().copied().collect();
        let rows: BTreeSet<FileId> = s.rows.eh_ranges.iter().map(|e| e.0).collect();
        assert_eq!(with, rows, "a range per file with records");
        // Full-link order.
        let mut by_off = s.rows.eh_ranges.clone();
        by_off.sort_by_key(|e| e.1.offset);
        let keys: Vec<_> = by_off.iter().map(|e| order_key(e.0, s.ranks[&e.0])).collect();
        assert!(keys.windows(2).all(|w| w[0] < w[1]), "ranges in full-link order: {by_off:?}");
        // Every byte before the terminator: a record or a hole.
        let mut pieces = Vec::new();
        let mut at = 0;
        for (file, r) in &by_off {
            let (o, cap) = (u64::from(r.offset), u64::from(r.capacity));
            assert!(o >= at, "ranges overlap");
            pieces.push((at, o));
            let n = s.bytes[file];
            assert!(n <= cap, "records fit");
            pieces.push((o + n, o + cap));
            at = o + cap;
        }
        pieces.push((at, c.size - 4));
        assert_eq!(merge(EH, pieces), s.rows.holes, "holes are exactly the unused bytes");
        for h in &s.rows.holes {
            assert!(h.size >= CIE_MIN || h.offset > 0, "a short hole after a record");
            assert_eq!(h.size % 4, 0);
        }
        // Rewritten ranges: records from the range's start.
        for r in &out.layout.ranges {
            if let Some(t) = touched.get(&r.file) {
                let mut at = r.offset;
                let want: Vec<u64> = t
                    .sizes
                    .iter()
                    .map(|n| {
                        let o = at;
                        at += n;
                        o
                    })
                    .collect();
                assert_eq!(r.records, want);
            } else {
                assert!(
                    out.layout.whole && r.records.is_empty(),
                    "an unchanged range is moved, not rewritten"
                );
            }
        }
        if out.layout.whole {
            let v = s.bytes.iter().map(|(&f, &n)| (f, s.ranks[&f], n)).collect();
            let (ranges, holes, size) = fresh(v);
            let got: Vec<_> = by_off
                .iter()
                .map(|(f, r)| (*f, u64::from(r.offset), u64::from(r.capacity)))
                .collect();
            assert_eq!(got, ranges, "the whole rewrite is the fresh layout");
            assert_eq!(
                merge(EH, holes.iter().map(|h| (h.offset, h.offset + h.size)).collect()),
                s.rows.holes
            );
            assert_eq!(size, c.size);
            assert_eq!(out.layout.ranges.len(), ranges.len(), "every range given");
        }
        let mut sorted = out.layout.ranges.clone();
        sorted.sort_by_key(|r| r.offset);
        assert_eq!(sorted, out.layout.ranges, "ranges sorted by offset");
    }

    fn base() -> Vec<F> {
        vec![
            f(1, 1 << 20, &[24, 48, 32]),
            f(2, 2 << 20, &[20, 64]),
            f(3, 3 << 20, &[24, 40, 40, 40]),
            f(4, 4 << 20, &[20]),
            f(5, 5 << 20, &[24, 32, 32]),
        ]
    }

    fn layout_err(e: &IncrReason) -> bool {
        *e == IncrReason::UpdateNotImplemented(Some(Unimplemented::Layout))
    }

    #[test]
    fn changed_file_fits_in_place() {
        let s = first(&base(), 0);
        let (s2, whole) = hop(&s, &[f(2, 2 << 20, &[20, 60])]);
        assert!(!whole);
        let (_, whole) = hop(&s2, &[f(3, 3 << 20, &[24, 40, 44, 40])]);
        assert!(!whole);
    }

    #[test]
    fn changed_file_shrinks_short_pad_and_cie_pad() {
        let s = first(&base(), 0);
        // File 4: 20 bytes, capacity 36. 32 bytes leave 4 (the last
        // record's nops), 16 leave a padding CIE of 20.
        let (s2, whole) = hop(&s, &[f(4, 4 << 20, &[20, 12])]);
        assert!(!whole);
        assert!(s2.rows.holes.iter().any(|h| h.size == 4));
        let (_, whole) = hop(&s2, &[f(4, 4 << 20, &[16])]);
        assert!(!whole);
    }

    #[test]
    fn changed_file_grows_past_capacity_rewrites_whole() {
        let s = first(&base(), 4096);
        let (s2, whole) = hop(&s, &[f(2, 2 << 20, &[20, 64, 64])]);
        assert!(whole);
        // Fresh capacities: the next growth of the same size fits again.
        let (_, whole) = hop(&s2, &[f(2, 2 << 20, &[20, 64, 64, 4])]);
        assert!(!whole);
    }

    #[test]
    fn whole_rewrite_outgrowing_the_chunk_is_reserve_exhausted() {
        let s = first(&base(), 0);
        let e = run(&s, &[f(2, 2 << 20, &[20, 4000])], true).unwrap_err();
        assert!(
            matches!(e, IncrReason::ReserveExhausted(ref m) if m.contains(".eh_frame")),
            "{e:?}"
        );
    }

    #[test]
    fn appeared_file_rewrites_whole_by_rank_not_id() {
        let s = first(&base(), 4096);
        // FILES row 9, rank between files 2 and 3.
        let (s2, whole) = hop(&s, &[f(9, (2 << 20) + (1 << 19), &[24, 32])]);
        assert!(whole);
        let at =
            |file: u32| s2.rows.eh_ranges.iter().find(|e| e.0 == FileId(file)).unwrap().1.offset;
        assert!(at(2) < at(9) && at(9) < at(3));
    }

    #[test]
    fn appeared_or_drawn_in_without_records_touches_nothing() {
        let s = first(&base(), 0);
        assert_eq!(run(&s, &[f(9, 7, &[])], true).unwrap(), None);
    }

    #[test]
    fn gone_file_range_becomes_a_hole() {
        let s = first(&base(), 0);
        let (s2, whole) = hop(&s, &[f(3, 3 << 20, &[])]);
        assert!(!whole);
        // File 2's pad, file 3's range and nothing else: one hole.
        let r2 = s.rows.eh_ranges[1].1;
        let r3 = s.rows.eh_ranges[2].1;
        let h = s2.rows.holes.iter().find(|h| h.offset == u64::from(r2.offset) + 84).unwrap();
        assert_eq!(h.offset + h.size, u64::from(r3.offset + r3.capacity));
    }

    #[test]
    fn drawn_in_file_keeps_its_place() {
        let s = first(&base(), 0);
        let (s2, whole) = hop(&s, &[f(3, 3 << 20, &[24, 40, 40, 40])]);
        assert!(!whole);
        assert_eq!(s2.rows.eh_ranges, s.rows.eh_ranges);
        assert_eq!(s2.rows.holes, s.rows.holes);
    }

    #[test]
    fn newly_extracted_and_no_longer_extracted() {
        let s = first(&base(), 4096);
        // A member newly extracted between two stored ranges: whole (R-3).
        let (s2, whole) = hop(&s, &[f(7, (4 << 20) + 5, &[24, 28])]);
        assert!(whole);
        // No longer extracted: its range is a hole.
        let (_, whole) = hop(&s2, &[f(7, (4 << 20) + 5, &[])]);
        assert!(!whole);
    }

    #[test]
    fn comdat_keeper_moving() {
        // Files 2 and 5 hold the group; file 2 keeps it. File 2's new
        // version drops it: its FDEs for the group's functions go, and
        // file 5 (drawn in, its group now kept) gains them.
        let mut files = base();
        files[4].sizes = vec![24, 32];
        let s = first(&files, 4096);
        let (_, whole) = hop(&s, &[f(2, 2 << 20, &[20]), f(5, 5 << 20, &[24, 32, 16])]);
        assert!(!whole, "file 5's capacity holds the group's FDE");
        let (_, whole) = hop(&s, &[f(2, 2 << 20, &[20]), f(5, 5 << 20, &[24, 32, 64, 64])]);
        assert!(whole);
    }

    #[test]
    fn three_hops_and_edit_a_then_edit_b() {
        let s = first(&base(), 8192);
        let (s, _) = hop(&s, &[f(1, 1 << 20, &[24, 48, 36])]);
        let (s, _) = hop(&s, &[f(3, 3 << 20, &[24, 40])]);
        let (s, w) = hop(&s, &[f(1, 1 << 20, &[24, 48, 36, 80])]);
        assert!(w);
        let (s, _) = hop(&s, &[f(3, 3 << 20, &[24, 40, 40, 40, 40])]);
        let (_, w) = hop(&s, &[f(5, 5 << 20, &[])]);
        assert!(!w);
    }

    #[test]
    fn compaction_between_two_edits_orders_by_rank() {
        let s = first(&base(), 8192);
        let (s, _) = hop(&s, &[f(2, 2 << 20, &[20, 60])]);
        // A compaction renumbers the FILES rows (ids reversed), ranks kept.
        let ren = |x: FileId| FileId(10 - x.0);
        let mut c = s.clone();
        c.rows.eh_ranges = s.rows.eh_ranges.iter().map(|&(f, r)| (ren(f), r)).collect();
        c.rows.eh_ranges.sort_by_key(|e| e.0);
        c.ranks = s.ranks.iter().map(|(&f, &r)| (ren(f), r)).collect();
        c.bytes = s.bytes.iter().map(|(&f, &n)| (ren(f), n)).collect();
        // File 3 (now row 7) grows past its capacity: whole, by rank.
        let (c2, whole) = hop(&c, &[f(7, 3 << 20, &[24, 40, 40, 40, 400])]);
        assert!(whole);
        let off =
            |file: u32| c2.rows.eh_ranges.iter().find(|e| e.0 == FileId(file)).unwrap().1.offset;
        assert!(off(9) < off(8) && off(8) < off(7) && off(7) < off(6) && off(6) < off(5));
    }

    #[test]
    fn a_file_gaining_its_first_records_rewrites_whole() {
        let mut files = base();
        files.push(f(6, 6 << 20, &[]));
        let s = first(&files, 4096);
        let (_, whole) = hop(&s, &[f(6, 6 << 20, &[24, 20])]);
        assert!(whole);
    }

    #[test]
    fn internal_file_is_last() {
        let mut files = base();
        files.push(f(0, 0, &[24, 24]));
        let s = first(&files, 4096);
        let (s2, whole) = hop(&s, &[f(8, 9 << 20, &[24])]);
        assert!(whole);
        let last = s2.rows.eh_ranges.iter().max_by_key(|e| e.1.offset).unwrap();
        assert_eq!(last.0, FileId::INTERNAL);
    }

    #[test]
    fn last_file_with_records_going_is_new_output_section() {
        let s = first(&[f(1, 1, &[24, 32])], 0);
        let e = run(&s, &[f(1, 1, &[])], true).unwrap_err();
        assert_eq!(e, IncrReason::NewOutputSection(".eh_frame left with no record".into()));
        let s = first(&[f(1, 1, &[24, 32]), f(2, 2, &[24])], 0);
        let e = run(&s, &[f(1, 1, &[]), f(2, 2, &[])], true).unwrap_err();
        assert!(matches!(e, IncrReason::NewOutputSection(_)));
    }

    #[test]
    fn whole_without_the_rows_is_not_implemented() {
        let s = first(&base(), 4096);
        assert!(layout_err(&run(&s, &[f(2, 2 << 20, &[20, 400])], false).unwrap_err()));
        assert!(layout_err(&run(&s, &[f(9, 1, &[24])], false).unwrap_err()));
    }

    #[test]
    fn refusals() {
        // A range and no `EhFile` for a file of the update.
        let s = first(&base(), 0);
        let stored = StoredLayout::new(&s.rows);
        let chunks = Column::from_vec(s.rows.chunks.clone()).unwrap();
        assert!(layout_err(&item_5(&stored, &[], &[FileId(2)], &chunks, None).unwrap_err()));
        // `.eh_frame_hdr` (its size needs the FDE count).
        let mut h = s.clone();
        h.rows.chunks.push(synth(EH_FRAME_HDR, 52));
        assert!(layout_err(&run(&h, &[f(2, 2 << 20, &[20, 60])], true).unwrap_err()));
        // An FDE first, a record not of whole words.
        let mut eh = eh_files(&[f(2, 2 << 20, &[20, 64])]);
        eh[0].records[0].cie = false;
        assert!(layout_err(&item_5(&stored, &eh, &[], &chunks, None).unwrap_err()));
        let eh = eh_files(&[f(2, 2 << 20, &[20, 62])]);
        assert!(layout_err(&item_5(&stored, &eh, &[], &chunks, None).unwrap_err()));
        // A corrupt range.
        let mut c = s.clone();
        c.rows.eh_ranges[1].1.capacity = 1 << 20;
        assert!(matches!(run(&c, &[f(2, 2 << 20, &[20])], true), Err(IncrReason::StateCorrupt(_))));
        // Records and no `.eh_frame` chunk.
        let mut n = MemoryLayout::default();
        n.chunks.push(Chunk { kind: ChunkKind::FileHeader, ..synth(b"", 64) });
        let st = State { rows: n, ranks: BTreeMap::new(), bytes: BTreeMap::new() };
        assert!(matches!(run(&st, &[f(1, 1, &[24])], true), Err(IncrReason::NewOutputSection(_))));
    }

    #[test]
    fn capacity_rule_is_slack_eh_pad() {
        for n in (4..20_000).step_by(4) {
            assert_eq!(n + Slack::INCREMENTAL.eh_pad(n), cap_of(n));
        }
    }
}
