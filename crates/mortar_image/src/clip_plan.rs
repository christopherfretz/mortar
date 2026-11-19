//! The clip plan (WP-255 §9.1; WP-255c2a): the fill plan of an update's
//! dirty ranges ("clips") only, built from the row seam [`Rows`] and the
//! data lists the caller gives, by the SAME row makers the whole plan
//! ([`crate::layout_plan::from_layout`]) calls: [`row_for`] for each placed
//! input section that meets a clip, [`pads`] whole, [`hole_bytes`] for the
//! debug holes, the structural chunks' bytes of
//! [`crate::link::write_structure`]. Inside the clips its rows are the
//! whole plan's rows (the test `clip_plan_rows_equal_the_whole_plan_clipped`).
//!
//! The sections of a clip are [`sections_in`]'s, from the rows and the
//! member index [`Members`] (WP-255c2b): the caller supplies their bytes,
//! never the list. Before any row, the gap rule: a byte of a `Body` chunk
//! in a clip that no member, hole, alignment padding or slack explains is
//! `clip byte ... written by no row` (always on; O(members under the
//! clips)).
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

/// One member of a `Body` chunk as [`Members`] lists it: the section and
/// its alignment (the input header's `sh_addralign`, at least 1).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Member {
    pub section: SectionId,
    pub align: u64,
}

/// The narrow index [`sections_in`] reads beside [`Rows`] (WP-255c2b):
/// for each in-file `Body` chunk, every placed input section that takes an
/// `Input` row (kept, placed, live, not folded, in an object, with file
/// bytes), in offset order, with its alignment. A full link makes it from
/// its placement (`link::clip_members`, `from_layout`'s member rule); an
/// update reads it from a stored table (255e). Where each member is and
/// how big comes from [`Rows::place`] and [`Rows::section_size`], not from
/// the index.
#[derive(Debug, Clone, Default)]
pub struct Members {
    pub by_chunk: BTreeMap<ChunkId, Vec<Member>>,
}

/// A placed input section [`sections_in`] gives: its id, its offset in
/// its chunk, its size and its alignment.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Placed {
    pub section: SectionId,
    pub offset: u64,
    pub size: u64,
    pub align: u64,
}

impl Placed {
    fn end(&self) -> u64 {
        self.offset + self.size
    }
}

/// The source of a clip section's bytes and relocations: the front of
/// section `s` placed at `(chunk, offset)` ([`row_for`]'s input). `None`
/// is an error of [`for_clips`]: every section it asks for takes a row.
pub type FrontOf<'a, 'f> =
    &'a dyn Fn(SectionId, (ChunkId, u64)) -> Result<Option<SectionFront<'f>>, Diagnostic>;

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
    Internal(Box<Diagnostic>),
}

impl From<Diagnostic> for ClipError {
    fn from(d: Diagnostic) -> Self {
        Self::Internal(Box::new(d))
    }
}

fn internal(what: &str) -> ClipError {
    ClipError::Internal(Box::new(Diagnostic::error(Kind::InternalError {
        what: format!("image: clip plan: {what}"),
    })))
}

/// Whether `[lo, hi)` meets one of `clips` (sorted, disjoint).
fn meets(clips: &[Range<u64>], lo: u64, hi: u64) -> bool {
    let k = clips.partition_point(|c| c.end <= lo);
    clips.get(k).is_some_and(|c| c.start < hi && lo < hi)
}

/// Member `m` of chunk `c` placed by `rows`.
fn placed<R: Rows>(rows: &R, c: ChunkId, m: &Member) -> Result<Placed, ClipError> {
    match (rows.place(m.section), rows.section_size(m.section)) {
        (Some((pc, offset)), Some(size)) if pc == c => {
            Ok(Placed { section: m.section, offset, size, align: m.align.max(1) })
        }
        _ => Err(internal(&format!("member {} of chunk {} has no place in it", m.section.0, c.0))),
    }
}

/// The index `k` in `list` of the first member that ends after `lo`, and
/// the members from `k` on that meet `[lo, hi)` (chunk offsets), checked
/// in order and disjoint. O(log members + members met).
fn span<R: Rows>(
    rows: &R,
    c: ChunkId,
    list: &[Member],
    lo: u64,
    hi: u64,
) -> Result<(usize, Vec<Placed>), ClipError> {
    let k = list.partition_point(|m| match (rows.place(m.section), rows.section_size(m.section)) {
        (Some((_, o)), Some(z)) => o + z <= lo,
        _ => false,
    });
    let mut out = Vec::new();
    let mut prev = if k > 0 { placed(rows, c, &list[k - 1])?.end() } else { 0 };
    for m in &list[k..] {
        let p = placed(rows, c, m)?;
        if p.offset < prev {
            return Err(internal(&format!(
                "member {} of chunk {} is out of order or overlaps its predecessor",
                m.section.0, c.0
            )));
        }
        if p.offset >= hi {
            break;
        }
        prev = p.end();
        if p.size > 0 && p.end() > lo {
            out.push(p);
        } else if p.size > 0 {
            return Err(internal(&format!(
                "member {} of chunk {} is out of order",
                m.section.0, c.0
            )));
        }
    }
    Ok((k, out))
}

