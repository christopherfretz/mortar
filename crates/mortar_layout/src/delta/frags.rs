//! IN-72 item 6 (merged sections, S-110, L-147): the update's `FRAGS`
//! rows from the stored ones, each file's old `FRAG_USE` range and its new
//! version's pieces (`LayoutInput::pieces`). WP-242.
//!
//! `FRAGS.refs` counts FILES (IN-18e, WP-239 S4-2) and is exact after the
//! update (IN-78): every file of the update subtracts one from each
//! fragment of its old `FRAG_USE` range and adds one to each distinct
//! fragment its new pieces name. A file of `changes` that `input.pieces`
//! does not name has no pieces in its new version (`LayoutInput`'s
//! contract: a gone file, or a file whose new version has none, may be
//! given with an empty list or not at all). Only the keys whose count
//! changes are looked up (`frags_of`, one row a key; IN-18c).
//!
//! What the delta says (`LayoutDelta::frags`, by key): every fragment
//! whose `refs` or offset changed. A row whose `refs` is above 0 and whose
//! stored row is absent (appended) or had `refs` 0 (revived) is written
//! from its piece by U-6 (IN-72: "a fragment whose count rises from 0 is
//! rewritten from the piece", review-in72 M-8): U-6 tells the two apart
//! with `frags_of`. A row at 0 is a dead range: a constant takes the fill
//! (a `HoleKind::Fill` hole of its `entsize` bytes in `LayoutDelta::holes`),
//! a string keeps its bytes (a `HoleKind::DeadFragment` hole of its stored
//! `size`, seam 6: WP-242 C); a revived fragment's stored hole is gone from
//! `holes_of` of its chunk.

use std::collections::{BTreeMap, BTreeSet};

use mortar_core::column::Column;
use mortar_core::id::{ChunkId, FileId, OutSecId, SectionId};
use mortar_core::incr::{IncrReason, LayoutInput, SectionChange, StoredFrag, Unimplemented};
use mortar_core::layout::{Chunk, ChunkKind, Hole, HoleKind};

use super::{Base, admit, corrupt, not_implemented, round_up, text};

/// A fragment's identity: `FRAGS`' key `(out, class word, hash)` (WP-239
/// S4-1).
pub(super) type Key = (OutSecId, u32, [u8; 16]);

/// What item 6 gives the rest of the delta layout.
#[derive(Debug, Default)]
pub(super) struct Merged {
    /// `LayoutDelta::frags`, sorted by key.
    pub rows: Vec<StoredFrag>,
    /// Dead constants' fill holes (made by this update).
    pub made: Vec<Hole>,
    /// Stored dead-range holes of revived fragments (gone after it).
    pub gone: Vec<Hole>,
    /// Output sections a fragment of which fell to 0 / rose from 0 or was
    /// appended (item 10's "left empty", `Update::left_empty`).
    pub died: BTreeSet<OutSecId>,
    pub rose: BTreeSet<OutSecId>,
}

impl Merged {
    /// The merged chunks whose holes this update replaces.
    pub(super) fn chunks(&self) -> impl Iterator<Item = ChunkId> + '_ {
        self.made.iter().chain(&self.gone).map(|h| h.chunk)
    }
}

/// One appended or revived fragment: its piece's size (the fill or the
/// bytes U-6 writes).
#[derive(Debug, Clone, Copy)]
struct Piece {
    size: u64,
}

/// Whether `class` is a string class (`SHF_STRINGS`), its `entsize` and
/// its alignment in bytes.
const fn class_parts(word: u32) -> (bool, u64, u8) {
    (word >> 31 != 0, (word & 0x00ff_ffff) as u64, ((word >> 24) & 0x7f) as u8)
}

