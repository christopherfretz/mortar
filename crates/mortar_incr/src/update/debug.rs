//! Debug sections in an update (`spec/incr.md` IN-50..IN-57, IN-70's
//! debug condition; WP-228, G-1). WP-213 (U-1) created the file; G-1
//! fills it and adds no `mod` line and no shared type: [`DebugStrings`],
//! [`DebugPlan`] and the helpers are this file's.
//!
//! **For the join (G-3).** In this order: (1) [`strings`] over the
//! changed files' `.debug_str`/`.debug_line_str` input bytes and the
//! PREVIOUS output (it reads, per hit, the bytes at the stored offset in
//! the previous output's merged section, nothing else of it); (2)
//! `mortar_layout::delta::layout` with [`LayoutInput::debug`] `true`
//! (debug sections are left unplaced for this file); (3) [`place`] on its
//! result before anything reads it. The image then writes the changed
//! files' debug sections at their new places (relocated as a full link
//! relocates them, strings named through [`DebugStrings::output_offset`],
//! a COMDAT import through [`import_target`]), the `Debug`/`DeadDebug`
//! holes of [`DebugPlan::chunks`] in IN-51b's neutral forms
//! (`mortar_image::neutral`), the appended strings at the merged
//! sections' tails, and `.debug_aranges` rewritten by [`aranges`]. Until
//! G-3 joins it, nothing calls this file from the shipped update path
//! and an update with a changed file carrying debug sections is
//! `update-not-implemented (debug)` (the join's check on
//! `ChangedFile::debug`, and `LayoutInput::debug` `false`).
//!
//! [`LayoutInput::debug`]: mortar_core::incr::LayoutInput::debug

use std::collections::{BTreeMap, BTreeSet, HashMap};

use mortar_core::incr::{IncrReason, LayoutDelta, StoredLayout, StoredSection, Unimplemented};
use mortar_core::layout::{Hole, HoleKind, Place};
use mortar_core::{ChunkId, FileId, OutSecId, SectionId};
use mortar_layout::slack::{self, Slack};

use super::rows::{self, DebugStrRow, GroupRow, debug_str_class};
use super::view::{Change, ChangedFile, ChangedFiles, StateView};

const SHF_ALLOC: u64 = 0x2;
const SHF_COMPRESSED: u64 = 0x800;

fn debug_reason() -> IncrReason {
    IncrReason::UpdateNotImplemented(Some(Unimplemented::Debug))
}

// ---- the per-section table (record §1.2, IN-51b, R-10.3) --------------------

/// How an update places one debug output section's contributions.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Policy {
    /// IN-50's three placements: in place, best fit into a dead range,
    /// the tail.
    Allotted,
    /// `.debug_addr` (IN-51, R-4): in place when it fits, else the tail;
    /// a dead table is never reused.
    InPlaceOrTail,
    /// `.debug_loclists`, `.debug_frame` (R-10.3): the tail only, until
    /// the five-check experiment enables reuse in the middle.
    TailOnly,
    /// `.debug_aranges` (IN-54): rewritten whole, never placed.
    Whole,
    /// `.debug_str`, `.debug_line_str` (IN-56): merged; strings appended.
    Strings,
    /// The name indexes (IN-55): `debug-name-index`.
    NameIndex,
}

/// A debug output section's neutral form (IN-51b: the smallest neutral
/// length and the granule of its size) and its policy.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Form {
    pub min: u64,
    pub granule: u64,
    pub policy: Policy,
}

/// The form of a debug output section by name; `None` for a `.debug_*`
/// section the update does not know (`update-not-implemented (debug)`).
/// The minima are the largest by version (IN-51b; the six of
/// `mortar_layout::slack::neutral` equal its table: a test holds it).
#[must_use]
pub fn form(name: &[u8]) -> Option<Form> {
    use Policy::{Allotted, InPlaceOrTail, NameIndex, Strings, TailOnly, Whole};
    let (min, granule, policy) = match name {
        b".debug_info" => (15, 1, Allotted),
        b".debug_abbrev" => (1, 1, Allotted),
        b".debug_line" => (32, 1, Allotted),
        b".debug_rnglists" => (12, 1, Allotted),
        b".debug_str_offsets" => (8, 4, Allotted),
        b".debug_ranges" => (16, 1, Allotted),
        b".debug_loc" | b".debug_macinfo" => {
            (if name == b".debug_loc" { 16 } else { 1 }, 1, Allotted)
        }
        b".debug_macro" => (7, 1, Allotted),
        b".debug_types" => (26, 1, Allotted),
        b".debug_addr" => (8, 8, InPlaceOrTail),
        b".debug_loclists" => (12, 1, TailOnly),
        b".debug_frame" => (13, 1, TailOnly),
        b".debug_aranges" => (0, 1, Whole),
        b".debug_str" | b".debug_line_str" => (0, 1, Strings),
        b".debug_pubnames"
        | b".debug_pubtypes"
        | b".debug_names"
        | b".debug_gnu_pubnames"
        | b".debug_gnu_pubtypes" => (0, 1, NameIndex),
        _ => return None,
    };
    Some(Form { min, granule, policy })
}

/// A contribution's slack beyond `size` (IN-50, IN-51b): the full
/// link's `mortar_layout::slack::Slack::allotment` with slack on. Only
/// the sections of `slack::neutral` take slack in a full link; the update
/// gives a contribution it places at the tail the same (none for the
/// others).
#[must_use]
pub fn slack_of(name: &[u8], size: u64) -> u64 {
    slack::neutral(name).map_or(0, |n| Slack::INCREMENTAL.allotment(size, n))
}

/// Whether `size` bytes fit `room` leaving a remainder of 0 or one IN-51b
/// form (IN-50's remainder condition, IN-53).
#[must_use]
pub fn fits(form: Form, room: u64, size: u64) -> bool {
    room >= size && {
        let rest = room - size;
        rest == 0 || (rest >= form.min && rest.is_multiple_of(form.granule))
    }
}

