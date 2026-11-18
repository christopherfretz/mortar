//! Refill and image in an update (`spec/incr.md` IN-73, IN-75; IN-70 step
//! U9): the refill set's units relocated again, the synthetic chunks and
//! headers rewritten, the dirty ranges and their bytes. WP-213 (U-1): the
//! type [`RefillError`] and the entry point [`refill`]; filled by U-6
//! (WP-218), which also fills `crate::map::update_map`.
//!
//! **What the refill takes from where** (WP-218's record, §R):
//! - `link` is the image's view of the update's output `P_U`: its
//!   `layout.placement` is the MATERIALIZED placement (the join fills it
//!   through `LayoutDelta::place_of`/`holes_of`, WP-216 R-3). Every byte the
//!   image fills is a byte of `P_U`.
//! - `layout` (U-4) says WHAT changed: the files placed here, the moved,
//!   revived sections, the holes, the growth steps, `.eh_frame`'s ranges,
//!   the fragments and the slots.
//! - `previous` is the output as it is: the moved bytes are COPIED from it
//!   (a section a growth step moved; `.eh_frame`'s unchanged records, whose
//!   PC-relative fields are then patched from the previous bytes alone,
//!   never from the file's input: WP-239 R-3, IN-34a "Pad and move").
//! - `symtab` (U-5) is written as given; `set` (U-3) adds its units and
//!   chunks.
//!
//! Every byte that differs between `previous` and `P_U` lies in a returned
//! range (IN-75); a range whose bytes equal the previous output's is
//! dropped, so an empty delta writes no byte.

use std::collections::BTreeMap;
use std::fs::File;
use std::ops::Range;
use std::os::unix::fs::FileExt as _;

use mortar_core::incr::{
    DirtyRange, IncrReason, LayoutDelta, RefillSet, StoredLayout, SymtabPlan, Unimplemented,
};
use mortar_core::layout::{ChunkKind, Placement};
use mortar_core::{ChunkId, FileId};
use mortar_synth::chunks::SynthKind;

use crate::IMAGE_BLOCK;
use crate::fill::Failed;
use crate::link::Link;

/// How a refill ends without bytes (WP-213: WP-211 §C's `Failed` cannot
/// carry "until it lands", nor IN-75's `input-changed`).
#[derive(Debug)]
pub enum RefillError {
    /// A §7 reason: the update falls back (`input-changed`, IN-75;
    /// `update-not-implemented (image)`, `(eh)`).
    Fallback(IncrReason),
    /// The link fails as a full link would (a relocation error of a
    /// refilled unit, IN-5).
    Failed(Failed),
}

/// What one refill did (WP-218: the O(change) counts the tests assert).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Counts {
    /// The 1 MiB blocks the ranges the image fills touch (I-95: the unit
    /// of the leaf hash, not of the fill), the pieces it filled (each
    /// range cut at the plan's jobs, I-31) and their bytes: the bytes
    /// wanted, a whole synthetic chunk a range touches widened to the
    /// chunk (I-104).
    pub windows: u64,
    pub jobs: u64,
    pub filled: u64,
    /// Bytes copied from the previous output (sections a growth step
    /// moved).
    pub copied: u64,
    /// `.eh_frame` records walked in the previous output, and FDE fields
    /// patched.
    pub eh_records: u64,
    pub eh_patched: u64,
    /// `.symtab` entries written (entries, fillers, moves, remaps).
    pub symtab_entries: u64,
    /// Inputs rechecked (twice each: before the fill and after it).
    pub rechecked: u64,
    /// Bytes of the previous output read for the leaves (IN-75, I-76),
    /// each block once: the blocks a dirty range or the note touches, and
    /// those whose length the new size changes; every other leaf is the
    /// stored one. The read serves both the check of the stored leaf
    /// (ruling F8) and the new leaf.
    pub build_id_read: u64,
    /// Leaves recomputed.
    pub leaves_hashed: u64,
    /// The ranges returned and their bytes.
    pub ranges: u64,
    pub dirty: u64,
}

/// The size of the update's output: the end of its last chunk in the
/// file (the section header table's, `spec/image.md` I-1). U-7 takes it
/// as `Pair::size` from the join (WP-218: the result of [`refill`] does
/// not carry it).
#[must_use]
pub fn output_size(link: &Link<'_, '_>) -> u64 {
    link.layout
        .placement
        .chunks
        .iter()
        .filter(|(_, c)| c.in_file())
        .map(|(_, c)| c.offset + c.size)
        .max()
        .unwrap_or(0)
}

/// What [`refill`] gives the join (WP-218; WP-211 §C, as ruled on the
/// hand-back: the leaves in and out).
#[derive(Debug, Clone, Default)]
pub struct Refill {
    /// The dirty ranges, sorted and disjoint (IN-75).
    pub ranges: Vec<DirtyRange>,
    /// Their bytes, concatenated in range order.
    pub bytes: Vec<u8>,
    /// The new output's leaves, whole (`StatePatch::leaves`; `LEAVES`,
    /// IN-18e): one per block of the new size (I-72), each digest
    /// zero-filled to 32 bytes; a stored leaf where no byte of its block
    /// changed.
    pub leaves: Vec<[u8; 32]>,
    /// The build id written into the note (`None` without one).
    pub build_id: Option<Vec<u8>>,
    pub counts: Counts,
}