/// Item 6 over the files of the update (`files`, sorted). `sections`: the
/// live, kept changes whose fate is pieces (merge-shaped, not placed whole
/// before; synth.md S-6), by id; every piece must name one of them, and
/// each of them must have a piece. `chunks`: the update's chunks (a
/// merged chunk's size and alignment grow with an append).
///
/// # Errors
/// `reserve-exhausted` (the merged chunk's tail cannot take an appended
/// fragment, or the output section has no merged chunk); `new-output-section`
/// (a piece of an output section the state does not have);
/// `update-not-implemented (debug)` (a piece of a `.debug_*` section, R-1:
/// never counted); `update-not-implemented (layout)` (a dead string whose
/// stored `size` is 0: a format that does not store it; a piece the changes
/// do not explain); `state-corrupt` (a count below 0, a `FRAG_USE` row with
/// no `FRAGS` row, a piece whose size is not its fragment's).
pub(super) fn item_6(
    base: &Base,
    chunks: &mut Column<ChunkId, Chunk>,
    stored: &mortar_core::incr::StoredLayout<'_>,
    input: &LayoutInput<'_>,
    files: &[FileId],
    sections: &BTreeSet<SectionId>,
) -> Result<Merged, IncrReason> {
    let new = new_uses(base, input, files, sections)?;
    let (net, first) = net_change(stored, files, &new)?;
    let piece_of: BTreeMap<Key, Piece> = first.iter().copied().collect();
    let mut out = Merged::default();
    let mut appended: Vec<Key> = Vec::new();
    for (&key, &d) in &net {
        if d == 0 {
            continue;
        }
        let row = stored.frags_of(key.0, key.1, key.2)?;
        if let Some(r) = row
            && (r.out, r.class, r.hash) != key
        {
            return Err(corrupt("frags_of gave another key's row"));
        }
        let before = row.map_or(0, |r| i64::from(r.refs));
        let after = before + d;
        if after < 0 {
            return Err(corrupt(format!(
                "FRAGS.refs below 0 for a fragment of output section {}",
                key.0.0
            )));
        }
        let after = u32::try_from(after).map_err(|_| corrupt("FRAGS.refs past u32"))?;
        match row {
            None if after == 0 => {}
            None => appended.push(key),
            Some(r) => {
                let chunk = merged_chunk(base, key.0)?;
                let (strings, entsize, _) = class_parts(key.1);
                if r.refs > 0 && after == 0 {
                    // A dead range (IN-3a): a constant takes the fill; a
                    // string keeps its bytes, a `DeadFragment` hole of its
                    // stored size (seam 6, WP-242 C).
                    let hole = if strings {
                        if r.size == 0 {
                            return Err(dead_string_refused());
                        }
                        Hole {
                            chunk,
                            offset: r.offset,
                            size: u64::from(r.size),
                            kind: HoleKind::DeadFragment,
                        }
                    } else {
                        if entsize == 0 {
                            return Err(corrupt("a constant fragment's class has entsize 0"));
                        }
                        Hole { chunk, offset: r.offset, size: entsize, kind: HoleKind::Fill }
                    };
                    out.made.push(hole);
                    out.died.insert(key.0);
                } else if r.refs == 0 && after > 0 {
                    // Revived: its stored hole goes, U-6 rewrites it.
                    let p = piece_of
                        .get(&key)
                        .ok_or_else(|| corrupt("a fragment rises from 0 with no piece"))?;
                    if r.size != 0 && u64::from(r.size) != p.size {
                        return Err(corrupt("a piece's size is not its fragment's"));
                    }
                    out.gone.push(revived_hole(stored, chunk, r, strings, entsize, p.size)?);
                    out.rose.insert(key.0);
                }
                out.rows.push(StoredFrag { refs: after, ..r });
            }
        }
    }
    // Appended at the merged chunk's tail (IN-72 item 6), in the order the
    // new versions first name them; refused past the chunk's capacity.
    let mut order: Vec<Key> =
        first.iter().map(|(k, _)| *k).filter(|k| appended.contains(k)).collect();
    order.sort_by_key(|k| k.0);
    for key in order {
        let p = piece_of[&key];
        let chunk = merged_chunk(base, key.0)?;
        let name = base.names.get(&key.0).map_or_else(String::new, |n| text(n));
        let (strings, entsize, align) = class_parts(key.1);
        if !strings && p.size != entsize {
            return Err(corrupt(format!("{name}: a constant piece is not entsize bytes")));
        }
        let a = 1u64 << align.min(63);
        let ch = chunks[chunk];
        let offset = round_up(ch.size, a).ok_or_else(not_implemented)?;
        let end = offset.checked_add(p.size).ok_or_else(not_implemented)?;
        // Past the chunk's capacity, an allocated chunk takes item 3's
        // growth step (its `PT_LOAD`'s reserve; F-2: the tail is O's,
        // and stage D moves the last chunk's to the segment); a chunk
        // outside every `PT_LOAD` (`.comment`) has no reserve.
        if end > ch.capacity && !ch.is_alloc() {
            return Err(IncrReason::ReserveExhausted(format!("{name}: merged tail")));
        }
        admit(chunks, chunk, a, base.rules.max_page, name.as_bytes())?;
        chunks[chunk].size = end;
        let refs = u32::try_from(net[&key]).map_err(|_| corrupt("FRAGS.refs past u32"))?;
        let size = u32::try_from(p.size).map_err(|_| not_implemented())?;
        out.rows.push(StoredFrag { out: key.0, class: key.1, hash: key.2, offset, size, refs });
        out.rose.insert(key.0);
    }
    out.rows.sort_by_key(|r| (r.out, r.class, r.hash));
    Ok(out)
}