/// Every placed input section of chunk `c` that meets `[lo, hi)` (offsets
/// in the chunk), in offset order: the members of `members` placed by
/// `rows` (WP-255c2b; ruling "After 255c2a" 2(i): the caller never
/// assembles a clip's section list). A chunk `members` does not list has
/// none.
///
/// # Errors
///
/// [`ClipError::Internal`] for a member `rows` does not place in `c`, or
/// members out of order or overlapping.
pub fn sections_in<R: Rows>(
    rows: &R,
    members: &Members,
    c: ChunkId,
    range: Range<u64>,
) -> Result<Vec<Placed>, ClipError> {
    let Some(list) = members.by_chunk.get(&c) else { return Ok(Vec::new()) };
    Ok(span(rows, c, list, range.start, range.end)?.1)
}

/// `x` rounded up to a multiple of `a` (a power of two, or 1).
fn align_up(x: u64, a: u64) -> u64 {
    x.div_ceil(a.max(1)) * a.max(1)
}

/// The written-by-no-row error at file offset `off`.
fn unwritten(off: u64) -> ClipError {
    ClipError::Internal(Box::new(Diagnostic::error(Kind::InternalError {
        what: format!("image: clip byte {off:#x} written by no row"),
    })))
}

/// The gap rule (ruling "After 255c2a" 2(ii)) over the gap `[p, n)` of
/// chunk `chunk` before an item aligned to `align`: legal when holes
/// starting exactly where the previous item (member or hole) ends, and
/// then the alignment padding of the next item, explain it exactly (`n` is
/// `p`, past the holes, rounded up to `align`; an incremental-mode link's
/// per-member slack is a `Fill` hole right after its member). `lo` is the clip's start (chunk offsets): the
/// error names the first unexplained byte in the clip.
fn explain(
    chunk: &Chunk,
    holes: &[(u64, u64)],
    mut p: u64,
    n: u64,
    align: u64,
    lo: u64,
) -> Result<(), ClipError> {
    let k = holes.partition_point(|h| h.1 <= p);
    for &(ho, he) in &holes[k..] {
        if ho >= n {
            break;
        }
        if ho != p || he > n {
            return Err(unwritten(chunk.offset + p.max(lo)));
        }
        p = he;
    }
    if n != align_up(p, align) {
        return Err(unwritten(chunk.offset + p.max(lo)));
    }
    Ok(())
}

/// The gap rule over the clip `[lo, hi)` (chunk offsets) of `Body` chunk
/// `c`: every gap between consecutive members (and before the first and
/// after the last) that meets the clip is explained ([`explain`]); after
/// the last member the chunk's tail up to its size is explained as a gap
/// before an item of alignment 1, and `[size, capacity)` is the slack
/// (the same capacity [`pads`]' views read). Returns the members met.
#[allow(clippy::too_many_arguments)]
fn gaps<R: Rows>(
    rows: &R,
    c: ChunkId,
    chunk: &Chunk,
    list: &[Member],
    holes: &[(u64, u64)],
    lo: u64,
    hi: u64,
) -> Result<Vec<Placed>, ClipError> {
    let (k, got) = span(rows, c, list, lo, hi)?;
    let meets_clip = |a: u64, b: u64| a < hi && lo < b && a < b;
    let mut p = if k > 0 { placed(rows, c, &list[k - 1])?.end() } else { 0 };
    for n in &got {
        if meets_clip(p, n.offset) {
            explain(chunk, holes, p, n.offset, n.align, lo)?;
        }
        p = n.end();
    }
    if let Some(m) = list.get(k + got.len()) {
        let n = placed(rows, c, m)?;
        if n.offset < p {
            return Err(internal(&format!(
                "member {} of chunk {} overlaps its predecessor",
                m.section.0, c.0
            )));
        }
        if meets_clip(p, n.offset) {
            explain(chunk, holes, p, n.offset, n.align, lo)?;
        }
    } else {
        if p > chunk.size {
            return Err(internal(&format!("chunk {}'s members end past its size", c.0)));
        }
        if meets_clip(p, chunk.size) {
            explain(chunk, holes, p, chunk.size, 1, lo)?;
        }
    }
    Ok(got)
}

