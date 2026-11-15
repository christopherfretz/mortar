//! References into split sections (`spec/synth.md` §3.4, §3.5, §4.4):
//! S-17 to S-22 for `Merge` sections, S-50 for `.eh_frame`.
//!
//! These map an input place to a piece; the output address is then
//! `out(frag) + within` (`MergeMap`, layout), or the `.eh_frame` output
//! offset `mortar_synth` gives.

use mortar_core::{FragId, SectionId, Split};

use crate::eh::EhFrames;
use crate::problem::What;

/// A place in a fragment: the output value is `out(frag) + within`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct PieceRef {
    pub frag: FragId,
    /// The offset from the piece's start (S-17), or from it plus nothing
    /// for a named symbol (S-19 adds `A` after the mapping).
    pub within: u64,
    /// The piece's ordinal in its section (the diagnostic key's `piece`).
    pub ordinal: u32,
}

/// S-17, S-18: a section symbol of split `section` (of `size` bytes) plus
/// addend: `r = st_value + A`. `0 ≤ r < size` gives the piece containing
/// byte `r`; otherwise `OutsideSection`. `None` for a section with no
/// pieces (empty, so every `r` is outside).
pub fn section_symbol(
    split: &Split,
    section: SectionId,
    size: u64,
    r: i64,
) -> Result<PieceRef, What> {
    let r = u64::try_from(r).ok().filter(|&r| r < size).ok_or(What::OutsideSection)?;
    at(split, section, r).ok_or(What::OutsideSection)
}

/// S-19, S-22: a named symbol at `v = st_value` in split `section` (of
/// `size` bytes): the piece containing byte `v`, or when `v == size` the
/// piece ending there (the last). The caller adds `A` after the mapping.
/// `v > size` is outside the section.
pub fn named_symbol(
    split: &Split,
    section: SectionId,
    size: u64,
    v: u64,
) -> Result<PieceRef, What> {
    if v > size {
        return Err(What::OutsideSection);
    }
    if v == size {
        let span = split.merge_pieces(section).ok_or(What::OutsideSection)?;
        let last = span.start + span.len - 1;
        let start = u64::from(split.piece_off[last as usize]);
        return Ok(PieceRef {
            frag: split.piece_frag[last as usize],
            within: v - start,
            ordinal: span.len - 1,
        });
    }
    at(split, section, v).ok_or(What::OutsideSection)
}

fn at(split: &Split, section: SectionId, r: u64) -> Option<PieceRef> {
    let p = split.merge_piece_at(section, u32::try_from(r).ok()?)?;
    Some(PieceRef {
        frag: split.piece_frag[p.piece as usize],
        within: u64::from(p.within),
        ordinal: p.ordinal,
    })
}

/// log2 of the bytes per bucket of a [`PieceIndex`].
const BUCKET: u32 = 8;

/// One `Merge` section of a [`PieceIndex`].
#[derive(Debug, Clone, Copy)]
struct IndexRow {
    section: SectionId,
    /// The section's size (`sh_size`).
    size: u64,
    /// Its pieces in `Split`'s columns.
    pieces: mortar_core::Span,
    /// Its first bucket in `PieceIndex::buckets`.
    bucket: u32,
}

/// [`section_symbol`] and [`named_symbol`] for many references (fill's,
/// `spec/image.md` I-119): per `Merge` section, its size and, per
/// `2^BUCKET` bytes, the ordinal of the piece holding the bucket's first
/// byte. A reference then searches only the pieces of its bucket (about
/// four in `.debug_str`) instead of all of its section's, and does not
/// read the section's header for its size. Memory: 16 bytes per section
/// and 4 per `2^BUCKET` bytes of mergeable input (0.8 MB for 52 MB).
/// The results are those of [`section_symbol`] and [`named_symbol`]
/// exactly (`piece_index_agrees` checks every offset of every section of
/// the corpus).
#[derive(Debug, Clone, Default)]
pub struct PieceIndex {
    rows: Box<[IndexRow]>,
    buckets: Box<[u32]>,
}