// ---- IN-50's choice, a pure function ------------------------------------------

/// Which of IN-50's placements a contribution took
/// (`--incremental-report`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Taken {
    InPlace,
    DeadRange,
    Tail,
}

/// A contribution's allotment: offset in its chunk and size (its bytes
/// and the slack after them).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Allot {
    pub offset: u64,
    pub size: u64,
}

/// IN-50 for one contribution of `size` bytes aligned to `align`:
/// `old` its file's current allotment, `dead` the chunk's dead ranges
/// (`DeadDebug` holes, by offset), `end` the chunk's size and `capacity`
/// its capacity (the tail reserve is the difference, IN-33). Returns the
/// placement, the new allotment (in place: the old one; a dead range:
/// exactly `size`, the remainder stays dead; the tail: `size` plus its
/// slack); `None` when it fits nowhere (`debug-compaction`, IN-57).
#[must_use]
pub fn choose(
    name: &[u8],
    old: Option<Allot>,
    size: u64,
    align: u64,
    dead: &[Hole],
    end: u64,
    capacity: u64,
) -> Option<(Taken, Allot)> {
    let f = form(name)?;
    let align = align.max(1);
    if f.policy != Policy::TailOnly
        && let Some(o) = old
        && o.offset.is_multiple_of(align)
        && fits(f, o.size, size)
    {
        return Some((Taken::InPlace, o));
    }
    if f.policy == Policy::Allotted {
        let best = dead
            .iter()
            .filter(|h| h.kind == HoleKind::DeadDebug && h.offset.is_multiple_of(align))
            .filter(|h| fits(f, h.size, size))
            .min_by_key(|h| (h.size, h.offset));
        if let Some(h) = best {
            return Some((Taken::DeadRange, Allot { offset: h.offset, size }));
        }
    }
    let at = end.checked_next_multiple_of(align)?;
    let allot = size + slack_of(name, size);
    (at.checked_add(allot)? <= capacity).then_some((Taken::Tail, Allot { offset: at, size: allot }))
}

/// IN-57's first trigger: the dead bytes summed over the debug sections
/// (strings excluded, record §2.2: a string nobody names is not counted)
/// exceed a quarter of their summed size.
#[must_use]
pub fn over_quarter(dead: u64, size: u64) -> bool {
    dead.saturating_mul(4) > size
}

// ---- strings (IN-56, R-2, B1-4) -------------------------------------------------

/// The changed files' debug strings (IN-56): where each string of their
/// `.debug_str`/`.debug_line_str` input sections is in the output, and
/// what the update appends.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct DebugStrings {
    /// Per (file, class): each string's input offset and output offset,
    /// sorted by input offset.
    pub maps: BTreeMap<(FileId, u8), Vec<(u64, u32)>>,
    /// Per class: the merged chunk, the offset the appended bytes start
    /// at (the chunk's previous size) and the bytes.
    pub appended: BTreeMap<u8, (ChunkId, u64, Vec<u8>)>,
    /// `StatePatch::debug_strs`' puts: one per appended string.
    pub rows: Vec<DebugStrRow>,
    /// Hits verified, misses (no row, a row that failed verification).
    pub hits: u64,
    pub misses: u64,
}

impl DebugStrings {
    /// The output offset of the string at `input` in `file`'s input
    /// section of class `class` (a `DW_FORM_strp` operand is its
    /// relocation's `S + A`, an offset into the input section); a
    /// reference into the middle of a string keeps its distance.
    #[must_use]
    pub fn output_offset(&self, file: FileId, class: u8, input: u64) -> Option<u64> {
        let map = self.maps.get(&(file, class))?;
        let i = map.partition_point(|&(at, _)| at <= input).checked_sub(1)?;
        let (at, out) = map[i];
        Some(u64::from(out) + (input - at))
    }
}

/// The class of a merged debug string section by name.
#[must_use]
pub fn str_class(name: &[u8]) -> Option<u8> {
    match name {
        b".debug_str" => Some(debug_str_class::STR),
        b".debug_line_str" => Some(debug_str_class::LINE_STR),
        _ => None,
    }
}

/// The offset an appended string of `len` bytes (without its NUL) takes
/// in a merged section of previous size `size` and `capacity` after
/// `pending` appended bytes: `debug-str-limit` when it does not fit the
/// 32-bit word that names it (B1-5), `debug-compaction` when the tail is
/// full (IN-57).
///
/// # Errors
/// Those two reasons.
pub fn append_at(size: u64, capacity: u64, pending: u64, len: u64) -> Result<u32, IncrReason> {
    let at = size.saturating_add(pending);
    let end = at.saturating_add(len).saturating_add(1);
    let off = u32::try_from(at).map_err(|_| {
        IncrReason::DebugStrLimit(format!("an appended debug string at offset {at:#x}"))
    })?;
    if end > capacity {
        return Err(IncrReason::DebugCompaction(format!(
            "the debug string tail is full ({end:#x} > {capacity:#x})"
        )));
    }
    Ok(off)
}

/// IN-56 (R-2, B1-4): each string of the changed files' merged debug
/// string sections is looked up in `DEBUG_STRS` ([`StateRows::debug_str`],
/// an accelerator, never trusted), every hit verified against
/// `previous` (the previous output's bytes: the string and its NUL at
/// the offset, inside the section), and appended on a miss or a failed
/// check. `inputs`: per changed file, the class and the bytes of its
/// input section. Called before `layout` and [`place`].
///
/// [`StateRows::debug_str`]: super::view::StateRows::debug_str
///
/// # Errors
/// `state-corrupt` as the rows' reads; `new-output-section` for a class
/// the previous output has no section of; `debug-str-limit`,
/// `debug-compaction` ([`append_at`]).
pub fn strings(
    view: &StateView<'_>,
    stored: &StoredLayout<'_>,
    inputs: &[(FileId, u8, &[u8])],
    previous: &[u8],
) -> Result<DebugStrings, IncrReason> {
    strings_with(&|out, hash| view.debug_str(out, hash), stored, inputs, previous)
}

