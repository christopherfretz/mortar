//! `Split` (`spec/model.md` §3.5): sections handled in pieces. Written by
//! `mortar_merge`'s first stage. The largest table of a debug link: 8
//! bytes per piece occurrence, 16 per fragment (§9).

use crate::assignment::Assignment;
use crate::column::Column;
use crate::id::{FragId, Id, OptId, SectionId, Span};
use crate::table::{Broken, ensure, unsorted_at};

/// A split section's pieces: a span of the piece columns.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SplitRow {
    pub section: SectionId,
    pub pieces: Span,
}

/// A distinct piece of mergeable content (§3.5 `frags[FragId]`): its first
/// occurrence, size and alignment. The output section is not stored: it is
/// the output section of `section` (`Assignment::out`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Frag {
    /// The section of the first occurrence.
    pub section: SectionId,
    /// Its offset there.
    pub offset: u32,
    pub size: u32,
    /// log2 of the alignment.
    pub align: u8,
}

/// What an `.eh_frame` piece is (§3.5).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct EhPiece {
    /// For an FDE, the position (in the section's eh pieces) of its CIE;
    /// `u32::MAX` for a CIE.
    pub cie: u32,
    /// For an FDE, the section its code range lies in (from its first
    /// relocation), if it has one.
    pub code: OptId<SectionId>,
}

impl EhPiece {
    /// A CIE.
    pub const CIE: Self = Self { cie: u32::MAX, code: OptId::NONE };

    #[must_use]
    pub fn is_cie(self) -> bool {
        self.cie == u32::MAX
    }
}

/// The table (§3.5). `Merge` and `EhFrame` sections each have their own
/// piece columns, so that a merge piece costs 8 bytes and an eh piece 12.
#[derive(Debug, Clone, Default)]
pub struct Split {
    /// The `Merge` sections, sorted by id, each with its span of
    /// `piece_off` / `piece_frag`.
    pub merge_of: Box<[SplitRow]>,
    /// The offset of each merge piece in its section, ascending per section
    /// from 0.
    pub piece_off: Box<[u32]>,
    /// The fragment of each merge piece.
    pub piece_frag: Box<[FragId]>,
    /// Numbered by first occurrence `(FileId, sh_index, piece)` (§3.5).
    pub frags: Column<FragId, Frag>,
    /// The `EhFrame` sections, sorted by id, each with its span of
    /// `eh_off` / `eh_piece`.
    pub eh_of: Box<[SplitRow]>,
    pub eh_off: Box<[u32]>,
    pub eh_piece: Box<[EhPiece]>,
}

/// The record that owns a field of `.eh_frame` or `.gcc_except_table`
/// (`spec/resolve.md` §6.3, WP-177): a field is judged by its record, not
/// by its section's name.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EhOwner {
    /// A live FDE, or a CIE a live FDE uses (`spec/model.md` §3.6): the
    /// record is written. `code` is the code the FDE describes (for a CIE,
    /// the first live FDE that uses it): its section and offset there.
    Live { code: Option<(SectionId, u64)> },
    /// A record that is not written: its fields are not either.
    Dead,
    /// No record owns the field (a `.gcc_except_table` offset before
    /// every LSDA start of its file's FDEs; a place outside every record).
    None,
}

/// A piece of a split section: the section's row and the piece's position
/// in it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PieceAt {
    /// The piece's position in the piece columns.
    pub piece: u32,
    /// Its position among its section's pieces (the `piece` of a
    /// diagnostic key, §8).
    pub ordinal: u32,
    /// The offset of the reference from the piece's start.
    pub within: u32,
}

fn find(rows: &[SplitRow], section: SectionId) -> Option<SplitRow> {
    rows.binary_search_by_key(&section, |row| row.section).ok().map(|at| rows[at])
}

fn piece_at(
    rows: &[SplitRow],
    offsets: &[u32],
    section: SectionId,
    offset: u32,
) -> Option<PieceAt> {
    let row = find(rows, section)?;
    let pieces = offsets.get(row.pieces.slots())?;
    let ordinal = pieces.partition_point(|&start| start <= offset).checked_sub(1)?;
    Some(PieceAt {
        piece: row.pieces.start + ordinal as u32,
        ordinal: ordinal as u32,
        within: offset - pieces[ordinal],
    })
}

