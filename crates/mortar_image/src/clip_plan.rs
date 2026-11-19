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
//! WP-255c2c: a synthetic chunk of a planned kind ([`PLANNED`]: `.symtab`,
//! `.strtab`, `.symtab_shndx`, `.eh_frame`, `.eh_frame_hdr`, the build-id
//! note) is the whole plan's `Synth` row, routed to the fill's chunk list
//! by [`ClipParts::synth`]; the output `.eh_frame`'s row brings the input
//! `.eh_frame` sections whose kept records meet the clip
//! ([`ClipParts::eh_sections`]); a patch island is its bytes
//! ([`ClipParts::patches`]); an input `.eh_frame` placed in a body chunk is
//! an ordinary member ([`row_for`], as in the whole plan).
//!
//! What it does not build yet is refused by name ([`ClipError::NotYet`]):
//! a clip that meets a synthetic chunk of another kind, or a `-r` chunk;
//! never a silent zero. A plain link never calls it.
//!
//! [`check_written`] is §9.1's written-bitmap rule, which the clip filler
//! runs under `debug_assertions`: every clip byte is written by an explicit
//! row (a unit or a pad), or lies in a zero gap derivable from the chunks.

use std::collections::BTreeMap;
use std::ops::Range;

use mortar_arch::ProofRun;
use mortar_core::layout::{Chunk, ChunkKind, Hole};
use mortar_core::par::Pool;
use mortar_core::{ChunkId, Column, FileId, FragId, SectionId};
use mortar_diag::{Diagnostic, Kind};
use mortar_elf::SectionIndex;
use mortar_synth::chunks::SynthKind;

use crate::layout_plan::{ChunkView, SegView, SynthRow, hole_bytes, pads};
use crate::plan::{
    Builder, EhSection, MergeSource, MergedUnit, Pattern, Plan, RecRows, RelaTable, RelocSection,
    SynthUnit, Unit,
};
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
    /// Every in-file `Merged` chunk with bytes ([`MergedMembers`]).
    pub merged: BTreeMap<ChunkId, MergedMembers>,
}

/// One placed fragment of a `Merged` chunk as [`MergedMembers`] lists it:
/// the fragment, the merge section its bytes are copied from, its size and
/// its alignment (the index's data: [`Rows`] gives neither the section nor
/// the size of a fragment).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FragMember {
    pub frag: FragId,
    pub section: SectionId,
    pub size: u64,
    pub align: u64,
}

/// A `Merged` chunk's index (I-38): its unit (the chunk's index in the
/// fragment order, `MergedUnit::chunk`, and its output section's name) and
/// every placed fragment, in offset order (where each one is comes from
/// [`Rows::frag_place`]).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MergedMembers {
    pub order: u32,
    pub name: Box<[u8]>,
    pub frags: Vec<FragMember>,
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
    /// Where merge section `s`'s bytes are (the fragments a clip meets are
    /// copied from it); `None` is an error of [`for_clips`].
    pub merge_source: &'p dyn Fn(SectionId) -> Result<Option<MergeSource>, Diagnostic>,
    /// WP-255c2c: every in-file synthetic chunk with file bytes, its kind
    /// and its row (`SynthRow::index` is its index in the chunk list the
    /// fill is given, `crate::fill::Image::chunks`). A synthetic chunk a
    /// clip meets that this does not hold, or of a kind not in
    /// [`PLANNED`], is [`NotYet::Synth`].
    pub synth: &'p BTreeMap<ChunkId, (SynthKind, SynthRow)>,
    /// WP-255c2c (S-52): every live input `.eh_frame` section with
    /// relocations that has a kept record meeting `[a, b)` (offsets in the
    /// output `.eh_frame`): the section, its file, its index there and its
    /// relocation table, in any order. A section left out is a relocation
    /// the fill does not apply (`eh::relocate` skips a section the plan
    /// does not list): the list is the caller's producer's, never
    /// assembled ad hoc.
    #[allow(clippy::type_complexity)]
    pub eh_sections:
        &'p dyn Fn(
            Range<u64>,
        )
            -> Result<Vec<(SectionId, FileId, SectionIndex, RelaTable)>, Diagnostic>,
    /// WP-255c2c: the patch islands' bytes (`link::patches`: the thunks;
    /// an erratum patch's words are the fill's fixups), whole, for each
    /// island a clip meets.
    pub patches: &'p BTreeMap<ChunkId, Box<[u8]>>,
    /// The build id's descriptor in the file (`Plan::sealed`), when there
    /// is one.
    pub sealed: Option<&'p Range<u64>>,
}

