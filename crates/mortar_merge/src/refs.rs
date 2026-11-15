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