/// [`strings`] over any lookup (the tests' corrupt rows).
///
/// # Errors
/// As [`strings`].
pub fn strings_with(
    lookup: &dyn Fn(u8, u64) -> Result<Option<u32>, IncrReason>,
    stored: &StoredLayout<'_>,
    inputs: &[(FileId, u8, &[u8])],
    previous: &[u8],
) -> Result<DebugStrings, IncrReason> {
    let mut out = DebugStrings::default();
    if inputs.is_empty() {
        return Ok(out);
    }
    let names: BTreeMap<OutSecId, Vec<u8>> = stored.outsecs()?.into_iter().collect();
    let chunks = stored.chunks()?;
    let chunk_of = |class: u8| {
        chunks.iter().find(|(_, c)| {
            c.kind.out().and_then(|o| names.get(&o)).and_then(|n| str_class(n)) == Some(class)
        })
    };
    let mut new: HashMap<(u8, &[u8]), u32> = HashMap::new();
    for &(file, class, bytes) in inputs {
        let Some((cid, chunk)) = chunk_of(class) else {
            return Err(IncrReason::NewOutputSection(format!(
                "debug string class {class} brought by a changed file"
            )));
        };
        let mut map = Vec::new();
        let mut at = 0usize;
        while at < bytes.len() {
            let len = bytes[at..].iter().position(|&b| b == 0).unwrap_or(bytes.len() - at);
            let s = &bytes[at..at + len];
            let hit = match lookup(class, rows::name_hash(s, b""))? {
                Some(o) if verified(previous, chunk.offset, chunk.size, u64::from(o), s) => Some(o),
                _ => None,
            };
            let off = if let Some(o) = hit {
                out.hits += 1;
                o
            } else if let Some(&o) = new.get(&(class, s)) {
                o
            } else {
                out.misses += 1;
                let entry = out.appended.entry(class).or_insert((*cid, chunk.size, Vec::new()));
                let o =
                    append_at(chunk.size, chunk.capacity, entry.2.len() as u64, s.len() as u64)?;
                entry.2.extend_from_slice(s);
                entry.2.push(0);
                new.insert((class, s), o);
                out.rows.push(DebugStrRow { out: class, hash: rows::name_hash(s, b""), offset: o });
                o
            };
            map.push((at as u64, off));
            at += len + 1;
        }
        out.maps.insert((file, class), map);
    }
    out.rows.sort_unstable();
    Ok(out)
}

/// Whether the previous output holds `s` and its NUL at `off` of the
/// section at file offset `base`, `size` bytes long (R-2.1).
fn verified(previous: &[u8], base: u64, size: u64, off: u64, s: &[u8]) -> bool {
    let len = s.len() as u64;
    if off.saturating_add(len) >= size {
        return false;
    }
    let Some(start) = base.checked_add(off).and_then(|a| usize::try_from(a).ok()) else {
        return false;
    };
    previous.get(start..start + s.len()) == Some(s) && previous.get(start + s.len()) == Some(&0)
}

// ---- COMDAT groups (R-5 on image.md I-47a) ---------------------------------------

/// The keeper of a signature after the update: of the stored `GROUPS`
/// rows minus the changed files' (their old versions), plus the changed
/// files whose new version has a copy, the least current rank.
///
/// # Errors
/// `state-corrupt` as the reads.
pub fn keeper_now(
    view: &StateView<'_>,
    changed: &ChangedFiles,
    signature: &[u8],
) -> Result<Option<u32>, IncrReason> {
    let is_changed = |f: u32| changed.files.binary_search_by_key(&FileId(f), |c| c.file).is_ok();
    let mut rows: Vec<GroupRow> =
        view.group(signature)?.into_iter().filter(|r| !is_changed(r.file)).collect();
    for c in &changed.files {
        if has_copy(view, changed, c, signature)? {
            rows.push(GroupRow {
                signature: crate::format::Str::default(),
                file: c.file.0,
                tomb: false,
            });
        }
    }
    let rank = |f: u32| -> Result<u64, IncrReason> {
        match changed.files.binary_search_by_key(&FileId(f), |c| c.file) {
            Ok(i) => Ok(changed.files[i].row.rank),
            Err(_) => Ok(view.file(FileId(f))?.rank),
        }
    };
    Ok(rows::keeper(&rows, rank)?.map(|r| r.file))
}

fn has_copy(
    view: &StateView<'_>,
    changed: &ChangedFiles,
    c: &ChangedFile,
    signature: &[u8],
) -> Result<bool, IncrReason> {
    if matches!(c.change, Change::Gone | Change::NoLongerExtracted) {
        return Ok(false);
    }
    for s in &c.sections {
        if s.anchor_kind == 1 && changed.string(view, s.anchor)? == signature {
            return Ok(true);
        }
    }
    Ok(false)
}