/// Refill the update's units against `previous` (U1's descriptor of the
/// output) and give the dirty ranges, sorted and disjoint, with their
/// bytes concatenated in range order, and the new leaves and build id
/// (WP-211 §C; IN-73, IN-75). `leaves` are the stored `LEAVES` of the
/// previous output, whole. `stored` gives the one row the placement
/// does not hold: an unchanged file's stored `.eh_frame` range
/// (`eh_range_of`), read for each range a whole rewrite moves. Called by
/// the join (IN-70 U9).
///
/// # Errors
/// [`RefillError`]: `input-changed` for an input that is not the file the
/// front read (IN-75); `state-corrupt` for `leaves` not one per block of
/// the previous output; `update-not-implemented (eh)` for an
/// `.eh_frame` encoding IN-34a does not move (decided before W1);
/// `update-not-implemented (image)` for what this writer does not do
/// (an output section added or removed, a 64-bit DWARF record).
#[allow(clippy::too_many_lines, reason = "IN-75's list of dirty sources, in order")]
pub fn refill(
    link: &Link<'_, '_>,
    layout: &LayoutDelta,
    symtab: &SymtabPlan,
    set: &RefillSet,
    stored: &StoredLayout<'_>,
    previous: &File,
    leaves: &[[u8; 32]],
) -> Result<Refill, RefillError> {
    let mut counts = Counts::default();
    let pl = &link.layout.placement;
    let prev = Previous::new(previous)?;
    // The stored leaves are the previous output's (I-72), checked before
    // any byte is used.
    if leaves.len() as u64 != mortar_digest::leaf_count(prev.len) {
        return Err(RefillError::Fallback(IncrReason::StateCorrupt(format!(
            "LEAVES: {} leaves for an output of {} bytes",
            leaves.len(),
            prev.len
        ))));
    }
    // IN-75: every input is checked against the file the front read
    // before its first byte is used.
    recheck(link, &mut counts)?;
    let old = prev.sections()?;
    let new = new_sections(pl);
    if old.class64 != new.class64 || old.rows.len() != new.rows.len() {
        return Err(not_implemented(Unimplemented::Image));
    }
    let names = prev.names(&old)?;
    let new_names = new_names(link);
    for (i, n) in names.iter().enumerate().skip(1) {
        if new_names.get(i).is_some_and(|m| m.as_deref().is_some_and(|m| m != n.as_slice())) {
            return Err(not_implemented(Unimplemented::Image));
        }
    }
    let shift = Shift::new(&old, &new);
    let eh_index = names.iter().position(|n| n == b".eh_frame");
    // U9, before W1: `.eh_frame`'s encodings (IN-34a, WP-239 R-3).
    let eh_moves = !shift.moved.is_empty();
    let eh_records = match eh_index {
        Some(i) if layout.eh.is_some() || eh_moves => {
            let r = &old.rows[i];
            let bytes = prev.read(r.offset..r.offset + r.size)?;
            Some(walk_eh(&bytes, &mut counts)?)
        }
        _ => None,
    };
    check_moved_code(link, layout)?;

    let mut overlay = Overlay::default();
    // 1. Bytes a growth step (or any output section move) moved: copied
    // from the previous output.
    for &i in &shift.moved {
        let (o, n) = (&old.rows[i], &new.rows[i]);
        if !n.in_file || !o.in_file || n.offset == o.offset {
            continue;
        }
        let len = o.size.min(n.size);
        let bytes = prev.read(o.offset..o.offset + len)?;
        counts.copied += len;
        overlay.put(n.offset, bytes);
    }
    // 2. What the image fills.
    let mut want: Vec<Range<u64>> = Vec::new();
    let in_file = |c: ChunkId| pl.chunks.get(c).filter(|c| c.in_file());
    let place = |s: mortar_core::SectionId| -> Option<Range<u64>> {
        let (c, at) = pl.place.get(s)?.get()?;
        let chunk = in_file(c)?;
        let size = section_size(link, s)?;
        let lo = chunk.offset + at;
        Some(lo..lo + size)
    };
    for u in &set.units {
        want.extend(place(u.section));
    }
    for c in &set.chunks {
        want.extend(in_file(c.chunk).map(|k| k.offset..k.offset + k.capacity));
    }
    for &f in &layout.files {
        if let Some(r) = link.resolution.section_base.get(f) {
            for s in r.iter() {
                want.extend(place(s));
            }
        }
    }
    for &s in layout.moved.iter().chain(&layout.revived) {
        want.extend(place(s));
    }
    let hole_range = |h: &mortar_core::layout::Hole| {
        in_file(h.chunk).map(|c| c.offset + h.offset..c.offset + h.offset + h.size)
    };
    // A dead string piece's bytes are left as written (`DeadFragment`,
    // L-147): not refilled.
    let refilled_hole =
        |h: &&mortar_core::layout::Hole| h.kind != mortar_core::layout::HoleKind::DeadFragment;
    want.extend(layout.holes.iter().filter(refilled_hole).filter_map(hole_range));
    want.extend(
        pl.holes
            .iter()
            .filter(|h| layout.hole_chunks.binary_search(&h.chunk).is_ok())
            .filter(refilled_hole)
            .filter_map(hole_range),
    );
    if !layout.frags.is_empty() {
        want.extend(frag_ranges(link, layout));
    }
    let synth =
        |k: SynthKind| link.synth.chunks.iter().find(|c| c.0 == k).and_then(|c| in_file(c.1));
    let whole = |c: &mortar_core::layout::Chunk| c.offset..c.offset + c.capacity.max(c.size);
    // Seam 6 (IN-72 item 7): appended and revived slots written; a
    // recounted one changes no byte.
    if !layout.slots_appended.is_empty()
        || !layout.slots_dead.is_empty()
        || !layout.slots_revived.is_empty()
    {
        for k in [SynthKind::Got, SynthKind::GotPlt, SynthKind::Plt, SynthKind::RelaPlt] {
            want.extend(synth(k).map(whole));
        }
    }
    let eh_chunk = synth(SynthKind::EhFrame);
    let mut eh_rewritten: Vec<Range<u64>> = Vec::new();
    if let (Some(eh), Some(c)) = (&layout.eh, eh_chunk) {
        if eh.whole {
            eh_rewritten.push(whole(c));
        } else {
            for r in &eh.ranges {
                let lo = c.offset + r.offset;
                eh_rewritten.push(lo..lo + r.capacity);
            }
        }
        want.extend(eh_rewritten.iter().cloned());
    }
    if layout.eh.is_some() || eh_moves {
        want.extend(synth(SynthKind::EhFrameHdr).map(whole));
    }
    if eh_moves {
        // Every synthetic chunk but those written below from the previous
        // bytes and the plans: their contents may hold a moved address.
        for (k, id) in link.synth.chunks {
            if matches!(
                k,
                SynthKind::EhFrame
                    | SynthKind::Symtab
                    | SynthKind::Strtab
                    | SynthKind::SymtabShndx
                    | SynthKind::NoteGnuBuildId
            ) {
                continue;
            }
            want.extend(in_file(*id).map(whole));
        }
    }
    want.extend(Shift::grown_tails(&old, &new));
    let headers: Vec<Range<u64>> = pl
        .chunks
        .iter()
        .filter(|(_, c)| {
            matches!(
                c.kind,
                ChunkKind::FileHeader | ChunkKind::ProgramHeaders | ChunkKind::SectionHeaders
            )
        })
        .map(|(_, c)| c.offset..c.offset + c.size)
        .collect();
    if eh_moves {
        // The padding around each moved section, up to its neighbours:
        // another section, or the section header table after the last
        // one (a moved section changes that table's fields, so it is
        // written whole whatever is joined to it).
        let table: Vec<Range<u64>> = pl
            .chunks
            .iter()
            .filter(|(_, c)| c.kind == ChunkKind::SectionHeaders)
            .map(|(_, c)| c.offset..c.offset + c.size)
            .collect();
        want.extend(shift.gaps(&new, &table));
    }
    want.extend(headers.iter().cloned());
    let want = normalize(want);
    // I-95 (WP-255a): the image fills the ranges themselves; the 1 MiB
    // block is the unit of the leaf hash, not of the fill.
    counts.windows = windows_of(&want).len() as u64;
    let refilled = crate::link::refill_ranges(link, &want).map_err(RefillError::Failed)?;
    counts.jobs = refilled.jobs.len() as u64;
    counts.filled = refilled.jobs.iter().map(|j| j.1.len() as u64).sum();
    recheck(link, &mut counts)?;
    for r in &want {
        let bytes = slice_of(&refilled.jobs, r).ok_or(RefillError::Fallback(
            IncrReason::UpdateNotImplemented(Some(Unimplemented::Image)),
        ))?;
        // A header table is written whole when any field changed, never
        // otherwise (IN-75).
        if headers.contains(r) && prev.read(r.clone())? == bytes {
            continue;
        }
        overlay.put(r.start, bytes);
    }
    // A retired slot's words are zero (`HoleKind::DeadSlot`, IN-72 item
    // 7): the hole U-4 gives, over what the image filled.
    for h in layout.holes.iter().filter(|h| h.kind == mortar_core::layout::HoleKind::DeadSlot) {
        if let Some(r) = hole_range(h) {
            overlay.put(r.start, vec![0; (r.end - r.start) as usize]);
        }
    }
    // 3. `.eh_frame`'s unchanged records: their PC-relative fields moved
    // with the code (IN-34a "Pad and move"), from the previous bytes.
    if let (Some(i), Some(records), true) = (eh_index, &eh_records, eh_moves) {
        let (o, n) = (&old.rows[i], &new.rows[i]);
        let bytes = prev.read(o.offset..o.offset + o.size)?;
        for p in patch_eh(&bytes, records, o.addr, n.addr, &shift, &mut counts)? {
            let at = n.offset + p.0;
            if eh_rewritten.iter().any(|r| r.contains(&at)) {
                continue;
            }
            overlay.put(at, p.1.to_vec());
        }
    }
    // `.eh_frame`'s unchanged ranges a whole rewrite moves (WP-243's
    // shape: no record offsets): from the previous bytes at the stored
    // offset, over what the image filled. A range with offsets is the
    // image's (the new records); a changed file not listed has none.
    if let (Some(eh), Some(c), Some(i)) = (&layout.eh, eh_chunk, eh_index) {
        let o = &old.rows[i];
        for r in eh.ranges.iter().filter(|r| r.records.is_empty() && r.capacity > 0) {
            let Some(sr) = stored.eh_range_of(r.file).map_err(RefillError::Fallback)? else {
                return Err(not_implemented(Unimplemented::Eh));
            };
            if u64::from(sr.capacity) != r.capacity {
                return Err(not_implemented(Unimplemented::Eh));
            }
            let lo = o.offset + u64::from(sr.offset);
            let bytes = prev.read(lo..lo + r.capacity)?;
            counts.copied += r.capacity;
            let moved = move_eh_range(
                &bytes,
                o.addr + u64::from(sr.offset),
                c.addr.unwrap_or(0) + r.offset,
                sr.fdes,
                &shift,
                &mut counts,
            )?;
            overlay.put(c.offset + r.offset, moved);
        }
    }
    // 4. `.symtab` and `.strtab` from U-5's plan (IN-49a).
    write_symtab(&prev, &old, &new, &names, symtab, &shift, &mut overlay, &mut counts)?;
    // 5. The ranges, those equal to the previous output dropped.
    let file_size = refilled.file_size;
    let dirty = differing(&prev, overlay)?;
    // 6. The leaves (IN-75, I-76): a block a dirty range or the note
    // touches is hashed again from the new bytes, as is a block whose
    // length the new size changes; every other leaf is the stored one.
    let w = IMAGE_BLOCK;
    let f = refilled.leaf_fn;
    let sealed = refilled.sealed.clone().filter(|_| refilled.build_id.is_some());
    let (old_n, new_n) = (leaves.len() as u64, mortar_digest::leaf_count(file_size));
    let mut blocks: Vec<u64> = Vec::new();
    for (at, bytes) in &dirty {
        blocks.extend(at / w..(at + bytes.len() as u64).div_ceil(w));
    }
    if let Some(r) = &sealed {
        blocks.extend(r.start / w..r.end.div_ceil(w));
    }
    if file_size != prev.len {
        blocks.extend(old_n.saturating_sub(1)..new_n);
        blocks.extend(new_n.saturating_sub(1)..new_n);
    }
    blocks.retain(|&k| k < new_n);
    blocks.sort_unstable();
    blocks.dedup();
    let mut now = Overlay::default();
    for (at, bytes) in &dirty {
        now.put(*at, bytes.clone());
    }
    let mut new_leaves: Vec<[u8; 32]> =
        (0..new_n).map(|k| leaves.get(k as usize).copied().unwrap_or([0; 32])).collect();
    // The previous output's note, where its stored leaves zeroed it
    // (I-72): the new one's place, moved with its section if a growth
    // step moved it.
    let old_sealed = sealed.as_ref().map(|s| {
        let i = names.iter().position(|n| n == b".note.gnu.build-id");
        match i.and_then(|i| Some((old.rows.get(i)?, new.rows.get(i)?))) {
            Some((o, n)) if n.offset <= s.start && s.end <= n.offset + n.size => {
                s.start - n.offset + o.offset..s.end - n.offset + o.offset
            }
            _ => s.clone(),
        }
    });
    for &k in &blocks {
        let r = mortar_digest::leaf_range(file_size, k);
        // ONE read of the block (WP-255a): the previous bytes, as long as
        // the longer of its old and new lengths.
        let old_r = (k < old_n).then(|| mortar_digest::leaf_range(prev.len, k));
        let end = old_r.as_ref().map_or(r.end, |o| o.end.max(r.end));
        let mut win = prev.read(r.start..end)?;
        counts.build_id_read += win.len() as u64;
        // Ruling F8 (IN-75): before its first write, a block the update
        // dirties is the block its stored leaf hashes (the note zeroed as
        // the leaf rule says, I-72/I-76); else the previous output is not
        // the one the state describes: `output-changed`.
        if let Some(o) = &old_r {
            check_leaf(&mut win, o, old_sealed.as_ref(), f, &leaves[k as usize], k)?;
        }
        win.truncate((r.end - r.start) as usize);
        now.read_over(r.start, &mut win);
        if let Some(s) = &sealed {
            zero(&mut win, r.start, s);
        }
        counts.leaves_hashed += 1;
        let mut d = [0u8; 32];
        let l = mortar_digest::leaf(f, &win);
        d[..l.as_bytes().len()].copy_from_slice(l.as_bytes());
        new_leaves[k as usize] = d;
    }
    let mut build_id = None;
    let dirty = match (refilled.build_id, &sealed) {
        (Some(style), Some(sealed)) => {
            let mut kept = Vec::with_capacity(new_leaves.len());
            for (k, d) in new_leaves.iter().enumerate() {
                let leaf = mortar_digest::Leaf::kept(f, file_size, k as u64, d)
                    .ok_or_else(|| not_implemented(Unimplemented::Image))?;
                kept.push((k as u64, leaf));
            }
            let id = crate::digest::build_id(style, file_size, &kept)
                .map_err(|_| not_implemented(Unimplemented::Image))?;
            let mut note = id.clone();
            note.resize((sealed.end - sealed.start) as usize, 0);
            build_id = Some(id);
            now.put(sealed.start, note);
            differing(&prev, now)?
        }
        _ => dirty,
    };
    let mut ranges = Vec::new();
    let mut out = Vec::new();
    for (at, bytes) in dirty {
        ranges.push(DirtyRange { offset: at, len: bytes.len() as u64 });
        out.extend_from_slice(&bytes);
    }
    counts.ranges = ranges.len() as u64;
    counts.dirty = out.len() as u64;
    Ok(Refill { ranges, bytes: out, leaves: new_leaves, build_id, counts })
}