/// The plan of `clips` (sorted, disjoint, non-empty ranges of the file):
/// inside the clips, the rows of the whole plan of the same rows; outside
/// them, gaps. The sections of each `Body` chunk a clip meets are
/// [`sections_in`]'s (from `rows` and `members`); `front` supplies each
/// one's bytes and relocations. Before any row is made, the gap rule
/// (ruling "After 255c2a" 2(ii), always on): every byte of a `Body` chunk
/// in a clip is a member's, a hole's, alignment padding before a member
/// or a hole, or the chunk's slack.
///
/// # Errors
///
/// [`ClipError::NotYet`] for a clip that meets a chunk the clip plan does
/// not build yet; [`ClipError::Internal`] for clips that are not sorted
/// and disjoint, a member `rows` does not place, a gap the rows do not
/// explain (`image: clip byte {offset:#x} written by no row`), a section
/// `front` does not supply (or supplies at another size), a structural
/// chunk the clip meets and `parts.structure` does not hold (or holds at
/// another size), and the plan check's own (I-27).
pub fn for_clips<R: Rows>(
    rows: &R,
    clips: &[Range<u64>],
    members: &Members,
    front: FrontOf<'_, '_>,
    parts: &ClipParts<'_>,
) -> Result<Plan, ClipError> {
    if clips.iter().any(|c| c.start >= c.end) || clips.windows(2).any(|w| w[0].end > w[1].start) {
        return Err(internal(&format!("clips {clips:x?} are not sorted, disjoint and non-empty")));
    }
    let chunks = rows.chunks();
    let file_size = chunks
        .iter()
        .filter(|(_, c)| c.in_file())
        .map(|(_, c)| c.offset + c.size)
        .max()
        .unwrap_or(0);
    let mut b = Builder::new(parts.target, file_size);
    let mut bodies: Vec<(ChunkId, &Chunk)> = Vec::new();
    for (id, c) in chunks.iter() {
        if !c.in_file() || c.size == 0 || !meets(clips, c.offset, c.offset + c.size) {
            continue;
        }
        let not_yet = match c.kind {
            ChunkKind::Body(_) => {
                bodies.push((id, c));
                continue;
            }
            ChunkKind::FileHeader | ChunkKind::ProgramHeaders | ChunkKind::SectionHeaders => {
                let bytes = parts.structure.get(&id).ok_or_else(|| {
                    internal(&format!("the structural chunk {} has no bytes", id.0))
                })?;
                if bytes.len() as u64 != c.size {
                    return Err(internal(&format!(
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
    bodies.sort_unstable_by_key(|(_, c)| c.offset);
    // The gap rule over every body chunk first; then the rows.
    let mut todo: Vec<(ChunkId, Placed)> = Vec::new();
    for &(id, c) in &bodies {
        let mut holes: Vec<(u64, u64)> = parts
            .holes
            .iter()
            .filter(|(h, _)| h.chunk == id)
            .map(|(h, _)| (h.offset, h.offset + h.size))
            .collect();
        holes.sort_unstable();
        let list = members.by_chunk.get(&id).map_or(&[][..], Vec::as_slice);
        let k = clips.partition_point(|r| r.end <= c.offset);
        for r in &clips[k..] {
            if r.start >= c.offset + c.size {
                break;
            }
            let lo = r.start.saturating_sub(c.offset);
            let hi = (r.end - c.offset).min(c.size);
            for p in gaps(rows, id, c, list, &holes, lo, hi)? {
                if todo.last().is_none_or(|l| l.1.section != p.section) {
                    todo.push((id, p));
                }
            }
        }
    }
    for (c, p) in todo {
        let Some(f) = front(p.section, (c, p.offset))? else {
            return Err(internal(&format!("section {}: no bytes for it", p.section.0)));
        };
        if f.size != p.size {
            return Err(internal(&format!(
                "section {}: {} bytes for its size {}",
                p.section.0, f.size, p.size
            )));
        }
        if f.name == b".eh_frame" {
            return Err(ClipError::NotYet(NotYet::EhFrame, c));
        }
        row_for(rows, p.section, (c, p.offset), &f, parts.proof, &mut b)?;
    }
    for (h, name) in parts.holes {
        let Some(c) = chunks.get(h.chunk) else {
            return Err(internal(&format!("a hole in no chunk {}", h.chunk.0)));
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