/// The net change of each key's count over the files of the update (old
/// uses -1, new distinct fragments +1), and the first piece of each key
/// the new versions name (in file, then piece, order).
#[allow(clippy::type_complexity, reason = "two results of one pass")]
fn net_change(
    stored: &mortar_core::incr::StoredLayout<'_>,
    files: &[FileId],
    new: &BTreeMap<FileId, Vec<(Key, Piece)>>,
) -> Result<(BTreeMap<Key, i64>, Vec<(Key, Piece)>), IncrReason> {
    let mut net: BTreeMap<Key, i64> = BTreeMap::new();
    let mut first: Vec<(Key, Piece)> = Vec::new();
    let mut seen: BTreeSet<Key> = BTreeSet::new();
    for &file in files {
        let old: BTreeSet<Key> = {
            let rows = stored.frag_uses_of(file)?;
            let n = rows.len();
            let set: BTreeSet<Key> = rows
                .into_iter()
                .map(|r| {
                    if r.file == file {
                        Ok((r.out, r.class, r.hash))
                    } else {
                        Err(corrupt(format!("FRAG_USE range of file {}", file.0)))
                    }
                })
                .collect::<Result<_, _>>()?;
            if set.len() != n {
                return Err(corrupt(format!("FRAG_USE of file {} names a fragment twice", file.0)));
            }
            set
        };
        let empty = Vec::new();
        let now = new.get(&file).unwrap_or(&empty);
        let now_keys: BTreeSet<Key> = now.iter().map(|(k, _)| *k).collect();
        for k in old.difference(&now_keys) {
            *net.entry(*k).or_default() -= 1;
        }
        for k in now_keys.difference(&old) {
            *net.entry(*k).or_default() += 1;
        }
        for &(k, p) in now {
            if seen.insert(k) {
                first.push((k, p));
            }
        }
    }
    Ok((net, first))
}

/// A string fragment whose count falls to 0 and whose stored `size` is 0:
/// no fragment is empty (a string holds its NUL), and the capture writes
/// every `FRAGS` row's size (`FRAGS.size`, seam 6), so such a row is not
/// one a capture made; its `DeadFragment` hole cannot be given, and the
/// update is refused rather than guess the size.
fn dead_string_refused() -> IncrReason {
    not_implemented()
}