/// The overlay's runs whose bytes differ from the previous output's
/// (IN-75: an empty delta writes no byte).
fn differing(prev: &Previous<'_>, overlay: Overlay) -> Result<Vec<(u64, Vec<u8>)>, RefillError> {
    let mut out = Vec::new();
    for (at, bytes) in overlay.merged() {
        if bytes.is_empty() || prev.read(at..at + bytes.len() as u64)? == bytes {
            continue;
        }
        out.push((at, bytes));
    }
    Ok(out)
}

fn not_implemented(u: Unimplemented) -> RefillError {
    RefillError::Fallback(IncrReason::UpdateNotImplemented(Some(u)))
}

/// `input-changed` (IN-75): every input of the link, as the store's
/// recheck finds it (`fstat` against the file the front read).
fn recheck(link: &Link<'_, '_>, counts: &mut Counts) -> Result<(), RefillError> {
    for (file, row) in link.inputs.files.iter() {
        counts.rechecked += 1;
        if let Err(e) = (link.recheck)(file) {
            let path = String::from_utf8_lossy(row.path).into_owned();
            return Err(RefillError::Fallback(IncrReason::InputChanged(format!("{path}: {e:?}"))));
        }
    }
    Ok(())
}

/// An input section's size.
fn section_size(link: &Link<'_, '_>, s: mortar_core::SectionId) -> Option<u64> {
    let (file, index) = link.resolution.sh_index(s)?;
    let mortar_core::inputs::Reader::Object(o) = &link.inputs.files.get(file)?.reader else {
        return None;
    };
    Some(o.section(index).ok()?.size())
}

/// The file ranges of `layout.frags` whose count is not 0 (IN-72 item 6:
/// a fragment appended, or whose count rises from 0, is rewritten): each
/// from its row's `offset` and `size` (seam 6) in its output section's
/// merged chunk; O(changed fragments), no other fragment read.
fn frag_ranges(link: &Link<'_, '_>, layout: &LayoutDelta) -> Vec<Range<u64>> {
    let merged: BTreeMap<u32, (u64, u64)> = link
        .layout
        .placement
        .chunks
        .iter()
        .filter(|(_, c)| c.in_file())
        .filter_map(|(_, c)| match c.kind {
            ChunkKind::Merged(o) => Some((o.0, (c.offset, c.size))),
            _ => None,
        })
        .collect();
    layout
        .frags
        .iter()
        .filter(|f| f.refs > 0)
        .filter_map(|f| {
            let &(at, size) = merged.get(&f.out.0)?;
            let end = f.offset + u64::from(f.size);
            (end <= size).then(|| at + f.offset..at + end)
        })
        .collect()
}