/// R-5 (c), image.md I-47a: the value of an import (a `DW_MACRO_import`
/// or any debug word) naming `name` of `signature`'s group, `size` bytes,
/// from a copy the importer does not keep: the KEPT copy's place when
/// the keeper's one same-named, non-allocated, same-size member is live
/// and placed, else `None` (I-47's tombstone). The ONE function that
/// chooses an import's value. The keeper's place comes from `layout`
/// when the keeper is a file of this update, else from its stored
/// `SECTIONS` row (`StoredLayout::sections_of(keeper)` filtered by the
/// signature, the name, the flags and the size); the keeper's input file
/// is never read.
///
/// # Errors
/// `state-corrupt` as the reads.
pub fn import_target(
    view: &StateView<'_>,
    stored: &StoredLayout<'_>,
    changed: &ChangedFiles,
    layout: &LayoutDelta,
    signature: &[u8],
    name: &[u8],
    size: u64,
) -> Result<Option<(ChunkId, u64)>, IncrReason> {
    let Some(k) = keeper_now(view, changed, signature)? else { return Ok(None) };
    let keeper = FileId(k);
    if let Ok(i) = changed.files.binary_search_by_key(&keeper, |c| c.file) {
        let c = &changed.files[i];
        let mut found = None;
        for (j, s) in c.sections.iter().enumerate() {
            if s.anchor_kind == 1
                && s.sh_flags & SHF_ALLOC == 0
                && s.size == size
                && changed.string(view, s.anchor)? == signature
                && changed.string(view, s.name)? == name
            {
                if found.is_some() {
                    return Ok(None);
                }
                let id = SectionId(c.first_section.0 + j as u32);
                found = layout.placement.place.get(id).and_then(|p| p.get());
            }
        }
        return Ok(found);
    }
    let rows: Vec<StoredSection> = stored
        .sections_of(keeper)?
        .into_iter()
        .filter(|s| {
            s.anchor_kind == 1
                && s.anchor == signature
                && s.name == name
                && s.sh_flags & SHF_ALLOC == 0
                && s.size == size
        })
        .collect();
    Ok(match rows.as_slice() {
        [one] => one.place.get(),
        _ => None,
    })
}

// ---- `.debug_aranges` (IN-54) ----------------------------------------------------

/// IN-54: `.debug_aranges` rewritten whole, in full-link order: the sets
/// of `previous` (the previous output's section, sets in full-link
/// order) whose `debug_info_offset` lies in a replaced file's OLD
/// `.debug_info` contribution are dropped, and the file's new sets
/// (their offsets already relocated) go where its first old set was. A
/// file with new sets and no old set (an appeared file, or one that had
/// none) has no position this function can know without a whole read of
/// `SECTIONS`: `update-not-implemented (debug)` in v1.
///
/// # Errors
/// `update-not-implemented (debug)` for a set this function cannot
/// parse (DWARF64, a length past the section) or the case above.
pub fn aranges(previous: &[u8], replaced: &[(u64, u64, Vec<u8>)]) -> Result<Vec<u8>, IncrReason> {
    let mut out = Vec::with_capacity(previous.len());
    let mut done = vec![false; replaced.len()];
    let mut at = 0usize;
    while at < previous.len() {
        let len = previous.get(at..at + 4).ok_or_else(debug_reason)?;
        let len = u32::from_le_bytes([len[0], len[1], len[2], len[3]]);
        if len == 0 {
            // Zero padding after the last set (a full link writes none).
            out.extend_from_slice(&previous[at..]);
            break;
        }
        if len >= 0xffff_fff0 {
            return Err(debug_reason());
        }
        let end = at.checked_add(4 + len as usize).filter(|&e| e <= previous.len());
        let end = end.ok_or_else(debug_reason)?;
        let info = previous.get(at + 6..at + 10).ok_or_else(debug_reason)?;
        let info = u64::from(u32::from_le_bytes([info[0], info[1], info[2], info[3]]));
        match replaced.iter().position(|&(s, e, _)| (s..e).contains(&info)) {
            Some(i) => {
                if !done[i] {
                    out.extend_from_slice(&replaced[i].2);
                    done[i] = true;
                }
            }
            None => out.extend_from_slice(&previous[at..end]),
        }
        at = end;
    }
    if done.iter().zip(replaced).any(|(&d, r)| !d && !r.2.is_empty()) {
        return Err(debug_reason());
    }
    Ok(out)
}

// ---- the placement (IN-50, IN-51, IN-53, IN-57, R-3, R-5, R-8) ----------------

/// One placed contribution of a changed file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Contribution {
    pub file: FileId,
    pub chunk: ChunkId,
    /// Its members, in section-index order, each with its offset in the
    /// chunk.
    pub members: Vec<(SectionId, u64)>,
    /// Its bytes (the last member's end minus the first's offset).
    pub size: u64,
    pub allot: Allot,
    pub taken: Taken,
}

/// What [`place`] decided beyond the layout (B1-4: it carries the
/// changed files' string maps and the appended bytes).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct DebugPlan {
    pub strings: DebugStrings,
    /// Every placed contribution of a changed file
    /// (`--incremental-report` names `taken`).
    pub contributions: Vec<Contribution>,
    /// Per debug chunk this update touched: its holes after the update
    /// (also in `layout.placement.holes`; the image writes each in its
    /// section's neutral form, R-3).
    pub chunks: Vec<(ChunkId, Vec<Hole>)>,
    /// IN-54's input: per changed or removed file, its OLD `.debug_info`
    /// contribution `[start, end)` (chunk offsets), whose sets
    /// [`aranges`] replaces.
    pub aranges: Vec<(FileId, u64, u64)>,
    /// R-5 (a): kept group copies left at their old place, `(new, old)`:
    /// the join compares the new copy's relocated bytes with the previous
    /// output's at the old place; a difference is `debug-comdat-keeper`.
    pub comdat_in_place: Vec<(SectionId, SectionId)>,
}

/// A changed file's debug sections of one output section.
struct Run {
    name: Vec<u8>,
    members: Vec<(SectionId, u64, u64)>, // id, size, align
}