/// The synthetic kinds the clip plan plans (WP-255c2c): those every taken
/// update hop dirties. Another kind under a clip is [`NotYet::Synth`].
pub const PLANNED: [SynthKind; 6] = [
    SynthKind::Symtab,
    SynthKind::Strtab,
    SynthKind::SymtabShndx,
    SynthKind::EhFrame,
    SynthKind::EhFrameHdr,
    SynthKind::NoteGnuBuildId,
];

impl std::fmt::Debug for ClipParts<'_> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ClipParts")
            .field("chunk_views", &self.chunk_views.len())
            .field("holes", &self.holes.len())
            .field("structure", &self.structure.len())
            .field("synth", &self.synth.len())
            .field("patches", &self.patches.len())
            .finish_non_exhaustive()
    }
}

/// What a clip meets that the clip plan does not build yet (255c2b).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NotYet {
    /// A synthetic chunk (§13) of a kind not in [`PLANNED`], or one
    /// [`ClipParts::synth`] does not hold.
    Synth,
    /// A patch island (thunks, erratum patches). Not refused since
    /// WP-255c2c.
    Patches,
    /// An input `.eh_frame` section placed in a body chunk. Not refused
    /// since WP-255c2c.
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

/// Fragment `m` of merged chunk `c` placed by `rows`.
fn frag_placed<R: Rows>(rows: &R, c: ChunkId, m: &FragMember) -> Result<Placed, ClipError> {
    match rows.frag_place(m.frag) {
        Some((pc, offset)) if pc == c && rows.merged_chunk(m.section) == Some(c) => {
            Ok(Placed { section: m.section, offset, size: m.size, align: m.align.max(1) })
        }
        _ => Err(internal(&format!("fragment {} of chunk {} has no place in it", m.frag.0, c.0))),
    }
}

/// Every placed fragment of merged chunk `c` that meets `[lo, hi)`
/// (offsets in the chunk), in offset order, each as a [`Placed`] of its
/// merge section (WP-255c2b; [`sections_in`]'s twin). A chunk `members`
/// does not list has none.
///
/// # Errors
///
/// [`ClipError::Internal`] for a fragment `rows` does not place in `c`,
/// or fragments out of order or overlapping.
pub fn frags_in<R: Rows>(
    rows: &R,
    members: &Members,
    c: ChunkId,
    range: Range<u64>,
) -> Result<Vec<(FragId, Placed)>, ClipError> {
    let Some(m) = members.merged.get(&c) else { return Ok(Vec::new()) };
    let at = |i: usize| frag_placed(rows, c, &m.frags[i]);
    let got = span(m.frags.len(), &at, c, range.start, range.end)?.1;
    Ok(got.into_iter().map(|(i, p)| (m.frags[i].frag, p)).collect())
}

/// Item `i` of a chunk's index, placed by the rows.
type At<'a> = &'a dyn Fn(usize) -> Result<Placed, ClipError>;