/// IN-34a moves an unchanged file's FDEs only through the growth steps: a
/// code section of a file whose `.eh_frame` range is not rewritten that
/// moved for another reason (`LayoutDelta::moved`) has FDEs this writer
/// cannot translate (its old place is the stored row, not an input of
/// `refill`): `update-not-implemented (eh)`.
fn check_moved_code(link: &Link<'_, '_>, layout: &LayoutDelta) -> Result<(), RefillError> {
    if layout.moved.is_empty() || layout.eh.as_ref().is_some_and(|e| e.whole) {
        return Ok(());
    }
    let rewritten: Vec<FileId> =
        layout.eh.as_ref().map(|e| e.ranges.iter().map(|r| r.file).collect()).unwrap_or_default();
    for &s in &layout.moved {
        let Some((file, index)) = link.resolution.sh_index(s) else { continue };
        let Some(mortar_core::inputs::Reader::Object(o)) =
            link.inputs.files.get(file).map(|f| &f.reader)
        else {
            continue;
        };
        let exec =
            o.section(index).is_ok_and(|x| x.flags() & mortar_elf::consts::SHF_EXECINSTR != 0);
        if exec && !rewritten.contains(&file) {
            return Err(not_implemented(Unimplemented::Eh));
        }
    }
    Ok(())
}

// ---- the previous output ---------------------------------------------------

/// The previous output (U1's descriptor), read by range; past its end it
/// reads as zeros (the update's output may be longer).
struct Previous<'f> {
    file: &'f File,
    len: u64,
}

impl<'f> Previous<'f> {
    fn new(file: &'f File) -> Result<Self, RefillError> {
        let len = file.metadata().map_err(|e| output_changed(&e))?.len();
        Ok(Self { file, len })
    }

    fn read(&self, r: Range<u64>) -> Result<Vec<u8>, RefillError> {
        let mut buf = vec![0u8; (r.end - r.start) as usize];
        let end = r.end.min(self.len);
        if end > r.start {
            self.file
                .read_exact_at(&mut buf[..(end - r.start) as usize], r.start)
                .map_err(|e| output_changed(&e))?;
        }
        Ok(buf)
    }

    /// The section header table (ELF64 or ELF32, little-endian).
    fn sections(&self) -> Result<Table, RefillError> {
        let h = self.read(0..64)?;
        parse_table(&h, |r| self.read(r)).ok_or_else(|| {
            RefillError::Fallback(IncrReason::OutputChanged("not an ELF file".to_owned()))
        })?
    }

    /// Each section's name.
    fn names(&self, t: &Table) -> Result<Vec<Vec<u8>>, RefillError> {
        let Some(s) = t.rows.get(t.shstrndx as usize) else {
            return Err(RefillError::Fallback(IncrReason::OutputChanged(
                "no section name table".to_owned(),
            )));
        };
        let pool = self.read(s.offset..s.offset + s.size)?;
        Ok(t.rows.iter().map(|r| cstr(&pool, r.name as usize).to_vec()).collect())
    }
}

fn output_changed(e: &std::io::Error) -> RefillError {
    RefillError::Fallback(IncrReason::OutputChanged(format!("the previous output: {e}")))
}

fn cstr(pool: &[u8], at: usize) -> &[u8] {
    let s = pool.get(at..).unwrap_or(&[]);
    &s[..s.iter().position(|&b| b == 0).unwrap_or(s.len())]
}

/// One section header's fields.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Row {
    pub name: u32,
    pub in_file: bool,
    pub alloc: bool,
    pub addr: u64,
    pub offset: u64,
    pub size: u64,
}

/// A section header table.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Table {
    pub class64: bool,
    pub shstrndx: u32,
    pub rows: Vec<Row>,
}

fn le(b: &[u8], at: usize, n: usize) -> u64 {
    b.get(at..at + n).map_or(0, |s| s.iter().rev().fold(0, |a, &x| a << 8 | u64::from(x)))
}

/// The section header table of an ELF file whose first 64 bytes are
/// `h`, reading the table through `read`. `None` for a file that is not
/// a little-endian ELF.
#[allow(clippy::type_complexity)]
fn parse_table<E>(
    h: &[u8],
    read: impl Fn(Range<u64>) -> Result<Vec<u8>, E>,
) -> Option<Result<Table, E>> {
    if h.get(..4) != Some(b"\x7fELF".as_slice()) || h.get(5) != Some(&1) {
        return None;
    }
    let class64 = h.get(4) == Some(&2);
    let (shoff, shentsize, shnum, shstrndx) = if class64 {
        (le(h, 0x28, 8), le(h, 0x3a, 2), le(h, 0x3c, 2), le(h, 0x3e, 2))
    } else {
        (le(h, 0x20, 4), le(h, 0x2e, 2), le(h, 0x30, 2), le(h, 0x32, 2))
    };
    let bytes = match read(shoff..shoff + shentsize * shnum) {
        Ok(b) => b,
        Err(e) => return Some(Err(e)),
    };
    Some(Ok(table_of(&bytes, class64, shentsize as usize, shstrndx as u32)))
}

/// The rows of a section header table's bytes.
#[must_use]
pub fn table_of(bytes: &[u8], class64: bool, entsize: usize, shstrndx: u32) -> Table {
    let rows = bytes
        .chunks_exact(entsize.max(1))
        .map(|e| {
            let (ty, flags, addr, offset, size) = if class64 {
                (le(e, 4, 4), le(e, 8, 8), le(e, 16, 8), le(e, 24, 8), le(e, 32, 8))
            } else {
                (le(e, 4, 4), le(e, 8, 4), le(e, 12, 4), le(e, 16, 4), le(e, 20, 4))
            };
            Row {
                name: le(e, 0, 4) as u32,
                in_file: ty != 8 && ty != 0,
                alloc: flags & 2 != 0,
                addr,
                offset,
                size,
            }
        })
        .collect();
    Table { class64, shstrndx, rows }
}

/// The update's section rows, from its placement (as `out_sections`): per
/// section index, its first chunk's address and offset and its extent.
fn new_sections(pl: &Placement) -> Table {
    let mut rows: Vec<Row> = Vec::new();
    let mut class64 = true;
    for (_, c) in pl.chunks.iter() {
        if c.kind == ChunkKind::FileHeader {
            class64 = c.size == 64;
        }
        let Some(n) = c.shndx else { continue };
        let n = n as usize;
        if rows.len() <= n {
            rows.resize(n + 1, Row::default());
        }
        let r = &mut rows[n];
        let start = c.addr.unwrap_or(c.offset);
        let first = if r.size == 0 && r.offset == 0 { None } else { Some(r.addr) };
        let pos = |r: &Row| if c.addr.is_some() { r.addr } else { r.offset };
        if first.is_none() {
            *r = Row {
                name: 0,
                in_file: c.in_file(),
                alloc: c.is_alloc(),
                addr: c.addr.unwrap_or(0),
                offset: c.offset,
                size: c.size,
            };
        } else {
            let end = (pos(r) + r.size).max(start + c.size);
            if start < pos(r) {
                r.addr = c.addr.unwrap_or(0);
                r.offset = c.offset;
            }
            r.size = end - pos(r);
        }
    }
    Table { class64, shstrndx: 0, rows }
}

/// Each section index's name in the update's placement.
fn new_names(link: &Link<'_, '_>) -> Vec<Option<Vec<u8>>> {
    let mut out: Vec<Option<Vec<u8>>> = Vec::new();
    for (_, c) in link.layout.placement.chunks.iter() {
        let (Some(i), Some(name)) = (c.shndx, c.name(link.assignment)) else { continue };
        let i = i as usize;
        if out.len() <= i {
            out.resize(i + 1, None);
        }
        out[i].get_or_insert_with(|| name.to_vec());
    }
    out
}

/// The output sections that moved between the previous output and the
/// update's (a growth step, IN-72 item 3), and the address map they make.
#[derive(Debug, Clone, Default)]
pub struct Shift {
    /// Section indices whose address or file offset changed.
    pub moved: Vec<usize>,
    /// Per allocated section of the previous output: its address range
    /// and how far it moved, sorted by address.
    pub addrs: Vec<(Range<u64>, i64)>,
    /// Per section: its file offset in the update's output, and what the
    /// previous output's offset becomes.
    sizes: Vec<(u64, u64)>,
}