/// IN-50..IN-57: the debug placement of an update, into `layout` (the
/// result of `layout` with `LayoutInput::debug`). Places every live debug
/// input section of the changed files (in place, best fit into a dead
/// range, or at the tail), makes the old allotments and the removed
/// files' contributions dead ranges (always in neutral form, R-3),
/// writes the touched debug chunks' holes into `layout` (and the chunks
/// into `hole_chunks`), moves the placed sections out of `killed` into
/// `moved` (place changed) or `revived` (no old row), grows the chunks
/// whose tail took a contribution, and returns what the image needs.
///
/// Named fall-backs: `debug-growth` (a growth step in an output with
/// debug sections, R-8), `debug-name-index` (IN-55), `debug-unit-kind`
/// (a compressed debug input section, B1), `debug-comdat-keeper` (R-5
/// b), `debug-compaction` (IN-57: fits nowhere, or dead over a quarter),
/// `update-not-implemented (debug)` (a debug section mortar's table does
/// not know, a debug output section in more than one chunk, a section of
/// an UNCHANGED file that died or revived: G-3 lifts it with liveness).
///
/// # Errors
/// Those reasons, and `state-corrupt` as the reads.
#[allow(clippy::too_many_lines, reason = "IN-50's steps in order")]
pub fn place(
    view: &StateView<'_>,
    stored: &StoredLayout<'_>,
    changed: &ChangedFiles,
    strings: DebugStrings,
    layout: &mut LayoutDelta,
) -> Result<DebugPlan, IncrReason> {
    let names: BTreeMap<OutSecId, Vec<u8>> = stored.outsecs()?.into_iter().collect();
    // The debug chunks by output-section name (one chunk each, or refused).
    let mut chunk_of: BTreeMap<Vec<u8>, ChunkId> = BTreeMap::new();
    for (id, c) in layout.placement.chunks.iter() {
        let Some(n) = c.kind.out().and_then(|o| names.get(&o)) else { continue };
        if c.is_alloc() || !n.starts_with(b".debug_") {
            continue;
        }
        if chunk_of.insert(n.clone(), id).is_some() {
            return Err(debug_reason());
        }
    }
    if chunk_of.is_empty() {
        return Ok(DebugPlan { strings, ..DebugPlan::default() });
    }
    if let Some(g) = layout.growth_steps.first() {
        return Err(IncrReason::DebugGrowth(format!(
            "a growth step of {} bytes in an output with debug sections",
            g.step
        )));
    }
    let is_debug = |name: &[u8], flags: u64| flags & SHF_ALLOC == 0 && name.starts_with(b".debug_");
    let files: BTreeSet<FileId> = changed.files.iter().map(|c| c.file).collect();

    // A section of an unchanged file listed killed or revived is
    // liveness's (G-3): the named fall-back of this delivery.
    for &id in layout.killed.iter().chain(&layout.revived) {
        if let Some(row) = stored.section(id)?
            && !files.contains(&row.file)
            && is_debug(&row.name, row.sh_flags)
        {
            return Err(debug_reason());
        }
    }

    let mut plan = DebugPlan { strings, ..DebugPlan::default() };
    // Per chunk: the holes as they stand (the stored ones, or the
    // layout's), edited below.
    let mut holes: BTreeMap<ChunkId, Vec<Hole>> = BTreeMap::new();
    let mut grown: BTreeMap<ChunkId, u64> = BTreeMap::new();
    let mut succeeded: BTreeSet<SectionId> = BTreeSet::new();
    let mut moved_now: Vec<SectionId> = Vec::new();
    let mut revived_now: Vec<SectionId> = Vec::new();
    let mut placed: Vec<(SectionId, Place)> = Vec::new();

    for c in &changed.files {
        let old_rows: Vec<StoredSection> = if matches!(c.change, Change::Appeared) {
            Vec::new()
        } else {
            stored.sections_of(c.file)?
        };
        // B3-7 (G-1c): a DWARF 4 split-DWARF file (`DW_AT_GNU_addr_base`)
        // has a HEADERLESS `.debug_addr` table, which a dead range must
        // keep named by a unit with that base; `Hole` carries no such
        // variant yet, so a changed or removed file that is one, in its
        // old or its new version, falls back.
        let new_names: Vec<(Vec<u8>, u64)> = c
            .sections
            .iter()
            .map(|s| Ok((changed.string(view, s.name)?, s.size)))
            .collect::<Result<_, IncrReason>>()?;
        if headerless_addr(old_rows.iter().map(|r| (&r.name[..], r.size)))
            || headerless_addr(new_names.iter().map(|(n, s)| (&n[..], *s)))
        {
            return Err(IncrReason::DebugUnitKind(
                "a DWARF 4 split-DWARF file (a headerless .debug_addr table): its dead \
                 range must stay named (G-1c)"
                    .to_owned(),
            ));
        }
        // The old version's debug contributions, by output section.
        let mut old: BTreeMap<Vec<u8>, (Allot, Vec<&StoredSection>)> = BTreeMap::new();
        for r in &old_rows {
            if !is_debug(&r.name, r.sh_flags) {
                continue;
            }
            let Some((ch, off)) = r.place.get() else { continue };
            let Some(f) = form(&r.name) else { return Err(debug_reason()) };
            if matches!(f.policy, Policy::Strings | Policy::Whole) {
                continue;
            }
            if chunk_of.get(&r.name) != Some(&ch) {
                return Err(debug_reason());
            }
            let e =
                old.entry(r.name.clone()).or_insert((Allot { offset: off, size: 0 }, Vec::new()));
            let end = (e.0.offset + e.0.size).max(off + r.size);
            e.0.offset = e.0.offset.min(off);
            e.0.size = end - e.0.offset;
            e.1.push(r);
        }
        if let Some((_, rows)) = old.get(&b".debug_info"[..]) {
            let s = rows.iter().filter_map(|r| r.place.get()).map(|p| p.1).min().unwrap_or(0);
            let e = rows.iter().filter_map(|r| Some(r.place.get()?.1 + r.size)).max().unwrap_or(0);
            plan.aranges.push((c.file, s, e));
        }
        // R-5 (b): a group this file kept and keeps no more (dropped, or
        // re-elected to another file) moves the kept copy.
        for r in &old_rows {
            if r.anchor_kind != 1 || !is_debug(&r.name, r.sh_flags) || r.place.get().is_none() {
                continue;
            }
            let before = rows::keeper(&view.group(&r.anchor)?, |f| Ok(view.file(FileId(f))?.rank))?;
            if before.map(|k| k.file) == Some(c.file.0)
                && keeper_now(view, changed, &r.anchor)? != Some(c.file.0)
            {
                return Err(IncrReason::DebugComdatKeeper(format!(
                    "{}: the kept copy of {} moves to another file",
                    text(&r.name),
                    text(&r.anchor)
                )));
            }
        }
        // The new version's live debug sections, by output section.
        let mut runs: Vec<Run> = Vec::new();
        for (k, s) in c.sections.iter().enumerate() {
            let name = changed.string(view, s.name)?;
            if !is_debug(&name, s.sh_flags) {
                continue;
            }
            if s.sh_flags & SHF_COMPRESSED != 0 {
                return Err(IncrReason::DebugUnitKind(format!("{}: compressed", text(&name))));
            }
            let id = SectionId(c.first_section.0 + k as u32);
            let f = form(&name).ok_or_else(debug_reason)?;
            match f.policy {
                // IN-55 option (A): the GNU sets are left out of an
                // incremental-mode output (`incr_discard`), as in the
                // full link: nothing to place (`split::classify_*` gives
                // them no reason either). B3-7: split DWARF's skeletons
                // (GCC and Clang write the sets with `-gsplit-dwarf`).
                Policy::NameIndex if mortar_layout::incr_discard::left_out(&name) => continue,
                Policy::NameIndex => return Err(IncrReason::DebugNameIndex(text(&name))),
                Policy::Strings | Policy::Whole => continue,
                _ => {}
            }
            if s.anchor_kind == 1 {
                // R-5: a group member is live only in the keeper's copy.
                let sig = changed.string(view, s.anchor)?;
                let before = rows::keeper(&view.group(&sig)?, |f| Ok(view.file(FileId(f))?.rank))?
                    .map(|r| r.file);
                let now = keeper_now(view, changed, &sig)?;
                if now != Some(c.file.0) {
                    if before == Some(c.file.0) {
                        return Err(IncrReason::DebugComdatKeeper(format!(
                            "{}: the keeper's copy of {} moves to another file",
                            text(&name),
                            text(&sig)
                        )));
                    }
                    continue;
                }
                if before.is_some() && before != Some(c.file.0) {
                    return Err(IncrReason::DebugComdatKeeper(format!(
                        "{}: {} is kept by another file now",
                        text(&name),
                        text(&sig)
                    )));
                }
                if before == Some(c.file.0) {
                    // (a): the same named, same-size copy stays in place.
                    let same = old.get(&name).and_then(|(_, rows)| {
                        rows.iter()
                            .find(|r| r.anchor_kind == 1 && r.anchor == sig && r.size == s.size)
                    });
                    let Some(o) = same else {
                        return Err(IncrReason::DebugComdatKeeper(format!(
                            "{}: the kept copy of {} changed size",
                            text(&name),
                            text(&sig)
                        )));
                    };
                    let p = o.place;
                    placed.push((id, p));
                    succeeded.insert(o.id);
                    plan.comdat_in_place.push((id, o.id));
                    continue;
                }
            }
            match runs.iter_mut().find(|r| r.name == name) {
                Some(r) => r.members.push((id, s.size, s.align)),
                None => runs.push(Run { name, members: vec![(id, s.size, s.align)] }),
            }
        }
        // Group copies kept in place are not part of the run's allotment:
        // the old contribution minus them is what the run may reuse.
        let in_place: BTreeSet<SectionId> = plan.comdat_in_place.iter().map(|&(_, o)| o).collect();
        for run in &runs {
            let chunk = *chunk_of.get(&run.name).ok_or_else(|| {
                IncrReason::NewOutputSection(format!(
                    "output section {} brought by a changed file",
                    text(&run.name)
                ))
            })?;
            let align = run.members.iter().map(|m| m.2.max(1)).max().unwrap_or(1);
            let mut rel = Vec::new();
            let mut size = 0u64;
            for &(id, sz, al) in &run.members {
                let at = size.checked_next_multiple_of(al.max(1)).ok_or_else(debug_reason)?;
                rel.push((id, at));
                size = at + sz;
            }
            let hs = chunk_holes(&mut holes, layout, stored, chunk)?;
            let whole = old.get(&run.name).map(|(a, _)| with_slack(*a, hs));
            let kept = kept_ranges(old.get(&run.name).map(|o| &o.1), &in_place);
            let old_allot = if kept.is_empty() { whole } else { None };
            if let Some(w) = whole
                && !kept.is_empty()
            {
                // Group copies kept in place (R-5 a) hold their bytes; the
                // rest of the old allotment is dead before the choice.
                let hs = holes.get_mut(&chunk).expect("read above");
                free_around(hs, chunk, w, &kept, &run.name)?;
            }
            let hs = &holes[&chunk];
            let ch = &layout.placement.chunks[chunk];
            let end = grown.get(&chunk).copied().unwrap_or(ch.size);
            let (taken, allot) = choose(&run.name, old_allot, size, align, hs, end, ch.capacity)
                .ok_or_else(|| {
                    IncrReason::DebugCompaction(format!(
                        "a contribution of {size} bytes to {} fits nowhere",
                        text(&run.name)
                    ))
                })?;
            if taken == Taken::Tail && run.name == b".debug_abbrev" {
                // IN-51: the neutral table ENDS `.debug_abbrev`; a
                // contribution after it is not in the rules (record B2).
                return Err(IncrReason::DebugCompaction(
                    "a .debug_abbrev contribution past the neutral table".into(),
                ));
            }
            let hs = holes.get_mut(&chunk).expect("read above");
            // The old allotment, when left, becomes one dead range (R-3).
            if let Some(o) = old_allot {
                remove_range(hs, o.offset, o.offset + o.size);
                if taken != Taken::InPlace {
                    hs.push(dead(chunk, o.offset, o.size, &run.name)?);
                }
            }
            match taken {
                Taken::DeadRange => {
                    let h = *hs
                        .iter()
                        .find(|h| h.offset == allot.offset && h.kind == HoleKind::DeadDebug)
                        .expect("chosen from these holes");
                    remove_range(hs, h.offset, h.offset + h.size);
                    if h.size > size {
                        hs.push(dead(chunk, h.offset + size, h.size - size, &run.name)?);
                    }
                }
                Taken::Tail => {
                    grown.insert(chunk, allot.offset + allot.size);
                }
                Taken::InPlace => {}
            }
            if allot.size > size && taken != Taken::DeadRange {
                hs.push(Hole {
                    chunk,
                    offset: allot.offset + size,
                    size: allot.size - size,
                    kind: HoleKind::Debug,
                });
            }
            hs.sort_by_key(|h| h.offset);
            let members: Vec<(SectionId, u64)> =
                rel.iter().map(|&(id, at)| (id, allot.offset + at)).collect();
            for &(id, at) in &members {
                placed.push((id, Place::at(chunk, at)));
            }
            plan.contributions.push(Contribution {
                file: c.file,
                chunk,
                members,
                size,
                allot,
                taken,
            });
        }
        // Old contributions with no new run: dead (a removed file, a
        // section the new version no longer has).
        for (name, (a, rows)) in &old {
            if runs.iter().any(|r| &r.name == name) {
                continue;
            }
            let chunk = chunk_of[name];
            let kept = kept_ranges(Some(rows), &in_place);
            let hs = chunk_holes(&mut holes, layout, stored, chunk)?;
            let a = with_slack(*a, hs);
            let hs = holes.get_mut(&chunk).expect("read above");
            free_around(hs, chunk, a, &kept, name)?;
        }
    }

    // IN-57's quarter, over every debug chunk but the strings'.
    let (mut dead_bytes, mut total) = (0u64, 0u64);
    for (name, &chunk) in &chunk_of {
        if matches!(form(name).map(|f| f.policy), Some(Policy::Strings | Policy::Whole)) {
            continue;
        }
        let hs = chunk_holes(&mut holes, layout, stored, chunk)?;
        dead_bytes +=
            hs.iter().filter(|h| h.kind == HoleKind::DeadDebug).map(|h| h.size).sum::<u64>();
        total += grown.get(&chunk).copied().unwrap_or(layout.placement.chunks[chunk].size);
    }
    if over_quarter(dead_bytes, total) {
        return Err(IncrReason::DebugCompaction(format!(
            "{dead_bytes} dead debug bytes of {total}"
        )));
    }

    // Into the layout.
    let mut touched: BTreeSet<ChunkId> = BTreeSet::new();
    for (c, hs) in &holes {
        if layout.holes_of(stored, *c)? != *hs {
            touched.insert(*c);
        }
    }
    for (&chunk, &size) in &grown {
        layout.placement.chunks[chunk].size = size;
    }
    let len = placed.iter().map(|(id, _)| id.0 + 1).max().unwrap_or(0);
    while layout.placement.place.len() < len {
        layout.placement.place.push(Place::NONE).map_err(|_| debug_reason())?;
    }
    for &(id, p) in &placed {
        layout.placement.place[id] = p;
    }
    // Moved / revived by the old row each new section replaces (same
    // name and ordinal among the file's same-named debug sections).
    for c in &changed.files {
        let olds: Vec<StoredSection> = if matches!(c.change, Change::Appeared) {
            Vec::new()
        } else {
            stored.sections_of(c.file)?
        };
        let mut seen: BTreeMap<Vec<u8>, u32> = BTreeMap::new();
        for (k, s) in c.sections.iter().enumerate() {
            let name = changed.string(view, s.name)?;
            if !is_debug(&name, s.sh_flags) {
                continue;
            }
            let n = seen.entry(name.clone()).or_default();
            let ord = *n;
            *n += 1;
            let id = SectionId(c.first_section.0 + k as u32);
            let Some(now) = placed.iter().find(|(i, _)| *i == id).map(|p| p.1) else { continue };
            let prev = olds.iter().filter(|o| o.name == name).nth(ord as usize);
            match prev.and_then(|o| o.place.get().map(|p| (o, p))) {
                Some((o, p)) => {
                    succeeded.insert(o.id);
                    if Some(p) != now.get() {
                        moved_now.push(id);
                    }
                }
                None => revived_now.push(id),
            }
        }
    }
    // An old row a placed section replaced is not killed; one with no
    // successor (a dead contribution) stays in `killed`.
    layout.killed.retain(|id| !succeeded.contains(id));
    layout.moved.extend(moved_now);
    layout.moved.sort_unstable();
    layout.moved.dedup();
    layout.revived.extend(revived_now);
    layout.revived.sort_unstable();
    layout.revived.dedup();
    if !touched.is_empty() {
        let mut all: Vec<Hole> = layout
            .placement
            .holes
            .iter()
            .filter(|h| !touched.contains(&h.chunk))
            .copied()
            .collect();
        for c in &touched {
            let hs = &holes[c];
            all.extend_from_slice(hs);
            let before = layout.holes_of(stored, *c)?;
            layout.holes.extend(hs.iter().filter(|h| !before.contains(h)).copied());
            plan.chunks.push((*c, hs.clone()));
        }
        all.sort_by_key(|h| (h.chunk, h.offset));
        layout.placement.holes = all.into_boxed_slice();
        layout.holes.sort_by_key(|h| (h.chunk, h.offset));
        layout.hole_chunks.extend(touched.iter().copied());
        layout.hole_chunks.sort_unstable();
        layout.hole_chunks.dedup();
    }
    Ok(plan)
}