/// Each file's new version's distinct fragments, in piece order, with the
/// first piece of each; checked against the changes.
fn new_uses(
    base: &Base,
    input: &LayoutInput<'_>,
    files: &[FileId],
    sections: &BTreeSet<SectionId>,
) -> Result<BTreeMap<FileId, Vec<(Key, Piece)>>, IncrReason> {
    let changes = input.changes;
    let find = |id: SectionId| -> Option<&SectionChange<'_>> {
        changes.binary_search_by_key(&id, |c| c.id).ok().map(|k| &changes[k])
    };
    let mut out: BTreeMap<FileId, Vec<(Key, Piece)>> = BTreeMap::new();
    let mut with_piece: BTreeSet<SectionId> = BTreeSet::new();
    for fp in input.pieces {
        if files.binary_search(&fp.file).is_err() {
            if fp.pieces.is_empty() {
                continue;
            }
            // Pieces of a file with no change: the join's contract is
            // broken; never counted.
            debug_assert!(false, "pieces of file {} not in changes", fp.file.0);
            return Err(not_implemented());
        }
        let list = out.entry(fp.file).or_default();
        let mut keys: BTreeSet<Key> = BTreeSet::new();
        for p in &fp.pieces {
            let c = find(p.section);
            if let Some(c) = c
                && (c.key.name.starts_with(b".debug_") || c.key.name.starts_with(b".zdebug"))
            {
                // R-1: no `.debug_*` fragment exists; the file's debug
                // contribution is the join's `(debug)`.
                return Err(IncrReason::UpdateNotImplemented(Some(Unimplemented::Debug)));
            }
            let ok = c.is_some_and(|c| c.file == fp.file) && sections.contains(&p.section);
            if !ok {
                // A piece of a section that is not a live, kept, merged
                // section of its file in this update (or one placed whole
                // before, S-6): not counted.
                return Err(not_implemented());
            }
            if !base.names.contains_key(&p.out) {
                return Err(IncrReason::NewOutputSection(format!("output section {}", p.out.0)));
            }
            with_piece.insert(p.section);
            let key = (p.out, p.class.word(), p.hash);
            if keys.insert(key) {
                list.push((key, Piece { size: p.size }));
            }
        }
    }
    if sections.iter().any(|s| !with_piece.contains(s)) {
        // A merged section without its pieces: the join did not give
        // them; never a guess.
        return Err(not_implemented());
    }
    Ok(out)
}

/// The merged chunk of `out` (one per output section, L-147); none: the
/// output section has no tail for fragments.
fn merged_chunk(base: &Base, out: OutSecId) -> Result<ChunkId, IncrReason> {
    base.of_out
        .get(&out)
        .and_then(|v| v.iter().copied().find(|&c| base.chunks[c].kind == ChunkKind::Merged(out)))
        .ok_or_else(|| {
            let name = base.names.get(&out).map_or_else(String::new, |n| text(n));
            IncrReason::ReserveExhausted(format!("{name}: no merged chunk"))
        })
}

/// The stored hole of a dead fragment that revives: a constant's fill of
/// `entsize` bytes, a string's `DeadFragment` of its piece's length, at
/// its offset. Any other shape is not this package's to guess.
fn revived_hole(
    stored: &mortar_core::incr::StoredLayout<'_>,
    chunk: ChunkId,
    r: StoredFrag,
    strings: bool,
    entsize: u64,
    size: u64,
) -> Result<Hole, IncrReason> {
    let want = if strings {
        Hole { chunk, offset: r.offset, size, kind: HoleKind::DeadFragment }
    } else {
        Hole { chunk, offset: r.offset, size: entsize, kind: HoleKind::Fill }
    };
    if stored.holes_in(chunk)?.contains(&want) { Ok(want) } else { Err(not_implemented()) }
}