impl Shift {
    /// The moves of `old` to `new` (rows by section index).
    #[must_use]
    pub fn new(old: &Table, new: &Table) -> Self {
        let mut s = Self::default();
        for (i, (o, n)) in old.rows.iter().zip(&new.rows).enumerate().skip(1) {
            if o.addr != n.addr || (o.in_file && n.in_file && o.offset != n.offset) {
                s.moved.push(i);
            }
            if o.alloc && o.size > 0 {
                let d = n.addr.wrapping_sub(o.addr) as i64;
                s.addrs.push((o.addr..o.addr + o.size, d));
            }
            s.sizes.push((o.size, n.size));
        }
        s.addrs.sort_by_key(|a| a.0.start);
        s
    }

    /// Where address `a` of the previous output is now.
    #[must_use]
    pub fn map(&self, a: u64) -> u64 {
        let at = self.addrs.partition_point(|r| r.0.start <= a);
        match at.checked_sub(1).and_then(|i| self.addrs.get(i)) {
            Some((r, d)) if a < r.end || (a == r.end && r.end > r.start) => {
                a.wrapping_add(*d as u64)
            }
            _ => a,
        }
    }

    /// The padding before and after each moved section in the update's
    /// output (up to its neighbours in file order: a section, or one of
    /// `fixed`, the header tables that bound the sections), for the image
    /// to fill. Without `fixed` the padding after the LAST section kept
    /// the previous output's bytes (batch 14: three bytes of the old
    /// `.shstrtab` before the section header table).
    fn gaps(&self, new: &Table, fixed: &[Range<u64>]) -> Vec<Range<u64>> {
        let mut spans: Vec<(u64, u64, usize)> = new
            .rows
            .iter()
            .enumerate()
            .skip(1)
            .filter(|(_, r)| r.in_file && r.size > 0)
            .map(|(i, r)| (r.offset, r.offset + r.size, i))
            .chain(fixed.iter().map(|r| (r.start, r.end, usize::MAX)))
            .collect();
        spans.sort_unstable();
        let mut out = Vec::new();
        for (k, s) in spans.iter().enumerate() {
            if !self.moved.contains(&s.2) {
                continue;
            }
            if let Some(p) = k.checked_sub(1).map(|p| spans[p])
                && p.1 < s.0
            {
                out.push(p.1..s.0);
            }
            if let Some(n) = spans.get(k + 1)
                && s.1 < n.0
            {
                out.push(s.1..n.0);
            }
        }
        out
    }

    /// The bytes past each section's previous size, where it grew.
    fn grown_tails(old: &Table, new: &Table) -> Vec<Range<u64>> {
        old.rows
            .iter()
            .zip(&new.rows)
            .skip(1)
            .filter(|(o, n)| n.in_file && n.size > o.size)
            .map(|(o, n)| n.offset + o.size..n.offset + n.size)
            .collect()
    }
}

// ---- `.eh_frame` (IN-34a) --------------------------------------------------

/// One record of the previous `.eh_frame`: its offset, its length (with
/// the length word), and for an FDE its CIE's offset; for a CIE whether
/// its FDEs have an LSDA and where its personality field is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct EhRecord {
    pub offset: u64,
    pub len: u64,
    /// `None` for a CIE.
    pub cie: Option<u64>,
    /// A CIE's: whether its FDEs carry an LSDA (`L`).
    pub lsda: bool,
    /// A CIE's: whether it has `z` augmentation data.
    pub z: bool,
    /// A CIE's personality field's offset in the section (`P`, 0x9b).
    pub personality: Option<u64>,
}

fn uleb(b: &[u8], at: &mut usize) -> Option<u64> {
    let mut v = 0u64;
    let mut shift = 0;
    loop {
        let x = *b.get(*at)?;
        *at += 1;
        v |= u64::from(x & 0x7f).checked_shl(shift)?;
        if x & 0x80 == 0 {
            return Some(v);
        }
        shift += 7;
    }
}

/// The records of `.eh_frame`'s bytes, each CIE's encodings checked: only
/// `DW_EH_PE_pcrel | sdata4` (0x1b) for `R` and `L` and its indirect form
/// (0x9b) for `P` are moved (WP-239 R-3); any other, a 64-bit record, or a
/// malformed one is `update-not-implemented (eh)`.
///
/// # Errors
/// `update-not-implemented (eh)`.
pub fn walk_eh(bytes: &[u8], counts: &mut Counts) -> Result<Vec<EhRecord>, RefillError> {
    let eh = || not_implemented(Unimplemented::Eh);
    let mut out: Vec<EhRecord> = Vec::new();
    let mut at = 0usize;
    while at + 4 <= bytes.len() {
        let len = le(bytes, at, 4);
        if len == 0 {
            break;
        }
        if len == 0xffff_ffff {
            return Err(eh());
        }
        let end = at + 4 + len as usize;
        if end > bytes.len() || len < 4 {
            return Err(eh());
        }
        counts.eh_records += 1;
        let id = le(bytes, at + 4, 4);
        if id == 0 {
            let mut p = at + 9;
            let aug_end = p + bytes[p..end].iter().position(|&b| b == 0).ok_or_else(eh)?;
            let aug = &bytes[p..aug_end];
            p = aug_end + 1;
            let version = bytes[at + 8];
            uleb(bytes, &mut p).ok_or_else(eh)?;
            uleb(bytes, &mut p).ok_or_else(eh)?; // SLEB: the same bytes
            if version == 1 {
                p += 1;
            } else {
                uleb(bytes, &mut p).ok_or_else(eh)?;
            }
            let mut rec = EhRecord {
                offset: at as u64,
                len: len + 4,
                cie: None,
                lsda: false,
                z: false,
                personality: None,
            };
            if let Some(rest) = aug.strip_prefix(b"z") {
                rec.z = true;
                uleb(bytes, &mut p).ok_or_else(eh)?;
                for &c in rest {
                    match c {
                        b'R' | b'L' => {
                            if *bytes.get(p).ok_or_else(eh)? != 0x1b {
                                return Err(eh());
                            }
                            rec.lsda |= c == b'L';
                            p += 1;
                        }
                        b'P' => {
                            if *bytes.get(p).ok_or_else(eh)? != 0x9b {
                                return Err(eh());
                            }
                            rec.personality = Some(p as u64 + 1);
                            p += 5;
                        }
                        b'S' => {}
                        _ => return Err(eh()),
                    }
                }
            } else if !aug.is_empty() {
                return Err(eh());
            }
            out.push(rec);
        } else {
            let cie = (at as u64 + 4).checked_sub(id).ok_or_else(eh)?;
            let c = out.iter().find(|r| r.offset == cie && r.cie.is_none()).ok_or_else(eh)?;
            // A CIE without `R` (its FDEs absolute): not moved here.
            if !c.z {
                return Err(eh());
            }
            out.push(EhRecord {
                offset: at as u64,
                len: len + 4,
                cie: Some(cie),
                lsda: c.lsda,
                z: true,
                personality: None,
            });
        }
        at = end;
    }
    Ok(out)
}

/// The FDE and CIE fields whose value changes when `.eh_frame` moves from
/// `old_addr` to `new_addr` and the code by `shift`: `(offset in the
/// section, the new 4 bytes)`. A value that does not fit is
/// `update-not-implemented (eh)`.
///
/// # Errors
/// `update-not-implemented (eh)`.
pub fn patch_eh(
    bytes: &[u8],
    records: &[EhRecord],
    old_addr: u64,
    new_addr: u64,
    shift: &Shift,
    counts: &mut Counts,
) -> Result<Vec<(u64, [u8; 4])>, RefillError> {
    let mut out = Vec::new();
    let mut field = |off: u64| -> Result<(), RefillError> {
        let v = le(bytes, off as usize, 4) as u32 as i32;
        let target = (old_addr + off).wrapping_add(i64::from(v) as u64);
        let now = shift.map(target);
        let d = now.wrapping_sub(new_addr + off) as i64;
        let d = i32::try_from(d).map_err(|_| not_implemented(Unimplemented::Eh))?;
        if d != v {
            counts.eh_patched += 1;
            out.push((off, d.to_le_bytes()));
        }
        Ok(())
    };
    for r in records {
        if r.cie.is_none() {
            if let Some(p) = r.personality {
                field(p)?;
            }
        } else {
            field(r.offset + 8)?;
            if r.lsda {
                let mut p = r.offset as usize + 16;
                let n = uleb(bytes, &mut p).ok_or_else(|| not_implemented(Unimplemented::Eh))?;
                if n >= 4 {
                    field(p as u64)?;
                }
            }
        }
    }
    Ok(out)
}