/// The holes of `chunk` as this update stands, read once.
fn chunk_holes<'h>(
    holes: &'h mut BTreeMap<ChunkId, Vec<Hole>>,
    layout: &LayoutDelta,
    stored: &StoredLayout<'_>,
    chunk: ChunkId,
) -> Result<&'h [Hole], IncrReason> {
    if let std::collections::btree_map::Entry::Vacant(e) = holes.entry(chunk) {
        e.insert(layout.holes_of(stored, chunk)?);
    }
    Ok(&holes[&chunk])
}

/// The `(offset, size)` of the old rows kept in place (R-5 a), by offset.
fn kept_ranges(
    rows: Option<&Vec<&StoredSection>>,
    in_place: &BTreeSet<SectionId>,
) -> Vec<(u64, u64)> {
    let mut v: Vec<(u64, u64)> = rows
        .into_iter()
        .flatten()
        .filter(|r| in_place.contains(&r.id))
        .filter_map(|r| Some((r.place.get()?.1, r.size)))
        .collect();
    v.sort_unstable();
    v
}

/// `whole` (an old allotment) becomes dead ranges (R-3) around the
/// `kept` ranges inside it.
fn free_around(
    hs: &mut Vec<Hole>,
    chunk: ChunkId,
    whole: Allot,
    kept: &[(u64, u64)],
    name: &[u8],
) -> Result<(), IncrReason> {
    remove_range(hs, whole.offset, whole.offset + whole.size);
    let mut at = whole.offset;
    for &(o, sz) in kept.iter().chain(std::iter::once(&(whole.offset + whole.size, 0))) {
        if o > at {
            hs.push(dead(chunk, at, o - at, name)?);
        }
        at = at.max(o + sz);
    }
    hs.sort_by_key(|h| h.offset);
    Ok(())
}