impl PieceIndex {
    /// The index of `split`'s `Merge` sections, `size(section)` giving
    /// each one's size (a section without one is left out: every
    /// reference into it is then outside, as a section without pieces).
    #[must_use]
    pub fn new(split: &Split, size: impl Fn(SectionId) -> Option<u64>) -> Self {
        let mut rows = Vec::with_capacity(split.merge_of.len());
        let mut buckets: Vec<u32> = Vec::new();
        for row in &split.merge_of {
            let Some(size) = size(row.section) else { continue };
            let Some(offsets) = split.piece_off.get(row.pieces.slots()) else { continue };
            let Ok(bucket) = u32::try_from(buckets.len()) else { break };
            // Offsets past u32 are never found (`at` converts first).
            let n = size.min(1 << 32).div_ceil(1 << BUCKET);
            let mut p = 0usize;
            for k in 0..n {
                let start = k << BUCKET;
                while p + 1 < offsets.len() && u64::from(offsets[p + 1]) <= start {
                    p += 1;
                }
                buckets.push(p as u32);
            }
            rows.push(IndexRow { section: row.section, size, pieces: row.pieces, bucket });
        }
        Self { rows: rows.into_boxed_slice(), buckets: buckets.into_boxed_slice() }
    }

    fn row(&self, section: SectionId) -> Option<&IndexRow> {
        let at = self.rows.binary_search_by_key(&section, |r| r.section).ok()?;
        self.rows.get(at)
    }

    /// The size of `section` when the index holds it.
    #[must_use]
    pub fn size(&self, section: SectionId) -> Option<u64> {
        self.row(section).map(|r| r.size)
    }

    /// The bytes the index holds.
    #[must_use]
    pub fn heap_bytes(&self) -> usize {
        self.rows.len() * size_of::<IndexRow>() + self.buckets.len() * 4
    }

    /// [`section_symbol`] of `section` with the size the index holds;
    /// `None` when the index does not hold the section.
    #[must_use]
    pub fn section_symbol(
        &self,
        split: &Split,
        section: SectionId,
        r: i64,
    ) -> Option<Result<PieceRef, What>> {
        let row = self.row(section)?;
        Some(match u64::try_from(r).ok().filter(|&r| r < row.size) {
            Some(r) => self.at(split, row, r).ok_or(What::OutsideSection),
            None => Err(What::OutsideSection),
        })
    }

    /// The row of `section` in the index ([`PieceIndex::section_symbol_in`]).
    #[must_use]
    pub fn row_of(&self, section: SectionId) -> Option<u32> {
        let at = self.rows.binary_search_by_key(&section, |r| r.section).ok()?;
        u32::try_from(at).ok()
    }

    /// [`PieceIndex::section_symbol`] of the section of row `row`
    /// ([`PieceIndex::row_of`]); `None` when there is no such row.
    #[must_use]
    pub fn section_symbol_in(
        &self,
        split: &Split,
        row: u32,
        r: i64,
    ) -> Option<Result<PieceRef, What>> {
        let row = self.rows.get(row as usize)?;
        Some(match u64::try_from(r).ok().filter(|&r| r < row.size) {
            Some(r) => self.at(split, row, r).ok_or(What::OutsideSection),
            None => Err(What::OutsideSection),
        })
    }

    /// [`named_symbol`] of `section` with the size the index holds;
    /// `None` when the index does not hold the section.
    #[must_use]
    pub fn named_symbol(
        &self,
        split: &Split,
        section: SectionId,
        v: u64,
    ) -> Option<Result<PieceRef, What>> {
        let row = self.row(section)?;
        Some(if v < row.size {
            self.at(split, row, v).ok_or(What::OutsideSection)
        } else {
            named_symbol(split, section, row.size, v)
        })
    }

    fn at(&self, split: &Split, row: &IndexRow, r: u64) -> Option<PieceRef> {
        let r32 = u32::try_from(r).ok()?;
        let offsets = split.piece_off.get(row.pieces.slots())?;
        let k = (r >> BUCKET) as usize;
        let base = row.bucket as usize;
        let lo = *self.buckets.get(base + k)? as usize;
        // The next bucket's first piece holds its first byte, so byte `r`
        // is in a piece at or before it.
        let n = (row.size.min(1 << 32).div_ceil(1 << BUCKET)) as usize;
        let hi = if k + 1 < n {
            (*self.buckets.get(base + k + 1)? as usize + 1).min(offsets.len())
        } else {
            offsets.len()
        };
        let found = offsets.get(lo..hi)?.partition_point(|&start| start <= r32).checked_sub(1)?;
        let ordinal = lo + found;
        let piece = row.pieces.start as usize + ordinal;
        Some(PieceRef {
            frag: *split.piece_frag.get(piece)?,
            within: u64::from(r32 - offsets[ordinal]),
            ordinal: ordinal as u32,
        })
    }
}