/// An unchanged file's `.eh_frame` range that MOVES in a whole rewrite
/// (IN-34a; WP-243's shape: a range of `LayoutDelta::eh` with an empty
/// `records` list): its bytes `range` as the previous output holds them
/// at `old_addr`, walked, checked against the stored row's FDE count
/// `fdes`, and their PC-relative fields re-targeted for `new_addr` and the
/// code's moves `shift`. The CIE pointers are distances inside the range
/// (one CIE a file, IN-34a) and do not change; an FDE whose CIE is
/// outside the range is `update-not-implemented (eh)`.
///
/// # Errors
/// `update-not-implemented (eh)`; `state-corrupt` for an FDE count that
/// is not the stored one.
pub fn move_eh_range(
    range: &[u8],
    old_addr: u64,
    new_addr: u64,
    fdes: u32,
    shift: &Shift,
    counts: &mut Counts,
) -> Result<Vec<u8>, RefillError> {
    let records = walk_eh(range, counts)?;
    let n = records.iter().filter(|r| r.cie.is_some()).count();
    if n != fdes as usize {
        return Err(RefillError::Fallback(IncrReason::StateCorrupt(format!(
            "FILES.eh_fdes: {fdes} stored, {n} in the output's range"
        ))));
    }
    let mut out = range.to_vec();
    for (at, b) in patch_eh(range, &records, old_addr, new_addr, shift, counts)? {
        out[at as usize..at as usize + 4].copy_from_slice(&b);
    }
    Ok(out)
}

// ---- `.symtab` and `.strtab` (IN-49a) -------------------------------------

/// U-5's plan written: entries, fillers, `.strtab` bytes, entries moved
/// from the previous output, and the remapped values of unchanged blocks
/// (their `st_value` through the output sections' moves).
#[allow(clippy::too_many_arguments)]
fn write_symtab(
    prev: &Previous<'_>,
    old: &Table,
    new: &Table,
    names: &[Vec<u8>],
    plan: &SymtabPlan,
    shift: &Shift,
    overlay: &mut Overlay,
    counts: &mut Counts,
) -> Result<(), RefillError> {
    let find = |n: &[u8]| names.iter().position(|x| x == n);
    let (Some(st), Some(ss)) = (find(b".symtab"), find(b".strtab")) else { return Ok(()) };
    let es: u64 = if new.class64 { 24 } else { 16 };
    let (o, n) = (old.rows[st], new.rows[st]);
    let sec1 = new.rows.get(1).map_or(0, |r| r.addr);
    let entry = |name: u32, info: u8, other: u8, shndx: u16, value: u64, size: u64| {
        let mut e = Vec::with_capacity(es as usize);
        e.extend_from_slice(&name.to_le_bytes());
        if new.class64 {
            e.extend_from_slice(&[info, other]);
            e.extend_from_slice(&shndx.to_le_bytes());
            e.extend_from_slice(&value.to_le_bytes());
            e.extend_from_slice(&size.to_le_bytes());
        } else {
            e.extend_from_slice(&(value as u32).to_le_bytes());
            e.extend_from_slice(&(size as u32).to_le_bytes());
            e.extend_from_slice(&[info, other]);
            e.extend_from_slice(&shndx.to_le_bytes());
        }
        e
    };
    // The moves by their new index. U-5 drops a move whose `to` `entries`
    // writes (a `Changed` global a removal moves): two writers of one index
    // are a plan that contradicts itself.
    let written: std::collections::BTreeSet<u32> = plan.entries.iter().map(|e| e.index).collect();
    let mut from_of: BTreeMap<u32, u32> = BTreeMap::new();
    for &(from, to) in &plan.moves {
        if from_of.insert(to, from).is_some() || written.contains(&to) {
            return Err(RefillError::Fallback(IncrReason::StateCorrupt(format!(
                "SYMTAB: the plan writes .symtab index {to} twice (a move and another writer)"
            ))));
        }
    }
    let old_entry =
        |i: u32| prev.read(o.offset + u64::from(i) * es..o.offset + u64::from(i + 1) * es);
    // Every move is read from the previous output, as read, before any
    // write (an appended entry may take a move's `from`): `prev` is the
    // previous output's descriptor, never the overlay.
    for (&to, &from) in &from_of {
        overlay.put(n.offset + u64::from(to) * es, old_entry(from)?);
        counts.symtab_entries += 1;
    }
    // A remap range is the table as it stands after the moves (IN-49a,
    // U-5's plan: "U-6 copies the moves, then remaps"): at index i the
    // source is the previous output's entry at a move's `from` when a move
    // has `to == i`, else its entry at i; `st_value` mapped, the rest as
    // read. A moved entry is remapped.
    for &(lo, hi) in &plan.remap {
        let at = n.offset + u64::from(lo) * es;
        let old_at = o.offset + u64::from(lo) * es;
        let mut b = prev.read(old_at..old_at + u64::from(hi - lo) * es)?;
        for (&to, &from) in from_of.range(lo..hi) {
            let k = ((to - lo) as usize) * es as usize;
            b[k..k + es as usize].copy_from_slice(&old_entry(from)?);
        }
        for e in b.chunks_exact_mut(es as usize) {
            counts.symtab_entries += 1;
            let (info, shndx, vat, w) =
                if new.class64 { (e[4], le(e, 6, 2), 8, 8) } else { (e[12], le(e, 14, 2), 4, 4) };
            if shndx == 0 || shndx >= 0xff00 || info & 0xf == 6 {
                continue;
            }
            let v = le(e, vat, w);
            let m = shift.map(v);
            e[vat..vat + w].copy_from_slice(&m.to_le_bytes()[..w]);
        }
        overlay.put(at, b);
    }
    for &(lo, hi) in &plan.fillers {
        let one = entry(0, 0x03, 0, 1, sec1, 0);
        let mut b = Vec::with_capacity(((hi - lo) as usize) * one.len());
        for _ in lo..hi {
            b.extend_from_slice(&one);
        }
        counts.symtab_entries += u64::from(hi - lo);
        overlay.put(n.offset + u64::from(lo) * es, b);
    }
    for e in &plan.entries {
        let shndx = match e.shndx {
            s if s < 0xff00 || s == 0xfff1 || s == 0xfff2 => s as u16,
            _ => return Err(not_implemented(Unimplemented::Image)),
        };
        counts.symtab_entries += 1;
        overlay.put(
            n.offset + u64::from(e.index) * es,
            entry(e.name, e.info, e.other, shndx, e.value, e.size),
        );
    }
    // A shrinking table (a global area that lost entries; U-5's Q2-5: the
    // plan has no field for it): the entries [new end, old end) are zeros,
    // as the file slack after a full link's table is. The old end is the
    // previous output's `.symtab` `sh_size` (its section header, read
    // above); the range must not reach another section of the new output.
    if o.size > n.size {
        let zero = n.offset + n.size..n.offset + o.size;
        let hit = new.rows.iter().enumerate().skip(1).any(|(i, r)| {
            i != st
                && r.in_file
                && r.size > 0
                && r.offset < zero.end
                && zero.start < r.offset + r.size
        });
        if hit {
            return Err(RefillError::Fallback(IncrReason::StateCorrupt(format!(
                "SYMTAB: the previous .symtab's end {:#x} reaches another section of the update",
                zero.end
            ))));
        }
        counts.symtab_entries += (o.size - n.size) / es;
        overlay.put(zero.start, vec![0; (zero.end - zero.start) as usize]);
    }
    let s = new.rows[ss];
    for (off, bytes) in &plan.strtab {
        overlay.put(s.offset + off, bytes.clone());
    }
    Ok(())
}

// ---- the dirty ranges ------------------------------------------------------

/// Sorted, disjoint ranges: overlapping and touching ones merged.
fn normalize(mut v: Vec<Range<u64>>) -> Vec<Range<u64>> {
    v.retain(|r| r.end > r.start);
    v.sort_by_key(|r| (r.start, r.end));
    let mut out: Vec<Range<u64>> = Vec::with_capacity(v.len());
    for r in v {
        match out.last_mut() {
            Some(l) if r.start <= l.end => l.end = l.end.max(r.end),
            _ => out.push(r),
        }
    }
    out
}