/// The allotment: the contribution plus the `Debug` hole right after it.
fn with_slack(a: Allot, holes: &[Hole]) -> Allot {
    let end = a.offset + a.size;
    let slack = holes.iter().find(|h| h.kind == HoleKind::Debug && h.offset == end);
    Allot { offset: a.offset, size: a.size + slack.map_or(0, |h| h.size) }
}

/// Drops the holes inside `[s, e)` (a contribution's slack, a dead range
/// being reused).
fn remove_range(holes: &mut Vec<Hole>, s: u64, e: u64) {
    holes.retain(|h| h.offset + h.size <= s || h.offset >= e);
}

/// Whether one version of a file (its sections' names and sizes) has a
/// HEADERLESS `.debug_addr` table: a non-empty `.debug_addr` and no
/// `.debug_line_str`. A DWARF 5 producer that writes `.debug_addr` (GCC
/// and Clang, split or not) also writes `.debug_line_str`; DWARF 2 to 4
/// define no `.debug_line_str`, and their `.debug_addr` is GNU split
/// DWARF's (`split::Handled::dwarf4_split`, here from names alone, as the
/// old version has no bytes). A DWARF 5 file without `.debug_line_str`
/// falls back too (conservative).
fn headerless_addr<'n>(sections: impl Iterator<Item = (&'n [u8], u64)>) -> bool {
    let (mut addr, mut line_str) = (false, false);
    for (n, _) in sections.filter(|s| s.1 > 0) {
        addr |= n == b".debug_addr";
        line_str |= n == b".debug_line_str";
    }
    addr && !line_str
}