/// S-21: a GOT-generating relocation (`got`) against a section symbol
/// (`section_symbol`) of a split `Merge` section with a non-zero addend
/// is refused. Through a named symbol the slot holds the S-19 value with
/// `A = 0`.
pub fn check_got(got: bool, section_symbol: bool, addend: i64) -> Result<(), What> {
    if got && section_symbol && addend != 0 { Err(What::GotSectionAddend) } else { Ok(()) }
}

/// What refers into an `.eh_frame` (S-50).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum EhWho {
    /// A relocation (through a section or a named symbol: its target
    /// offset is `st_value + A` or the symbol's value).
    Reloc,
    /// A symbol other than a mapping symbol. `droppable` when it is a
    /// local that no relocation refers to (rule 4 drops it instead of
    /// moving it).
    Symbol { droppable: bool },
    /// An AArch64 mapping symbol (`$d`): it follows its record (S-76).
    Mapping,
}

/// The value of a reference into an `.eh_frame` (S-50).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum EhValue {
    /// An offset from the start of the output `.eh_frame`.
    Offset(u64),
    /// The symbol is dropped from `.symtab`.
    Dropped,
}

/// S-50: the value of a reference at offset `r` of section row `s` of
/// `eh` (`EhFrames::sections[s]`).
///
/// - `kept(record)`: the output offset of a kept record (a global index
///   of [`EhFrames::records`]), `None` for a removed one;
/// - `after`: the output offset of the next kept record after the
///   section's records in output order, or of the final terminator.
///
/// Rules: 1, `r = 0` is the output's start; 2, inside or at the start of
/// a kept record follows it; 3, at the start of a removed record (or its
/// terminator), or `r = sh_size`, is the next kept record of the section,
/// else `after`; 4, inside a removed record refuses a relocation, drops a
/// droppable symbol, and gives another symbol rule 3's value; 5, `r < 0`
/// or `r > sh_size` is outside. A mapping symbol is kept at its record's
/// place exactly when the record is kept.
pub fn eh_ref(
    eh: &EhFrames,
    s: usize,
    r: i64,
    who: EhWho,
    kept: impl Fn(u32) -> Option<u64>,
    after: u64,
) -> Result<EhValue, What> {
    let sec = &eh.sections[s];
    let size = u64::from(sec.size);
    let r = u64::try_from(r).ok().filter(|&r| r <= size).ok_or(What::OutsideSection)?;
    let first = sec.records.start;
    let records = eh.records_of(s);
    // The record containing r (index in the section), if any.
    let i = records.partition_point(|rec| u64::from(rec.offset) <= r).checked_sub(1);
    let i = i.filter(|&i| r < records[i].end());
    // Rule 3's value for a place at or after removed record `from`.
    let next_kept =
        |from: usize| (from..records.len()).find_map(|j| kept(first + j as u32)).unwrap_or(after);
    if who == EhWho::Mapping {
        return Ok(match i.and_then(|i| kept(first + i as u32).map(|o| (i, o))) {
            Some((i, o)) => EhValue::Offset(o + (r - u64::from(records[i].offset))),
            None => EhValue::Dropped,
        });
    }
    if r == 0 {
        return Ok(EhValue::Offset(0));
    }
    let Some(i) = i else {
        // At or inside the terminator, or at the end.
        let start = sec.terminator.map(u64::from);
        if r == size || start == Some(r) {
            return Ok(EhValue::Offset(after));
        }
        return match who {
            EhWho::Reloc => Err(What::IntoRemovedRecord),
            EhWho::Symbol { droppable: true } => Ok(EhValue::Dropped),
            _ => Ok(EhValue::Offset(after)),
        };
    };
    let rec = &records[i];
    let delta = r - u64::from(rec.offset);
    if let Some(o) = kept(first + i as u32) {
        return Ok(EhValue::Offset(o + delta));
    }
    if delta == 0 {
        return Ok(EhValue::Offset(next_kept(i + 1)));
    }
    match who {
        EhWho::Reloc => Err(What::IntoRemovedRecord),
        EhWho::Symbol { droppable: true } => Ok(EhValue::Dropped),
        _ => Ok(EhValue::Offset(next_kept(i + 1))),
    }
}