/// The image windows (`IMAGE_BLOCK`) the ranges touch, sorted.
fn windows_of(ranges: &[Range<u64>]) -> Vec<u64> {
    let w = IMAGE_BLOCK;
    let mut out: Vec<u64> = Vec::new();
    for r in ranges {
        let (lo, hi) = (r.start / w, (r.end - 1) / w);
        for k in lo..=hi {
            if out.last() != Some(&k) {
                out.push(k);
            }
        }
    }
    out.dedup();
    out
}

/// The bytes of `r` from the filled jobs (each `(range, bytes)`, sorted
/// and disjoint).
fn slice_of(jobs: &[(Range<u64>, Vec<u8>)], r: &Range<u64>) -> Option<Vec<u8>> {
    let mut out = Vec::with_capacity((r.end - r.start) as usize);
    let mut at = r.start;
    while at < r.end {
        let (j, b) = jobs.get(jobs.partition_point(|(j, _)| j.end <= at))?;
        if j.start > at {
            return None;
        }
        let end = r.end.min(j.end);
        out.extend_from_slice(&b[(at - j.start) as usize..(end - j.start) as usize]);
        at = end;
    }
    Some(out)
}

/// Ruling F8 (WP-255a; IN-75): block `k` of the previous output, whose
/// bytes `win` starts with (its old range `old`), hashes to the stored
/// leaf `stored`, with the note's descriptor `sealed` zeroed as the leaf
/// rule says (I-72, I-76: the function that made the stored leaf). `win`
/// is given back as it was read. A difference is `output-changed`.
fn check_leaf(
    win: &mut [u8],
    old: &Range<u64>,
    sealed: Option<&Range<u64>>,
    f: mortar_digest::TreeFn,
    stored: &[u8; 32],
    k: u64,
) -> Result<(), RefillError> {
    let n = (old.end - old.start) as usize;
    let Some(bytes) = win.get_mut(..n) else {
        return Err(not_implemented(Unimplemented::Image));
    };
    // The note's bytes in this block, kept to be put back.
    let note =
        sealed.map(|s| s.start.max(old.start)..s.end.min(old.end)).filter(|z| z.start < z.end);
    let kept = note.as_ref().map(|z| {
        let at = (z.start - old.start) as usize..(z.end - old.start) as usize;
        let kept = bytes[at.clone()].to_vec();
        bytes[at].fill(0);
        kept
    });
    let leaf = mortar_digest::leaf(f, bytes);
    if let (Some(z), Some(kept)) = (&note, kept) {
        bytes[(z.start - old.start) as usize..(z.end - old.start) as usize].copy_from_slice(&kept);
    }
    let mut d = [0u8; 32];
    d[..leaf.as_bytes().len()].copy_from_slice(leaf.as_bytes());
    if &d != stored {
        return Err(RefillError::Fallback(IncrReason::OutputChanged(format!(
            "block {k} of the previous output (file bytes [{:#x}, {:#x})) is not the block its \
             stored leaf hashes",
            old.start, old.end
        ))));
    }
    Ok(())
}

fn zero(win: &mut [u8], base: u64, r: &Range<u64>) {
    let lo = r.start.max(base);
    let hi = r.end.min(base + win.len() as u64);
    if lo < hi {
        win[(lo - base) as usize..(hi - base) as usize].fill(0);
    }
}

/// Bytes put at file offsets, a later put over an earlier one.
#[derive(Debug, Default)]
pub struct Overlay {
    runs: BTreeMap<u64, Vec<u8>>,
}

impl Overlay {
    /// `bytes` at `at`, over whatever was there.
    pub fn put(&mut self, at: u64, bytes: Vec<u8>) {
        if bytes.is_empty() {
            return;
        }
        let end = at + bytes.len() as u64;
        let mut hit: Vec<u64> =
            self.runs.range(..at).next_back().map(|(&s, _)| s).into_iter().collect();
        hit.extend(self.runs.range(at..end).map(|(&s, _)| s));
        for s in hit {
            let Some(b) = self.runs.remove(&s) else { continue };
            let e = s + b.len() as u64;
            if e <= at || s >= end {
                self.runs.insert(s, b);
                continue;
            }
            if s < at {
                self.runs.insert(s, b[..(at - s) as usize].to_vec());
            }
            if e > end {
                self.runs.insert(end, b[(end - s) as usize..].to_vec());
            }
        }
        self.runs.insert(at, bytes);
    }

    /// Overwrites `win` (bytes at `base`) with the overlay's bytes there.
    pub fn read_over(&self, base: u64, win: &mut [u8]) {
        let end = base + win.len() as u64;
        let first = self.runs.range(..=base).next_back().map_or(base, |(&s, _)| s);
        for (&s, b) in self.runs.range(first..end) {
            let e = s + b.len() as u64;
            let (lo, hi) = (s.max(base), e.min(end));
            if lo < hi {
                win[(lo - base) as usize..(hi - base) as usize]
                    .copy_from_slice(&b[(lo - s) as usize..(hi - s) as usize]);
            }
        }
    }