/// A dead range in its section's neutral form (R-3): refused when the
/// range is below the form's minimum (no neutral form fits it).
fn dead(chunk: ChunkId, offset: u64, size: u64, name: &[u8]) -> Result<Hole, IncrReason> {
    let f = form(name).ok_or_else(debug_reason)?;
    if size < f.min || !size.is_multiple_of(f.granule) {
        return Err(IncrReason::DebugCompaction(format!(
            "a dead range of {size} bytes in {} has no neutral form",
            text(name)
        )));
    }
    Ok(Hole { chunk, offset, size, kind: HoleKind::DeadDebug })
}

fn text(b: &[u8]) -> String {
    String::from_utf8_lossy(b).into_owned()
}

#[cfg(test)]
mod tests {
    use super::*;

    // spec: incr.md IN-50, IN-51b (debug.rs's minima equal slack.rs's for the six)
    #[test]
    fn forms_agree_with_slack() {
        for n in [
            &b".debug_info"[..],
            b".debug_abbrev",
            b".debug_line",
            b".debug_rnglists",
            b".debug_str_offsets",
            b".debug_ranges",
        ] {
            let (f, s) = (form(n).unwrap(), slack::neutral(n).unwrap());
            assert_eq!((f.min, f.granule), (s.min, s.granule), "{}", text(n));
        }
        assert_eq!(slack_of(b".debug_info", 30), 18);
        assert_eq!(slack_of(b".debug_frame", 30), 0);
    }

    // spec: B1-5 (`debug-str-limit` from a faked section size near 2^32)
    #[test]
    fn debug_str_limit_near_four_gib() {
        let size = (1u64 << 32) - 8;
        assert_eq!(append_at(size, u64::MAX, 0, 4).unwrap(), u32::try_from(size).unwrap());
        let e = append_at(size, u64::MAX, 8, 4).unwrap_err();
        assert_eq!(e.name(), "debug-str-limit");
        let e = append_at(100, 104, 0, 4).unwrap_err();
        assert_eq!(e.name(), "debug-compaction");
    }
}
