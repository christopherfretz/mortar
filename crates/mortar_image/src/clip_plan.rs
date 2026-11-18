//! The clip plan (WP-255 §9.1; WP-255c2a): the fill plan of an update's
//! dirty ranges ("clips") only, built from the row seam [`Rows`] and the
//! data lists the caller gives, by the SAME row makers the whole plan
//! ([`crate::layout_plan::from_layout`]) calls: [`row_for`] for each placed
//! input section that meets a clip, [`pads`] whole, [`hole_bytes`] for the
//! debug holes, the structural chunks' bytes of
//! [`crate::link::write_structure`]. Inside the clips its rows are the
//! whole plan's rows (the test `clip_plan_rows_equal_the_whole_plan_clipped`).
//!
//! What it does not build yet is refused by name ([`ClipError::NotYet`]):
//! a clip that meets a `Merged`, `Synthetic` or `Patches` chunk, or an
//! input `.eh_frame` (255c2b builds those from stored rows); never a
//! silent zero. A plain link never calls it.
//!
//! [`check_written`] is §9.1's written-bitmap rule, which the clip filler
//! runs under `debug_assertions`: every clip byte is written by an explicit
//! row (a unit or a pad), or lies in a zero gap derivable from the chunks.

use std::collections::BTreeMap;
use std::ops::Range;

use mortar_arch::ProofRun;
use mortar_core::layout::{Chunk, ChunkKind, Hole};
use mortar_core::par::Pool;
use mortar_core::{ChunkId, Column, FileId, SectionId};
use mortar_diag::{Diagnostic, Kind};
use mortar_elf::SectionIndex;

use crate::layout_plan::{ChunkView, SegView, hole_bytes, pads};
use crate::plan::{Builder, Pattern, Plan, Unit};
use crate::rows::{Rows, SectionFront, row_for};
use crate::target::Target;

/// `SHF_EXECINSTR`.
const SHF_EXECINSTR: u64 = 0x4;

/// A placed input section that meets a clip: its id, its place and its
/// file's front ([`row_for`]'s inputs). Who finds them is the caller's (an
/// update: the state's by-range index).
#[derive(Debug, Clone, Copy)]
pub struct ClipSection<'f> {
    pub section: SectionId,
    /// `(chunk, offset in the chunk)`.
    pub place: (ChunkId, u64),
    pub front: SectionFront<'f>,
}

/// The data lists the clip plan's non-`Input` rows are made from.
#[derive(Clone, Copy)]
pub struct ClipParts<'p> {
    pub target: Target,
    /// [`pads`]' views, whole: every in-file chunk with bytes and every
    /// `PT_LOAD` segment.
    pub chunk_views: &'p [ChunkView],
    pub seg_views: &'p [SegView],
    /// `-z separate-code`.
    pub separate_code: bool,
    /// The holes of the chunks a clip meets, each with its chunk's output
    /// section name (the neutral unit's form).
    pub holes: &'p [(Hole, &'p [u8])],
    /// The neutral abbreviation table's offset in its section
    /// ([`crate::layout_plan::abbrev_offset`] over the whole placement).
    pub abbrev: u32,
    /// The structural chunks' bytes ([`crate::link::write_structure`]),
    /// for each one a clip meets.
    pub structure: &'p BTreeMap<ChunkId, Box<[u8]>>,
    /// The proof runs of input section `(file, index)` of `len` bytes.
    pub proof: &'p dyn Fn(FileId, SectionIndex, u64) -> Vec<ProofRun>,
}

impl std::fmt::Debug for ClipParts<'_> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ClipParts")
            .field("chunk_views", &self.chunk_views.len())
            .field("holes", &self.holes.len())
            .field("structure", &self.structure.len())
            .finish_non_exhaustive()
    }
}

/// What a clip meets that the clip plan does not build yet (255c2b).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NotYet {
    /// A `Merged` chunk (I-38).
    Merged,
    /// A synthetic chunk (§13), `.symtab` and `.eh_frame` included.
    Synth,
    /// A patch island (thunks, erratum patches).
    Patches,
    /// An input `.eh_frame` section placed in a body chunk.
    EhFrame,
    /// A `-r` chunk (a group or a relocation section).
    Relocatable,
}