/// The index `k` of the first of the `len` items of chunk `c` (in offset
/// order, placed by `at`) that ends after `lo`, and the items from `k` on
/// that meet `[lo, hi)` (chunk offsets) with their indices, checked in
/// order and disjoint. O(log items + items met).
fn span(
    len: usize,
    at: At<'_>,
    c: ChunkId,
    lo: u64,
    hi: u64,
) -> Result<(usize, Vec<(usize, Placed)>), ClipError> {
    let (mut k, mut e) = (0, len);
    while k < e {
        let mid = k + (e - k) / 2;
        if at(mid).is_ok_and(|p| p.end() <= lo) {
            k = mid + 1;
        } else {
            e = mid;
        }
    }
    let mut out = Vec::new();
    let mut prev = if k > 0 { at(k - 1)?.end() } else { 0 };
    for i in k..len {
        let p = at(i)?;
        if p.offset < prev || (p.size > 0 && p.offset < hi && p.end() <= lo) {
            return Err(internal(&format!(
                "item {i} of chunk {} is out of order or overlaps its predecessor",
                c.0
            )));
        }
        if p.offset >= hi {
            break;
        }
        prev = p.end();
        if p.size > 0 {
            out.push((i, p));
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
    let at = |i: usize| placed(rows, c, &list[i]);
    Ok(span(list.len(), &at, c, range.start, range.end)?.1.into_iter().map(|x| x.1).collect())
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

/// The gap rule over the clip `[lo, hi)` (chunk offsets) of chunk `c`
/// whose `len` items (members of a `Body` chunk, fragments of a `Merged`
/// one) `at` places: every gap between consecutive items (and before the
/// first and after the last) that meets the clip is explained
/// ([`explain`]); after the last item the chunk's tail up to its size is
/// explained as a gap before an item of alignment 1, and `[size,
/// capacity)` is the slack (the same capacity [`pads`]' views read).
/// Returns the items met, with their indices.
fn gaps(
    len: usize,
    at: At<'_>,
    (c, chunk): (ChunkId, &Chunk),
    holes: &[(u64, u64)],
    lo: u64,
    hi: u64,
) -> Result<Vec<(usize, Placed)>, ClipError> {
    let (k, got) = span(len, at, c, lo, hi)?;
    let meets_clip = |a: u64, b: u64| a < hi && lo < b && a < b;
    let mut p = if k > 0 { at(k - 1)?.end() } else { 0 };
    for (_, n) in &got {
        if meets_clip(p, n.offset) {
            explain(chunk, holes, p, n.offset, n.align, lo)?;
        }
        p = p.max(n.end());
    }
    let next = got.last().map_or(k, |l| l.0 + 1);
    // The next item with bytes (a zero-sized one explains nothing).
    let mut j = next;
    while j < len && at(j)?.size == 0 {
        j += 1;
    }
    if j < len {
        let n = at(j)?;
        if n.offset < p {
            return Err(internal(&format!("item {j} of chunk {} overlaps its predecessor", c.0)));
        }
        if meets_clip(p, n.offset) {
            explain(chunk, holes, p, n.offset, n.align, lo)?;
        }
    } else {
        if p > chunk.size {
            return Err(internal(&format!("chunk {}'s items end past its size", c.0)));
        }
        if meets_clip(p, chunk.size) {
            explain(chunk, holes, p, chunk.size, 1, lo)?;
        }
    }
    Ok(got)
}

/// The items of chunk `c` (`len` of them, placed by `at`) that the clips
/// meet, each clip's gaps checked by [`gaps`] with the chunk's holes.
fn met(
    clips: &[Range<u64>],
    (id, c): (ChunkId, &Chunk),
    parts: &ClipParts<'_>,
    len: usize,
    at: At<'_>,
) -> Result<Vec<Placed>, ClipError> {
    let mut holes: Vec<(u64, u64)> = parts
        .holes
        .iter()
        .filter(|(h, _)| h.chunk == id)
        .map(|(h, _)| (h.offset, h.offset + h.size))
        .collect();
    holes.sort_unstable();
    let mut out = Vec::new();
    let k = clips.partition_point(|r| r.end <= c.offset);
    for r in &clips[k..] {
        if r.start >= c.offset + c.size {
            break;
        }
        let lo = r.start.saturating_sub(c.offset);
        let hi = (r.end - c.offset).min(c.size);
        out.extend(gaps(len, at, (id, c), &holes, lo, hi)?.into_iter().map(|x| x.1));
    }
    Ok(out)
}

/// The bytes of fixed chunk `id` (a structural chunk, or a patch island)
/// from `map`, at its size.
fn fixed(
    map: &BTreeMap<ChunkId, Box<[u8]>>,
    what: &str,
    id: ChunkId,
    c: &Chunk,
) -> Result<Box<[u8]>, ClipError> {
    let bytes =
        map.get(&id).ok_or_else(|| internal(&format!("the {what} {} has no bytes", id.0)))?;
    if bytes.len() as u64 != c.size {
        return Err(internal(&format!(
            "the {what} {} has {} bytes for {}",
            id.0,
            bytes.len(),
            c.size
        )));
    }
    Ok(bytes.clone())
}

/// The row of synthetic chunk `id` named `name` (WP-255c2c), or `None`
/// when the clip plan does not plan it: [`ClipParts::synth`] does not hold
/// it, or its kind is not in [`PLANNED`].
///
/// # Errors
///
/// A planned kind whose name is not the chunk's (the caller's list is not
/// this placement's).
fn synth_row(
    parts: &ClipParts<'_>,
    id: ChunkId,
    name: &[u8],
) -> Result<Option<(SynthKind, SynthRow)>, ClipError> {
    let Some(&(kind, row)) = parts.synth.get(&id) else { return Ok(None) };
    if !PLANNED.contains(&kind) {
        return Ok(None);
    }
    if kind.name(false) != name {
        return Err(internal(&format!(
            "synthetic chunk {} is {} and listed as {}",
            id.0,
            String::from_utf8_lossy(name),
            String::from_utf8_lossy(kind.name(false))
        )));
    }
    Ok(Some((kind, row)))
}

/// The `Synth` rows of the synthetic chunks the clips meet (WP-255c2c):
/// the whole plan's row of each (its whole extent: a chunk is one unit),
/// and for the output `.eh_frame` the input sections whose kept records
/// meet the bytes the fill writes of it (the clips in it; the whole chunk
/// when it is written whole, I-104), each once.
fn synth_rows(
    clips: &[Range<u64>],
    synths: &[(ChunkId, &Chunk, &'static [u8], SynthRow)],
    parts: &ClipParts<'_>,
    b: &mut Builder,
) -> Result<(), ClipError> {
    let mut eh: BTreeMap<SectionId, (FileId, SectionIndex, RelaTable)> = BTreeMap::new();
    for &(id, c, name, row) in synths {
        let n = b.name(name);
        let unit = SynthUnit { chunk: row.index, id, name: n, whole: row.whole, eh: row.eh };
        b.synth(c.offset, c.size, unit);
        if !row.eh {
            continue;
        }
        let end = c.offset + c.size;
        let pieces: Vec<Range<u64>> = if row.whole {
            std::iter::once(0..c.size).collect()
        } else {
            clips
                .iter()
                .filter(|r| r.start < end && c.offset < r.end)
                .map(|r| r.start.max(c.offset) - c.offset..r.end.min(end) - c.offset)
                .collect()
        };
        for p in pieces {
            for (s, file, index, table) in (parts.eh_sections)(p)? {
                eh.entry(s).or_insert((file, index, table));
            }
        }
    }
    for (section, (file, index, table)) in eh {
        let rs = RelocSection { section, rows: RecRows::Rela(table), proof: Box::new([]) };
        let relocs = b.relocs(rs);
        b.eh_section(EhSection { section, file, index: index.0, relocs });
    }
    Ok(())
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
#[allow(
    clippy::too_many_lines,
    reason = "the clip plan's arms in order: members, merged, synthetic, structure"
)]
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
    let mut merged: Vec<(ChunkId, &Chunk)> = Vec::new();
    let mut synths: Vec<(ChunkId, &Chunk, &'static [u8], SynthRow)> = Vec::new();
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
                b.bytes(c.offset, fixed(parts.structure, "structural chunk", id, c)?);
                continue;
            }
            ChunkKind::Patches(_) => {
                b.bytes(c.offset, fixed(parts.patches, "patch island", id, c)?);
                continue;
            }
            ChunkKind::Merged(_) => {
                merged.push((id, c));
                continue;
            }
            ChunkKind::Synthetic(s) => match synth_row(parts, id, s.name)? {
                Some((_, row)) => {
                    synths.push((id, c, s.name, row));
                    continue;
                }
                None => NotYet::Synth,
            },
            ChunkKind::Group(_) | ChunkKind::Relocs(_) => NotYet::Relocatable,
        };
        return Err(ClipError::NotYet(not_yet, id));
    }
    bodies.sort_unstable_by_key(|(_, c)| c.offset);
    // The gap rule over every body chunk first; then the rows.
    let mut todo: Vec<(ChunkId, Placed)> = Vec::new();
    for &(id, c) in &bodies {
        let list = members.by_chunk.get(&id).map_or(&[][..], Vec::as_slice);
        let at = |i: usize| placed(rows, id, &list[i]);
        for p in met(clips, (id, c), parts, list.len(), &at)? {
            if todo.last().is_none_or(|l| l.1.section != p.section) {
                todo.push((id, p));
            }
        }
    }
    // The same over every merged chunk: the fragments the clips meet, and
    // the merge sections they are copied from.
    let mut units: Vec<(ChunkId, &MergedMembers)> = Vec::new();
    let mut sources: std::collections::BTreeSet<SectionId> = std::collections::BTreeSet::new();
    for &(id, c) in &merged {
        let Some(m) = members.merged.get(&id) else {
            return Err(internal(&format!("merged chunk {} has no fragment index", id.0)));
        };
        let at = |i: usize| frag_placed(rows, id, &m.frags[i]);
        sources.extend(met(clips, (id, c), parts, m.frags.len(), &at)?.iter().map(|p| p.section));
        units.push((id, m));
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
        row_for(rows, p.section, (c, p.offset), &f, parts.proof, &mut b)?;
    }
    synth_rows(clips, &synths, parts, &mut b)?;
    for (id, m) in units {
        let c = &chunks[id];
        let name = b.name(&m.name);
        b.merged(c.offset, c.size, MergedUnit { chunk: m.order, name });
    }
    for s in sources {
        let Some(src) = (parts.merge_source)(s)? else {
            return Err(internal(&format!("merge section {}: no bytes for it", s.0)));
        };
        b.merge_source(src);
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
    if let Some(s) = parts.sealed {
        b.sealed(s.clone());
    }
    Ok(b.finish(crate::IMAGE_BLOCK, &Pool::sequential())?)
}

/// c2b-3 (ruling "After 255c2b" item 3): before the first row, every
/// in-file `Body` chunk a clip meets lists in `members` exactly as many
/// members as `count` gives for it, a count from a source INDEPENDENT of
/// the member list (a dropped member smaller than the next item's
/// alignment is otherwise unseen by the gap rule). `count` answering
/// `None` (no independent count) refuses too. O(chunks + clips).
///
/// # Errors
///
/// `internal-error (image: clip plan: chunk {c}: the member index lists
/// {n} members, the independent count {m})`, or `(... no independent
/// member count)`.
pub fn check_member_counts(
    members: &Members,
    chunks: &Column<ChunkId, Chunk>,
    clips: &[Range<u64>],
    count: &dyn Fn(ChunkId) -> Option<usize>,
) -> Result<(), ClipError> {
    for (id, c) in chunks.iter() {
        if !c.in_file()
            || c.size == 0
            || !matches!(c.kind, ChunkKind::Body(_))
            || !meets(clips, c.offset, c.offset + c.size)
        {
            continue;
        }
        let n = members.by_chunk.get(&id).map_or(0, Vec::len);
        match count(id) {
            Some(m) if m == n => {}
            Some(m) => {
                return Err(internal(&format!(
                    "chunk {}: the member index lists {n} members, the independent count {m}",
                    id.0
                )));
            }
            None => {
                return Err(internal(&format!("chunk {}: no independent member count", id.0)));
            }
        }
    }
    Ok(())
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