    /// The runs, touching ones joined, in order.
    pub fn merged(self) -> Vec<(u64, Vec<u8>)> {
        let mut out: Vec<(u64, Vec<u8>)> = Vec::new();
        for (s, b) in self.runs {
            match out.last_mut() {
                Some((ls, lb)) if *ls + lb.len() as u64 == s => lb.extend_from_slice(&b),
                _ => out.push((s, b)),
            }
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // spec: incr.md IN-72 item 3 (a growth step: every later section of
    // its PT_LOAD moves in address, and in file offset unless NOBITS; the
    // later non-allocated sections move in the file), IN-75 (the grown
    // tail and every moved section dirty): hand-made section tables, the
    // linked half in `crates/mortar/tests/update_image.rs`.
    #[test]
    fn growth_step_rewrites_headers_hand_made() {
        let row = |in_file, alloc, addr, offset, size| Row {
            name: 0,
            in_file,
            alloc,
            addr,
            offset,
            size,
        };
        let old = Table {
            class64: true,
            shstrndx: 0,
            rows: vec![
                Row::default(),
                row(true, true, 0x1000, 0x1000, 0x100),
                row(true, true, 0x2100, 0x1100, 0x10),
                row(false, true, 0x2110, 0x1110, 0x20),
                row(true, false, 0, 0x1110, 0x48),
            ],
        };
        let mut new = old.clone();
        new.rows[1].size += 0x80;
        new.rows[2].addr += 0x80;
        new.rows[2].offset += 0x80;
        new.rows[3].addr += 0x80;
        new.rows[3].offset += 0x80;
        new.rows[4].offset += 0x80;
        let s = Shift::new(&old, &new);
        assert_eq!(s.moved, [2, 3, 4]);
        assert_eq!(s.map(0x1050), 0x1050, "the grown section's own code stays");
        assert_eq!(s.map(0x2104), 0x2184);
        assert_eq!(s.map(0x2120), 0x21a0, "NOBITS moves in address");
        assert_eq!(s.map(0x3000), 0x3000, "past every section");
        let tails: Vec<(u64, u64)> =
            Shift::grown_tails(&old, &new).into_iter().map(|r| (r.start, r.end)).collect();
        assert_eq!(tails, [(0x1100, 0x1180)]);
        // No byte of padding between the moved sections: no gap to fill.
        assert!(s.gaps(&new, &[]).is_empty(), "{:?}", s.gaps(&new, &[]));
        // The padding between the last moved section and the section
        // header table is one (the table is no section: a bound only).
        let table = |start: u64| [Range { start, end: start + 0x140 }];
        assert!(s.gaps(&new, &table(0x11d8)).is_empty());
        assert_eq!(s.gaps(&new, &table(0x11e0)), table(0x11d8).map(|r| r.start..0x11e0));
    }

    /// A hand-made previous output for `write_symtab`: `.text` at 0x1000,
    /// `.data` at 0x2000, a 5-entry `.symtab` at 0x100 (null, local L,
    /// global A in `.text`, global R and global M in `.data`), and the
    /// update's table: `.text` grown by 0x80 (`.data` moves by 0x80 in
    /// address and file), `.symtab` of `new_entries` entries at 0x180.
    struct SymtabCase {
        old: Table,
        new: Table,
        names: Vec<Vec<u8>>,
        prev: File,
    }

    const ES: u64 = 24;
    const L: u32 = 1;
    const A: u32 = 3;
    const R: u32 = 5;
    const M: u32 = 7;
    const N: u32 = 9;

    fn sym(name: u32, info: u8, shndx: u16, value: u64, size: u64) -> Vec<u8> {
        let mut e = Vec::new();
        e.extend_from_slice(&name.to_le_bytes());
        e.extend_from_slice(&[info, 0]);
        e.extend_from_slice(&shndx.to_le_bytes());
        e.extend_from_slice(&value.to_le_bytes());
        e.extend_from_slice(&size.to_le_bytes());
        e
    }

    fn symtab_case(new_entries: u64) -> SymtabCase {
        let row =
            |alloc, addr, offset, size| Row { name: 0, in_file: true, alloc, addr, offset, size };
        let old = Table {
            class64: true,
            shstrndx: 0,
            rows: vec![
                Row::default(),
                row(true, 0x1000, 0x40, 0x40),
                row(true, 0x2000, 0x80, 0x20),
                row(false, 0, 0x100, 5 * ES),
                row(false, 0, 0x200, 0x20),
            ],
        };
        let new = Table {
            rows: vec![
                Row::default(),
                row(true, 0x1000, 0x40, 0xc0),
                row(true, 0x2080, 0x100, 0x20),
                row(false, 0, 0x180, new_entries * ES),
                row(false, 0, 0x280, 0x20),
            ],
            ..old.clone()
        };
        let mut bytes = vec![0u8; 0x300];
        let table = [
            vec![0; ES as usize],
            sym(L, 0x02, 1, 0x1004, 4),
            sym(A, 0x12, 1, 0x1008, 4),
            sym(R, 0x11, 2, 0x2000, 8),
            sym(M, 0x11, 2, 0x2008, 8),
        ]
        .concat();
        bytes[0x100..0x100 + table.len()].copy_from_slice(&table);
        // The previous output: a file of this process, unlinked once open
        // (the descriptor is what refill reads).
        let path = std::env::temp_dir().join(format!(
            "mortar-wp218-symtab-{}-{new_entries}-{:?}",
            std::process::id(),
            std::thread::current().id()
        ));
        std::fs::write(&path, &bytes).unwrap();
        let prev = File::open(&path).unwrap();
        std::fs::remove_file(&path).unwrap();
        let names =
            [&b""[..], b".text", b".data", b".symtab", b".strtab"].map(<[u8]>::to_vec).to_vec();
        SymtabCase { old, new, names, prev }
    }

    /// The new table's bytes [0, `n` entries) as `write_symtab` leaves
    /// them (0xaa where nothing was written).
    fn symtab_written(c: &SymtabCase, plan: &SymtabPlan, n: u64) -> Result<Vec<u8>, RefillError> {
        let prev = Previous::new(&c.prev)?;
        let shift = Shift::new(&c.old, &c.new);
        let mut overlay = Overlay::default();
        let mut counts = Counts::default();
        write_symtab(&prev, &c.old, &c.new, &c.names, plan, &shift, &mut overlay, &mut counts)?;
        let mut out = vec![0xaa; (n * ES) as usize];
        overlay.read_over(c.new.rows[3].offset, &mut out);
        Ok(out)
    }

    fn entry_at(t: &[u8], i: usize) -> &[u8] {
        &t[i * ES as usize..(i + 1) * ES as usize]
    }

    // spec: incr.md IN-49a (a removed global's index taken by the area's
    // last entry, a `move`; every growth step makes the rest of the area a
    // `remap` range, the move's `to` included: U-6 copies the moves, then
    // remaps; WP-218 resume ruling 1), U-5's Q2-5 (a shrinking global area:
    // the entries [new end, old end) are zeros, the file slack of a full
    // link). R removed, M (in `.data`, which the growth moves by 0x80)
    // moved from 4 to 3.
    #[test]
    fn symtab_move_inside_remap_is_remapped_and_shrink_zeroed() {
        let c = symtab_case(4);
        let plan = SymtabPlan {
            moves: vec![(4, 3)],
            remap: vec![(1, 4)],
            tables: mortar_core::incr::SymtabTables {
                symtab_size: 4 * ES,
                symtab_slack: ES,
                ..Default::default()
            },
            ..SymtabPlan::default()
        };
        let t = symtab_written(&c, &plan, 5).unwrap();
        assert_eq!(entry_at(&t, 1), sym(L, 0x02, 1, 0x1004, 4), "L: .text did not move");
        assert_eq!(entry_at(&t, 2), sym(A, 0x12, 1, 0x1008, 4));
        assert_eq!(entry_at(&t, 3), sym(M, 0x11, 2, 0x2088, 8), "M moved, value mapped");
        assert_eq!(entry_at(&t, 4), [0; ES as usize], "the old end zeroed");
        for i in 1..5 {
            let name = u32::from_le_bytes(entry_at(&t, i)[..4].try_into().unwrap());
            assert_ne!(name, R, "the removed symbol's name at index {i}");
        }
    }

    // spec: incr.md IN-49a, U-5's plan (every move READ from the previous
    // output before any appended entry is written: an appended entry may
    // take a move's `from`; WP-218 resume ruling 3), and two writers of one
    // index refused (ruling 1: U-5 drops a move whose `to` `entries`
    // writes).
    #[test]
    fn symtab_appended_entry_over_a_move_source() {
        let c = symtab_case(5);
        let n = mortar_core::incr::SymtabEntry {
            index: 4,
            name: N,
            info: 0x12,
            other: 0,
            shndx: 1,
            value: 0x1010,
            size: 4,
        };
        let plan = SymtabPlan {
            moves: vec![(4, 3)],
            remap: vec![(1, 4)],
            entries: vec![n],
            ..SymtabPlan::default()
        };
        let t = symtab_written(&c, &plan, 5).unwrap();
        assert_eq!(entry_at(&t, 3), sym(M, 0x11, 2, 0x2088, 8), "M read before N was written");
        assert_eq!(entry_at(&t, 4), sym(N, 0x12, 1, 0x1010, 4), "the appended entry");
        let both =
            SymtabPlan { entries: vec![mortar_core::incr::SymtabEntry { index: 3, ..n }], ..plan };
        let r = symtab_written(&c, &both, 5);
        assert!(
            matches!(r, Err(RefillError::Fallback(IncrReason::StateCorrupt(_)))),
            "a move and an entry at one index: {r:?}"
        );
    }

    // spec: incr.md IN-75 (a later write over an earlier one; the ranges
    // sorted and disjoint).
    #[test]
    fn overlay_puts_over_and_merges() {
        let mut o = Overlay::default();
        o.put(10, vec![1; 10]);
        o.put(15, vec![2; 10]);
        o.put(5, vec![3; 3]);
        o.put(12, vec![4; 2]);
        o.put(25, vec![5; 1]);
        assert_eq!(
            o.merged(),
            vec![(5, vec![3; 3]), (10, vec![1, 1, 4, 4, 1, 2, 2, 2, 2, 2, 2, 2, 2, 2, 2, 5]),]
        );
        let mut o = Overlay::default();
        o.put(0, vec![9; 4]);
        o.put(0, vec![8; 2]);
        o.put(3, vec![7; 4]);
        let mut w = vec![0; 8];
        o.read_over(0, &mut w);
        assert_eq!(w, [8, 8, 9, 7, 7, 7, 7, 0]);
    }

    #[test]
    fn normalize_and_windows() {
        assert_eq!(normalize(vec![5..9, 0..2, 2..3, 8..12, 20..20]), vec![0..3, 5..12]);
        let w = IMAGE_BLOCK;
        assert_eq!(windows_of(&[0..1, w - 1..w + 1, 3 * w..3 * w + 2]), vec![0, 1, 3]);
    }
}