/// Why [`for_clips`] made no plan.
#[derive(Debug)]
pub enum ClipError {
    /// A clip meets chunk `ChunkId` of a kind not built yet: the update
    /// is `update-not-implemented (image)`.
    NotYet(NotYet, ChunkId),
    /// The inputs are not a clip plan's (an `internal-error`).
    Internal(Diagnostic),
}

impl From<Diagnostic> for ClipError {
    fn from(d: Diagnostic) -> Self {
        Self::Internal(d)
    }
}

fn internal(what: String) -> ClipError {
    ClipError::Internal(Diagnostic::error(Kind::InternalError {
        what: format!("image: clip plan: {what}"),
    }))
}

/// Whether `[lo, hi)` meets one of `clips` (sorted, disjoint).
fn meets(clips: &[Range<u64>], lo: u64, hi: u64) -> bool {
    let k = clips.partition_point(|c| c.end <= lo);
    clips.get(k).is_some_and(|c| c.start < hi && lo < hi)
}

/// The plan of `clips` (sorted, disjoint, non-empty ranges of the file):
/// inside the clips, the rows of the whole plan of the same rows; outside
/// them, gaps. `sections` are every placed input section that meets a
/// clip, sorted by file offset.
///
/// # Errors
///
/// [`ClipError::NotYet`] for a clip that meets a chunk the clip plan does
/// not build yet; [`ClipError::Internal`] for clips that are not sorted
/// and disjoint, a section that is not in an in-file `Body` chunk or meets
/// no clip, sections out of order, a structural chunk the clip meets and
/// `parts.structure` does not hold (or holds at another size), and the
/// plan check's own (I-27).
pub fn for_clips<R: Rows>(
    rows: &R,
    clips: &[Range<u64>],
    sections: &[ClipSection<'_>],
    parts: &ClipParts<'_>,
) -> Result<Plan, ClipError> {
    if clips.iter().any(|c| c.start >= c.end) || clips.windows(2).any(|w| w[0].end > w[1].start) {
        return Err(internal(format!("clips {clips:x?} are not sorted, disjoint and non-empty")));
    }
    let chunks = rows.chunks();
    let file_size = chunks
        .iter()
        .filter(|(_, c)| c.in_file())
        .map(|(_, c)| c.offset + c.size)
        .max()
        .unwrap_or(0);
    let mut b = Builder::new(parts.target, file_size);
    for (id, c) in chunks.iter() {
        if !c.in_file() || c.size == 0 || !meets(clips, c.offset, c.offset + c.size) {
            continue;
        }
        let not_yet = match c.kind {
            ChunkKind::Body(_) => continue,
            ChunkKind::FileHeader | ChunkKind::ProgramHeaders | ChunkKind::SectionHeaders => {
                let bytes = parts.structure.get(&id).ok_or_else(|| {
                    internal(format!("the structural chunk {} has no bytes", id.0))
                })?;
                if bytes.len() as u64 != c.size {
                    return Err(internal(format!(
                        "the structural chunk {} has {} bytes for {}",
                        id.0,
                        bytes.len(),
                        c.size
                    )));
                }
                b.bytes(c.offset, bytes.clone());
                continue;
            }
            ChunkKind::Merged(_) => NotYet::Merged,
            ChunkKind::Synthetic(_) => NotYet::Synth,
            ChunkKind::Patches(_) => NotYet::Patches,
            ChunkKind::Group(_) | ChunkKind::Relocs(_) => NotYet::Relocatable,
        };
        return Err(ClipError::NotYet(not_yet, id));
    }
    let mut last = 0u64;
    for s in sections {
        let (c, off) = s.place;
        let chunk = chunks
            .get(c)
            .ok_or_else(|| internal(format!("section {} in no chunk", s.section.0)))?;
        if !chunk.in_file() || !matches!(chunk.kind, ChunkKind::Body(_)) {
            return Err(internal(format!(
                "section {} is not in an in-file body chunk",
                s.section.0
            )));
        }
        let at = chunk.offset + off;
        if at < last {
            return Err(internal(format!("section {} is out of file order", s.section.0)));
        }
        last = at;
        if !meets(clips, at, at + s.front.size) {
            return Err(internal(format!("section {} meets no clip", s.section.0)));
        }
        if s.front.name == b".eh_frame" {
            return Err(ClipError::NotYet(NotYet::EhFrame, c));
        }
        row_for(rows, s.section, s.place, &s.front, parts.proof, &mut b)?;
    }
    for (h, name) in parts.holes {
        let Some(c) = chunks.get(h.chunk) else {
            return Err(internal(format!("a hole in no chunk {}", h.chunk.0)));
        };
        let at = c.offset + h.offset;
        if !c.in_file() || !meets(clips, at, at + h.size) {
            continue;
        }
        if let Some(bytes) = hole_bytes(h, name, parts.target, parts.abbrev)? {
            b.bytes(at, bytes.into_boxed_slice());
        }
    }
    pads(parts.chunk_views.to_vec(), parts.seg_views, parts.target, parts.separate_code, &mut b);
    Ok(b.finish(crate::IMAGE_BLOCK, &Pool::sequential())?)
}

/// §9.1's written-bitmap rule over clip piece `piece` of `plan`: every
/// byte is written by an explicit row (an `Input`, `Bytes`, `Synth` or
/// `Merged` unit, or a gap of a non-zero pattern, which only [`pads`]
/// makes), or lies in a zero gap derivable from `chunks`: no in-file chunk
/// there, a non-exec chunk's tail between its size and its capacity, or a
/// non-exec `Body` chunk's bytes no section covers (alignment padding,
/// I-41: zero). The clip filler runs it under `debug_assertions`.
///
/// # Errors
///
/// `internal-error (image: clip byte {offset:#x} written by no row)` for
/// the first byte neither marks.
pub fn check_written(
    plan: &Plan,
    chunks: &Column<ChunkId, Chunk>,
    piece: &Range<u64>,
) -> Result<(), Diagnostic> {
    let len = piece.end.saturating_sub(piece.start);
    let mut bits = vec![0u64; usize::try_from(len.div_ceil(64)).unwrap_or(0)];
    let mut mark = |lo: u64, hi: u64| {
        for o in lo.max(piece.start)..hi.min(piece.end) {
            let i = o - piece.start;
            bits[(i / 64) as usize] |= 1 << (i % 64);
        }
    };
    for row in &plan.rows[plan.first_row(piece.start)..] {
        if row.offset >= piece.end {
            break;
        }
        match row.unit {
            Unit::Gap(p) if p == Pattern::ZERO => {
                for (_, c) in chunks.iter().filter(|(_, c)| c.in_file()) {
                    let end = c.offset + c.size.max(c.capacity);
                    if c.flags & SHF_EXECINSTR != 0 || end <= row.offset || c.offset >= row.end() {
                        continue;
                    }
                    if matches!(c.kind, ChunkKind::Body(_)) {
                        mark(c.offset, end);
                    } else {
                        mark(c.offset + c.size, end);
                    }
                }
                // No chunk there: every byte of the row no chunk holds.
                let mut at = row.offset;
                let mut held: Vec<(u64, u64)> = chunks
                    .iter()
                    .filter(|(_, c)| c.in_file())
                    .map(|(_, c)| (c.offset, c.offset + c.size.max(c.capacity)))
                    .filter(|&(lo, hi)| lo < row.end() && hi > row.offset && lo < hi)
                    .collect();
                held.sort_unstable();
                for (lo, hi) in held {
                    if lo > at {
                        mark(at, lo.min(row.end()));
                    }
                    at = at.max(hi);
                }
                if at < row.end() {
                    mark(at, row.end());
                }
            }
            _ => mark(row.offset, row.end()),
        }
    }
    for (k, w) in bits.iter().enumerate() {
        if *w != u64::MAX {
            let first = (0..64).find(|b| w & (1 << b) == 0).unwrap_or(0);
            let off = piece.start + k as u64 * 64 + first;
            if off < piece.end {
                return Err(Diagnostic::error(Kind::InternalError {
                    what: format!("image: clip byte {off:#x} written by no row"),
                }));
            }
        }
    }
    Ok(())
}