impl Split {
    /// The pieces of a `Merge` section.
    #[must_use]
    pub fn merge_pieces(&self, section: SectionId) -> Option<Span> {
        find(&self.merge_of, section).map(|row| row.pieces)
    }

    /// The pieces of an `EhFrame` section.
    #[must_use]
    pub fn eh_pieces(&self, section: SectionId) -> Option<Span> {
        find(&self.eh_of, section).map(|row| row.pieces)
    }

    /// The merge piece a reference at `offset` in `section` lands in (§3.5:
    /// a reference into a split section is resolved by offset).
    #[must_use]
    pub fn merge_piece_at(&self, section: SectionId, offset: u32) -> Option<PieceAt> {
        piece_at(&self.merge_of, &self.piece_off, section, offset)
    }

    /// The eh piece a reference at `offset` in `section` lands in.
    #[must_use]
    pub fn eh_piece_at(&self, section: SectionId, offset: u32) -> Option<PieceAt> {
        piece_at(&self.eh_of, &self.eh_off, section, offset)
    }

    /// The invariants of §3.5 that the table and `Assignment` show:
    ///
    /// - section rows sorted by id; spans contiguous in that order and
    ///   covering their columns; piece offsets ascending from 0;
    /// - fragments numbered by first occurrence, and each fragment's row
    ///   is its first occurrence; two pieces share a fragment only within
    ///   one output section;
    /// - an FDE's CIE is a CIE of the same section.
    pub fn check(&self, assignment: &Assignment<'_>) -> Result<(), Broken> {
        let t = "Split";
        ensure(self.piece_off.len() == self.piece_frag.len(), t, || {
            "merge piece columns differ".to_owned()
        })?;
        ensure(self.eh_off.len() == self.eh_piece.len(), t, || {
            "eh piece columns differ".to_owned()
        })?;
        for (rows, offsets) in [(&self.merge_of, &self.piece_off), (&self.eh_of, &self.eh_off)] {
            if let Some(at) = unsorted_at(rows, |row| row.section) {
                return Err(Broken { table: t, what: format!("section row {at} out of order") });
            }
            let mut next = 0u64;
            for row in rows {
                let s = row.section.0;
                ensure(u64::from(row.pieces.start) == next && row.pieces.len > 0, t, || {
                    format!("section {s}: pieces not contiguous")
                })?;
                next = row.pieces.end();
                let pieces = offsets.get(row.pieces.slots()).unwrap_or_default();
                ensure(
                    pieces.first() == Some(&0) && pieces.windows(2).all(|p| p[0] < p[1]),
                    t,
                    || format!("section {s}: piece offsets not ascending from 0"),
                )?;
            }
            ensure(next == offsets.len() as u64, t, || {
                "pieces not covered by sections".to_owned()
            })?;
        }
        let mut seen = 0u32;
        for row in &self.merge_of {
            for at in row.pieces.slots() {
                let frag = self.piece_frag[at];
                ensure(frag.0 <= seen && frag.index() < self.frags.len(), t, || {
                    format!("fragment {} not numbered by first occurrence", frag.0)
                })?;
                let first = &self.frags[frag];
                if frag.0 == seen {
                    seen += 1;
                    ensure(
                        first.section == row.section && first.offset == self.piece_off[at],
                        t,
                        || format!("fragment {}: row is not its first occurrence", frag.0),
                    )?;
                }
                ensure(assignment.out(first.section) == assignment.out(row.section), t, || {
                    format!("fragment {} shared across output sections", frag.0)
                })?;
            }
        }
        ensure(seen == self.frags.len(), t, || "fragments without an occurrence".to_owned())?;
        for row in &self.eh_of {
            for at in row.pieces.slots() {
                let piece = self.eh_piece[at];
                ensure(
                    piece.is_cie()
                        || (piece.cie < row.pieces.len
                            && self.eh_piece[row.pieces.start as usize + piece.cie as usize]
                                .is_cie()),
                    t,
                    || format!("section {}: FDE {at} without a CIE of its section", row.section.0),
                )?;
            }
        }
        Ok(())
    }
}

const _: () = assert!(size_of::<Frag>() == 16);
const _: () = assert!(size_of::<SplitRow>() == 12);
const _: () = assert!(size_of::<EhPiece>() == 8);
const _: () = assert!(size_of::<u32>() + size_of::<FragId>() == 8);
