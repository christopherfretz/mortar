//! WP-225 M2b-3 (the record `spec/landed/incr/WP-225.md`, "M2b-3 design",
//! as ruled): the materializer. A format-5 stored layout and the delta
//! layout's `LayoutDelta` (both in the update's ids: the state's rows, and
//! `ChangedFile::first_section + k` for a changed file's sections) made
//! into a whole `Layout` in a FRONT's ids (`SectionId`, `ChunkId`,
//! `OutSecId`, `SymbolId` of a full link's front over the update's
//! inputs): what the image takes through the front's replay hook
//! (path (A), R-1), one function per decision table (D-3), every refusal
//! by name.
//!
//! Called by the update only (`mortar::update`), never by a plain link: a
//! command-line link's `Env::replay` stays `None` (D-7).

use mortar_core::hash::HashMap;

use mortar_core::column::Column;
use mortar_core::id::{ChunkId, FileId, FragId, Id, OutSecId, SectionId, SymKey, SymKind, SymbolId};
use mortar_core::identity::{
    ChunkIdentity, ChunkTag, FileIdentity, Lookup, OutSecIdentity, SymbolIdentity, chunk_id,
    chunk_ordinals, outsec_id, symbol_id, symbol_lookup,
};
use mortar_core::incr::{
    IncrReason, LayoutDelta, MergeClass, PlaceNow, SectionChange, StoredLayout, StoredSlot,
    Unimplemented,
};
use mortar_core::inputs::Reader;
use mortar_core::layout::{
    AllocKey, Chunk, ChunkKind, DefValue, Defined, EhRecord, Hole, HoleKind, Layout, MergeMap,
    Patch, PatchKind, Place, Placement, Redirect, Sized, SlotKind, Slots, SymPlace, ThunkTarget,
};
use mortar_elf::SectionIndex;

use crate::capture::{EhFile, Keys, Link, TAGS};
use crate::format::{ChunkRow, NONE, Str, chunk_bits, kind, layout_bits, sym_kind};
use crate::update::rows::file_flags;
use crate::update::view::StateView;

/// A refusal: the reason the update falls back by, and its name (which
/// table and row of D-3, or a FINDING with both answers: the text starts
/// with `FINDING`). `update-not-implemented (x)` carries no detail, so the
/// name travels beside it.
pub type Refusal = (IncrReason, String);

fn not(word: Unimplemented, what: String) -> Refusal {
    (IncrReason::UpdateNotImplemented(Some(word)), what)
}
fn layout(what: String) -> Refusal {
    not(Unimplemented::Layout, what)
}
fn image(what: String) -> Refusal {
    not(Unimplemented::Image, what)
}
fn eh(what: String) -> Refusal {
    not(Unimplemented::Eh, what)
}
fn symtab(what: String) -> Refusal {
    not(Unimplemented::Symtab, what)
}
fn corrupt(what: String) -> Refusal {
    (IncrReason::StateCorrupt(what.clone()), what)
}
fn read(e: IncrReason) -> Refusal {
    let what = e.report();
    (e, what)
}

/// The front's side of a materialization ([`materialize`]'s last five).
#[derive(Debug, Clone, Copy)]
struct FrontSide<'a, 'l, 's> {
    link: &'a Link<'l, 's>,
    keys: &'a Keys<'s>,
    computed: &'a Layout,
    classes: &'a [MergeClass],
    eh: &'a [EhFile],
}

// ---- D-2: the id maps -----------------------------------------------------------

/// The chunk of this link each `CHUNKS` row names: a COPY of the chunk
/// block of `capture.rs`'s `map` (ruling R-4: one producer later, WP-264's
/// or the supervisor's merge), over rows read from any store of them.
/// `name(row)` is the row's name bytes; `outsecs` the present output
/// section of each `OUTSECS` row.
///
/// # Errors
/// `CHUNKS row i: no such chunk in this link` / `.. not this link's chunk
/// of that identity`, as `map`.
pub fn chunk_map(
    link: &Link<'_, '_>,
    rows: &[ChunkRow],
    name: &dyn Fn(Str) -> Result<Vec<u8>, String>,
    outsecs: &[Option<OutSecId>],
    placement: &Placement,
) -> Result<Vec<ChunkId>, String> {
    let ord = chunk_ordinals(placement, link.assignment);
    let mut chunks = Vec::with_capacity(rows.len());
    for (i, c) in rows.iter().enumerate() {
        let tag = *TAGS.get(c.tag as usize).ok_or("a chunk tag")?;
        let bytes = if tag == ChunkTag::Synthetic || c.out != NONE { Some(name(c.name)?) } else { None };
        let identity = ChunkIdentity { kind: tag, name: bytes.as_deref(), ordinal: c.ordinal };
        let id = chunk_id(&ord, &identity)
            .ok_or_else(|| format!("CHUNKS row {i}: no such chunk in this link"))?;
        let present = placement.chunks.get(id).ok_or("a chunk")?;
        let (out, part, dep) = match present.kind {
            ChunkKind::Body(pt) | ChunkKind::Patches(pt) => (Some(pt.out), pt.part, false),
            ChunkKind::Merged(o) | ChunkKind::Relocs(o) => (Some(o), 0, false),
            ChunkKind::Synthetic(sy) => (None, 0, sy.address_dependent),
            _ => (None, 0, false),
        };
        let want_out =
            if c.out == NONE { None } else { outsecs.get(c.out as usize).copied().flatten() };
        if out != want_out || part != c.part || dep != (c.bits & chunk_bits::ADDRESS_DEPENDENT != 0)
        {
            return Err(format!("CHUNKS row {i}: not this link's chunk of that identity"));
        }
        chunks.push(id);
    }
    Ok(chunks)
}

/// `capture.rs`'s own `map` over a format-4 state, its chunk block's
/// answer only: the reference [`chunk_map`] is tested equal to (ruling
/// R-4). Tests only.
///
/// # Errors
/// `map`'s.
#[doc(hidden)]
pub fn capture_chunk_map(
    link: &Link<'_, '_>,
    keys: &Keys<'_>,
    s: &crate::format::State,
    placement: &Placement,
) -> Result<Vec<ChunkId>, String> {
    let mut laps = crate::capture::Laps::new();
    let mut clock = crate::capture::Clock { laps: &mut laps, at: std::time::Instant::now() };
    crate::capture::map(link, keys, s, placement, &mut clock, None).map(|(m, _)| m.chunks)
}

/// The ids of D-2, built once (the prologue).
struct Ids<'v> {
    /// Per `CHUNKS` row: the front's chunk.
    chunks: Vec<ChunkId>,
    /// Per `OUTSECS` row: the front's output section.
    outsecs: Vec<Option<OutSecId>>,
    /// Per front output section: its `OUTSECS` row, or `NONE`.
    out_rows: Vec<u32>,
    /// Per front section: its `SECTIONS` row (unchanged files) or its
    /// change's id (the files of `d.files`), or `NONE`.
    of_front: Vec<u32>,
    /// Per update id named by a change: the front's section.
    changed: HashMap<u32, SectionId>,
    /// `SECTIONS` count.
    sections: u32,
    /// `SYMBOLS` count.
    symbols: u64,
    files: &'v [FileId],
}

impl Ids<'_> {
    fn chunk(&self, row: ChunkId, what: &str) -> Result<ChunkId, Refusal> {
        self.chunks
            .get(row.0 as usize)
            .copied()
            .ok_or_else(|| corrupt(format!("{what}: CHUNKS row {} past the table", row.0)))
    }
    fn is_changed(&self, file: FileId) -> bool {
        self.files.binary_search(&file).is_ok()
    }
}

/// The global of a `SYMBOLS` row, by name and version (D-2 "globals").
struct Globals<'k> {
    whole: Option<Lookup<SymbolId>>,
    keys: &'k Keys<'k>,
    cache: HashMap<u32, SymbolId>,
}

impl Globals<'_> {
    fn get(
        &mut self,
        view: &StateView<'_>,
        link: &Link<'_, '_>,
        ids: &Ids<'_>,
        row: SymbolId,
        what: &str,
    ) -> Result<SymbolId, Refusal> {
        if let Some(&s) = self.cache.get(&row.0) {
            return Ok(s);
        }
        if u64::from(row.0) >= ids.symbols {
            return Err(layout(format!("{what}: key {} is past SYMBOLS (an update-only key)", row.0)));
        }
        let y = view.symbol(row).map_err(read)?;
        if y.kind == sym_kind::LOCAL {
            return Err(corrupt(format!("{what}: SYMBOLS row {}: a local", row.0)));
        }
        let name = view.string(y.name).map_err(read)?;
        let version =
            if y.kind == sym_kind::VERSIONED { Some(view.string(y.version).map_err(read)?) } else { None };
        let g = SymbolIdentity { name: &name, version: version.as_deref() };
        let lookup = if self.keys.complete {
            &self.keys.symbols
        } else {
            self.whole.get_or_insert_with(|| symbol_lookup(&link.inputs.symbols))
        };
        let id = symbol_id(&link.inputs.symbols, lookup, &g).ok_or_else(|| {
            image(format!(
                "{what}: SYMBOLS row {} ({}): no such key in this link",
                row.0,
                String::from_utf8_lossy(&name)
            ))
        })?;
        self.cache.insert(row.0, id);
        Ok(id)
    }

    /// A stored key (`SymKey::global(row)` or a local by `(FILES row,
    /// index)`, as `LayoutOf` gives it) in the front's ids.
    fn key(
        &mut self,
        view: &StateView<'_>,
        link: &Link<'_, '_>,
        ids: &Ids<'_>,
        key: SymKey,
        what: &str,
    ) -> Result<SymKey, Refusal> {
        match key.kind() {
            SymKind::Global(row) => Ok(SymKey::global(self.get(view, link, ids, row, what)?)),
            SymKind::Local(e) => {
                if ids.is_changed(e.file) {
                    return Err(layout(format!(
                        "{what}: a stored local of file {} (a file of d.files: its old version's)",
                        e.file.0
                    )));
                }
                if link.inputs.files.get(e.file).is_none() {
                    return Err(image(format!("{what}: a local of file {}: no such file", e.file.0)));
                }
                Ok(key)
            }
        }
    }
}

fn file_name(view: &StateView<'_>, row: &crate::update::rows::FileRow) -> Result<(Vec<u8>, Vec<u8>), Refusal> {
    Ok((view.string(row.path).map_err(read)?, view.string(row.member_name).map_err(read)?))
}

/// D-2's prologue: files, sections, outsecs, chunks.
#[allow(clippy::too_many_lines, reason = "D-2's maps, one block each")]
fn ids<'v>(
    view: &StateView<'_>,
    stored: &StoredLayout<'_>,
    d: &'v LayoutDelta,
    changes: &[SectionChange<'_>],
    front: &FrontSide<'_, '_, '_>,
) -> Result<Ids<'v>, Refusal> {
    let link = front.link;
    let res = link.resolution;
    // ---- files: by identity, and each row its own number.
    let n_files = view.count(kind::FILES);
    if n_files != u64::from(link.inputs.files.len()) {
        return Err(image(format!(
            "files: FILES has {n_files} rows, the link {} files",
            link.inputs.files.len()
        )));
    }
    for i in 0..link.inputs.files.len() {
        let row = view.file(FileId(i)).map_err(read)?;
        if row.flags & file_flags::GONE != 0 {
            return Err(image(format!("files: FILES row {i} is GONE")));
        }
        let (path, member_name) = file_name(view, &row)?;
        let identity = FileIdentity {
            path: &path,
            mention: row.mention,
            member: (row.member == 1).then_some((&member_name[..], row.member_ordinal)),
        };
        let got = front.keys.identities.file_id(&identity);
        if got != Some(FileId(i)) {
            return Err(image(format!("files: FILES row {i} is the link's file {got:?}")));
        }
    }
    // ---- outsecs.
    let rows = view.outsecs().map_err(read)?;
    let mut outsecs = Vec::with_capacity(rows.len());
    for o in &rows {
        let name = view.string(o.name).map_err(read)?;
        outsecs.push(outsec_id(
            &front.keys.outsecs,
            &OutSecIdentity { name: &name, ordinal: o.ordinal },
        ));
    }
    let mut out_rows = vec![NONE; link.assignment.outsecs.len() as usize];
    for (r, o) in outsecs.iter().enumerate() {
        if let Some(o) = o
            && let Some(x) = out_rows.get_mut(o.0 as usize)
        {
            *x = r as u32;
        }
    }
    // ---- chunks.
    let rows = view.chunks().map_err(read)?;
    let name = |s: Str| view.string(s).map_err(|e| e.report());
    let chunks =
        chunk_map(link, &rows, &name, &outsecs, &front.computed.placement).map_err(image)?;
    // ---- sections: by position in an unchanged file, from the changes in
    // a file of `d.files`.
    let sections = u32::try_from(view.count(kind::SECTIONS))
        .map_err(|_| corrupt("SECTIONS over 2^32 rows".into()))?;
    let mut of_front = vec![NONE; res.sections.len() as usize];
    for i in 0..link.inputs.files.len() {
        let file = FileId(i);
        let count = res.section_base.get(file).map_or(0, |r| r.len());
        if d.files.binary_search(&file).is_ok() {
            continue;
        }
        let rows = stored.sections_of(file).map_err(read)?;
        if rows.len() as u64 != u64::from(count) {
            return Err(corrupt(format!(
                "sections: file {i} has {} SECTIONS rows and {count} sections",
                rows.len(),
            )));
        }
        let object = match link.inputs.files[file].reader {
            Reader::Object(o) => Some(o),
            _ => None,
        };
        for (k, r) in rows.iter().enumerate() {
            let k = k as u32;
            let (sh_type, _, size, _, _) =
                crate::capture::header_fields(link.inputs, file, SectionIndex(k));
            let name: &[u8] = match object {
                Some(o) if k > 0 => o
                    .section(SectionIndex(k))
                    .ok()
                    .and_then(|s| s.name().ok())
                    .unwrap_or_default(),
                _ => &[],
            };
            if r.index != k || r.name != name || r.sh_type != sh_type || r.size != size {
                return Err(corrupt(format!(
                    "sections: SECTIONS row {} is not section {k} of file {i} ({} type {:#x} size {:#x}; the file's {} type {sh_type:#x} size {size:#x})",
                    r.id.0,
                    String::from_utf8_lossy(&r.name),
                    r.sh_type,
                    r.size,
                    String::from_utf8_lossy(name)
                )));
            }
            let fid = res.section_id(file, SectionIndex(k)).ok_or_else(|| {
                corrupt(format!("sections: file {i} has no section {k}"))
            })?;
            of_front[fid.slot()] = r.id.0;
        }
    }
    let mut changed: HashMap<u32, SectionId> = HashMap::default();
    for c in changes {
        if d.files.binary_search(&c.file).is_err() {
            return Err(layout(format!(
                "sections: a change of section {} names file {}, not of d.files",
                c.id.0, c.file.0
            )));
        }
        let fid = res.section_id(c.file, SectionIndex(c.index)).ok_or_else(|| {
            layout(format!("sections: change {} (file {} index {}): no such section", c.id.0, c.file.0, c.index))
        })?;
        if of_front[fid.slot()] != NONE || changed.insert(c.id.0, fid).is_some() {
            return Err(layout(format!(
                "sections: section {} of file {} named twice by the changes",
                c.index, c.file.0
            )));
        }
        of_front[fid.slot()] = c.id.0;
    }
    for &file in &d.files {
        let count = res.section_base.get(file).map_or(0, |r| r.len());
        for k in 0..count {
            let fid = res.section_id(file, SectionIndex(k)).unwrap_or(SectionId(NONE));
            if of_front.get(fid.0 as usize).is_none_or(|&x| x == NONE) {
                return Err(layout(format!(
                    "sections: section {k} of file {} (of d.files) has no change",
                    file.0
                )));
            }
        }
    }
    Ok(Ids {
        chunks,
        outsecs,
        out_rows,
        of_front,
        changed,
        sections,
        symbols: view.count(kind::SYMBOLS),
        files: &d.files,
    })
}

// ---- D-3: the tables -------------------------------------------------------------

/// `placement.chunks`: `d`'s chunks (whole, by `CHUNKS` row) in the front's
/// ids, each with the front's kind; every front chunk named once.
fn chunks(d: &LayoutDelta, ids: &Ids<'_>, computed: &Layout) -> Result<Vec<Chunk>, Refusal> {
    let cp = &computed.placement;
    if d.placement.chunks.len() as usize != ids.chunks.len() {
        return Err(layout(format!(
            "chunks: d.placement.chunks has {} rows, CHUNKS {}",
            d.placement.chunks.len(),
            ids.chunks.len()
        )));
    }
    let mut out: Vec<Option<Chunk>> = vec![None; cp.chunks.len() as usize];
    for (row, (&id, c)) in ids.chunks.iter().zip(d.placement.chunks.as_slice()).enumerate() {
        let slot = &mut out[id.slot()];
        if slot.is_some() {
            return Err(image(format!("chunks: CHUNKS row {row}: a chunk named twice")));
        }
        *slot = Some(Chunk { kind: cp.chunks[id].kind, ..*c });
    }
    out.into_iter()
        .enumerate()
        .map(|(i, c)| c.ok_or_else(|| image(format!("chunks: chunk {i} of this link has no row"))))
        .collect()
}

/// `placement.place`: an unchanged file's sections at their stored places,
/// a changed file's at `d.place_of` (D-3 refusals (i), (ii)).
fn place(
    stored: &StoredLayout<'_>,
    d: &LayoutDelta,
    ids: &Ids<'_>,
    computed: &Layout,
    chunks: &[Chunk],
) -> Result<Vec<Place>, Refusal> {
    // (i): `d` moves, revives or kills a section of `d.files` only.
    for (list, name) in [(&d.moved, "moved"), (&d.revived, "revived"), (&d.killed, "killed")] {
        for &id in list {
            let ok = match stored.section(id).map_err(read)? {
                Some(row) => ids.is_changed(row.file),
                None => ids.changed.contains_key(&id.0),
            };
            if !ok {
                return Err(layout(format!(
                    "place (i): d.{name} names section {} of no file of d.files",
                    id.0
                )));
            }
        }
    }
    let cp = &computed.placement;
    let mut out = vec![Place::NONE; cp.place.len() as usize];
    for (i, &row) in ids.of_front.iter().enumerate() {
        if row == NONE {
            continue;
        }
        let fid = SectionId(i as u32);
        let at = if row >= ids.sections || ids.changed.get(&row) == Some(&fid) {
            // A section of `d.files` (its change's id).
            match d.place_of(stored, SectionId(row)).map_err(read)? {
                PlaceNow::Here(p) => p.get(),
                PlaceNow::NotPlaced => None,
                PlaceNow::Stored(_) => {
                    return Err(layout(format!("place: change {row} has a stored place")));
                }
            }
        } else {
            stored.section(SectionId(row)).map_err(read)?.and_then(|s| s.place.get())
        };
        let Some((c, off)) = at else { continue };
        let c = ids.chunk(c, "place")?;
        out[i] = Place::at(c, off);
    }
    // (ii): a changed file's section placed where the front says, in the
    // chunk of the front's output section and part.
    for (&uid, &fid) in &ids.changed {
        let mine = out[fid.slot()].get();
        let full = cp.place.get(fid).and_then(|p| p.get());
        let same = match (mine, full) {
            (None, None) => true,
            (Some((c, _)), Some((fc, _))) => {
                matches!(chunks[c.slot()].kind, ChunkKind::Body(_))
                    && chunks[c.slot()].kind == cp.chunks[fc].kind
            }
            _ => false,
        };
        if !same {
            let kind = |p: Option<(ChunkId, u64)>, t: &[Chunk]| p.map(|(c, o)| (t[c.slot()].kind, o));
            return Err(layout(format!(
                "FINDING place (ii): section {} (update id {uid}): the delta {:?}, the front {:?}",
                fid.0,
                kind(mine, chunks),
                kind(full, cp.chunks.as_slice())
            )));
        }
    }
    Ok(out)
}

/// `placement.defined`: `d`'s rows (the stored `DEFINED`, END rows of a
/// resized chunk followed), each judged against the front's value.
#[allow(clippy::too_many_arguments, reason = "the view, the maps and both answers")]
fn defined(
    view: &StateView<'_>,
    link: &Link<'_, '_>,
    stored_chunks: &[(ChunkId, Chunk)],
    d: &LayoutDelta,
    ids: &Ids<'_>,
    g: &mut Globals<'_>,
    computed: &Layout,
    chunks: &[Chunk],
) -> Result<Vec<Defined>, Refusal> {
    let cp = &computed.placement;
    let mut out = Vec::with_capacity(d.placement.defined.len());
    for x in &d.placement.defined {
        let symbol = g.get(view, link, ids, x.symbol, "defined")?;
        let value = match x.value {
            DefValue::Absolute(v) => DefValue::Absolute(v),
            DefValue::At { chunk, offset } => {
                let c = ids.chunk(chunk, "defined")?;
                let resized = stored_chunks
                    .get(chunk.0 as usize)
                    .is_none_or(|s| s.1.size != d.placement.chunks[chunk].size);
                let front = cp.defined.iter().find(|f| f.symbol == symbol).map(|f| f.value);
                let ok = match front {
                    Some(DefValue::At { chunk: fc, offset: fo }) if fc == c => {
                        if offset == 0 {
                            fo == 0
                        } else if offset == chunks[c.slot()].size {
                            fo == cp.chunks[fc].size
                        } else {
                            fo == offset && !resized
                        }
                    }
                    _ => false,
                };
                if !ok {
                    return Err(layout(format!(
                        "defined: key {} at chunk {}+{offset:#x}: the front's {front:?} (IN-71c)",
                        symbol.0, c.0
                    )));
                }
                DefValue::At { chunk: c, offset }
            }
        };
        if let DefValue::Absolute(v) = value {
            let front = cp.defined.iter().find(|f| f.symbol == symbol).map(|f| f.value);
            if front != Some(DefValue::Absolute(v)) {
                return Err(layout(format!(
                    "defined: key {} absolute {v:#x}: the front's {front:?}",
                    symbol.0
                )));
            }
        }
        out.push(Defined { symbol, value });
    }
    out.sort_by_key(|x| x.symbol);
    if out.len() != cp.defined.len() {
        return Err(layout(format!(
            "defined: {} rows, the front {} (a linker-defined key the delta does not have)",
            out.len(),
            cp.defined.len()
        )));
    }
    Ok(out)
}

/// `placement.patches` and `.redirects` (stored; refused around `d.files`).
fn patches(
    view: &StateView<'_>,
    link: &Link<'_, '_>,
    stored: &StoredLayout<'_>,
    ids: &Ids<'_>,
    g: &mut Globals<'_>,
    computed: &Layout,
) -> Result<(Vec<Patch>, Vec<Redirect>), Refusal> {
    let res = link.resolution;
    let of_changed = |s: SectionId| res.sh_index(s).is_some_and(|(f, _)| ids.is_changed(f));
    let cp = &computed.placement;
    for p in &cp.patches {
        if let PatchKind::Erratum { section, .. } = p.kind
            && of_changed(section)
        {
            return Err(layout(format!(
                "patches (ii): the front has an erratum patch in section {} of d.files (item 9)",
                section.0
            )));
        }
    }
    if let Some(r) = cp.redirects.iter().find(|r| of_changed(r.section)) {
        return Err(layout(format!(
            "patches (ii): the front has a redirect in section {} of d.files (item 9: a changed file's thunks)",
            r.section.0
        )));
    }
    let section = |row: SectionId, what: &str| -> Result<SectionId, Refusal> {
        let r = stored.section(row).map_err(read)?.ok_or_else(|| {
            corrupt(format!("{what}: SECTIONS row {} is no file's", row.0))
        })?;
        if ids.is_changed(r.file) {
            return Err(layout(format!(
                "{what}: SECTIONS row {} is of file {} (d.files: an old row)",
                row.0, r.file.0
            )));
        }
        res.section_id(r.file, SectionIndex(r.index))
            .ok_or_else(|| corrupt(format!("{what}: SECTIONS row {}: no such section", row.0)))
    };
    let mut out = Vec::new();
    for (i, p) in stored.patches().map_err(read)?.into_iter().enumerate() {
        let kind = match p.kind {
            PatchKind::Thunk(ThunkTarget::Sym { key, addend }) => {
                PatchKind::Thunk(ThunkTarget::Sym { key: g.key(view, link, ids, key, "patches")?, addend })
            }
            PatchKind::Thunk(ThunkTarget::Plt(key)) => {
                PatchKind::Thunk(ThunkTarget::Plt(g.key(view, link, ids, key, "patches")?))
            }
            PatchKind::Erratum { erratum, section: s, offset } => {
                PatchKind::Erratum { erratum, section: section(s, "patches (i)")?, offset }
            }
        };
        out.push((Patch { chunk: ids.chunk(p.chunk, "patches")?, offset: p.offset, kind }, i));
    }
    out.sort_by_key(|(p, _)| (p.chunk, p.offset));
    let mut moved = vec![0u32; out.len()];
    for (new, (_, old)) in out.iter().enumerate() {
        moved[*old] = new as u32;
    }
    let mut redirects = Vec::new();
    for r in stored.redirects().map_err(read)? {
        let patch = *moved
            .get(r.patch as usize)
            .ok_or_else(|| corrupt(format!("redirects: patch {} past PATCHES", r.patch)))?;
        redirects.push(Redirect { section: section(r.section, "redirects")?, offset: r.offset, patch });
    }
    redirects.sort_by_key(|r| (r.section, r.offset));
    Ok((out.into_iter().map(|(p, _)| p).collect(), redirects))
}

/// `merge.frag_off`: each live non-debug fragment of the front at its
/// identity's offset (`d.frags` first, then `FRAGS`); a `.debug_*`
/// fragment at the front's (WP-239 R-1; `debug_condition` keeps changed
/// files without debug sections).
fn frag_off(
    stored: &StoredLayout<'_>,
    d: &LayoutDelta,
    ids: &Ids<'_>,
    front: &FrontSide<'_, '_, '_>,
) -> Result<Column<FragId, u64>, Refusal> {
    let link = front.link;
    let split = link.split;
    let mut class = vec![NONE; split.frags.len() as usize];
    for (row, m) in split.merge_of.iter().enumerate() {
        let c = front
            .classes
            .get(row)
            .ok_or_else(|| image(format!("frag_off: merge_of row {row} has no class")))?
            .word();
        for p in m.pieces.slots() {
            if let Some(&frag) = split.piece_frag.get(p)
                && let Some(x) = class.get_mut(frag.0 as usize)
            {
                *x = c;
            }
        }
    }
    let mine: HashMap<(u32, u32, [u8; 16]), (u64, u32)> =
        d.frags.iter().map(|f| ((f.out.0, f.class, f.hash), (f.offset, f.refs))).collect();
    let mut taken: mortar_core::hash::HashSet<(u32, u32, [u8; 16])> =
        mortar_core::hash::HashSet::default();
    let mut frag_off = front.computed.merge.frag_off.clone();
    let mut buf = Vec::new();
    for (i, off) in frag_off.as_mut_slice().iter_mut().enumerate() {
        if *off == MergeMap::NONE {
            continue;
        }
        let frag = FragId::from_index(i as u32);
        let f = split.frags.get(frag).ok_or_else(|| image("frag_off: a fragment".into()))?;
        let out = link
            .assignment
            .out(f.section)
            .ok_or_else(|| image(format!("frag_off: fragment {i} has no output section")))?;
        if link.assignment.outsecs[out].name.starts_with(b".debug_") {
            continue;
        }
        let row = ids.out_rows.get(out.0 as usize).copied().unwrap_or(NONE);
        if row == NONE {
            return Err(layout(format!("frag_off: fragment {i}'s output section has no OUTSECS row")));
        }
        let c = class[i];
        let hash = crate::capture::frag_hash(link, frag, &mut buf)
            .ok_or_else(|| image(format!("frag_off: fragment {i}'s bytes")))?;
        let key = (row, c, hash);
        let (at, refs) = match mine.get(&key) {
            Some(&x) => x,
            None => match stored.frags_of(OutSecId(row), c, hash).map_err(read)? {
                Some(s) => (s.offset, s.refs),
                None => {
                    return Err(layout(format!(
                        "frag_off: live fragment {i} (out row {row}, class {c:#x}) has no row"
                    )));
                }
            },
        };
        if refs == 0 {
            return Err(layout(format!(
                "FINDING frag_off: fragment {i} is live in the front, its row has refs 0 (offset {at:#x})"
            )));
        }
        taken.insert(key);
        *off = at;
    }
    if let Some(f) = d.frags.iter().find(|f| f.refs > 0 && !taken.contains(&(f.out.0, f.class, f.hash))) {
        return Err(layout(format!(
            "FINDING frag_off: d.frags gives (out row {}, class {:#x}) refs {}, the front has it dead",
            f.out.0, f.class, f.refs
        )));
    }
    Ok(frag_off)
}

fn list_mut(slots: &mut Slots, kind: SlotKind) -> &mut Box<[SymKey]> {
    match kind {
        SlotKind::Got => &mut slots.got,
        SlotKind::Plt => &mut slots.plt,
        SlotKind::Iplt => &mut slots.iplt,
        SlotKind::TlsDesc => &mut slots.tls_desc,
        SlotKind::TlsGd => &mut slots.tls_gd,
        SlotKind::TlsIe => &mut slots.tls_ie,
    }
}

/// The slot lists (`SLOTS` with `d`'s rows), `tls_ld`, the dynamic tables
/// (the front's, each empty).
fn slot_lists(
    view: &StateView<'_>,
    link: &Link<'_, '_>,
    stored: &StoredLayout<'_>,
    d: &LayoutDelta,
    ids: &Ids<'_>,
    g: &mut Globals<'_>,
    computed: &Layout,
) -> Result<Slots, Refusal> {
    let cs = &computed.slots;
    let dynamic = !cs.dynsym.is_empty()
        || cs.dynsym_unhashed != 0
        || cs.gnu_hash_buckets != 0
        || !cs.dyn_relocs.is_empty()
        || !cs.needed.is_empty()
        || !cs.verneed.is_empty()
        || !cs.copies.is_empty()
        || !cs.copy_of.is_empty()
        || cs.dynstr != mortar_core::layout::DynStr::default();
    if dynamic {
        return Err(image("slots: a dynamic table (a dynamic output is not an update in v1)".into()));
    }
    if let Some(s) = d.slots_appended.first() {
        return Err(layout(format!("slots: an appended slot ({:?} {})", s.kind, s.index)));
    }
    let mut rows: Vec<StoredSlot> = stored.slots().map_err(read)?;
    for s in d.slots_dead.iter().chain(&d.slots_revived).chain(&d.slots_recounted) {
        let r = rows
            .iter_mut()
            .find(|r| r.kind == s.kind && r.index == s.index)
            .ok_or_else(|| layout(format!("slots: d's row {:?} {} is no SLOTS row", s.kind, s.index)))?;
        *r = *s;
    }
    let mut slots = cs.clone();
    for kind in crate::capture::SLOT_LISTS {
        let mut list: Vec<(u32, SymKey)> = Vec::new();
        for r in rows.iter().filter(|r| r.kind == kind) {
            if r.refs == 0 {
                return Err(layout(format!(
                    "slots: {kind:?} {} is retired (refs 0; CheckContext.retired is empty, G-9)",
                    r.index
                )));
            }
            list.push((r.index, g.key(view, link, ids, r.key, "slots")?));
        }
        list.sort_by_key(|x| x.0);
        *list_mut(&mut slots, kind) = list.into_iter().map(|x| x.1).collect();
    }
    let l = view.layout().map_err(read)?;
    let tls_ld = l.bits & layout_bits::TLS_LD != 0;
    if tls_ld != cs.tls_ld {
        return Err(layout(format!("slots: tls_ld stored {tls_ld}, the front {}", cs.tls_ld)));
    }
    slots.tls_ld = tls_ld;
    Ok(slots)
}

/// `slots.eh_records`: the front's records grouped by file, the groups in
/// the order of each file's range after the update.
fn eh_records(
    view: &StateView<'_>,
    stored: &StoredLayout<'_>,
    d: &LayoutDelta,
    front: &FrontSide<'_, '_, '_>,
) -> Result<Vec<EhRecord>, Refusal> {
    let res = front.link.resolution;
    let mut groups: Vec<(FileId, Vec<EhRecord>)> = Vec::new();
    let mut at: HashMap<FileId, usize> = HashMap::default();
    for r in &front.computed.slots.eh_records {
        let (file, _) = res
            .sh_index(r.section)
            .ok_or_else(|| image(format!("eh_records: section {} has no file", r.section.0)))?;
        let k = *at.entry(file).or_insert_with(|| {
            groups.push((file, Vec::new()));
            groups.len() - 1
        });
        groups[k].1.push(*r);
    }
    let ranges = d.eh.as_ref().map(|e| e.ranges.as_slice()).unwrap_or_default();
    let fdes: HashMap<FileId, u32> = front.eh.iter().map(|e| (e.file, e.fdes)).collect();
    let mut keyed = Vec::with_capacity(groups.len());
    for (file, recs) in groups {
        let n = recs.len();
        let offset = if let Some(r) = ranges.iter().find(|r| r.file == file) {
            if r.records.len() != n {
                return Err(eh(format!(
                    "eh_records: file {}: {n} records, d.eh's range {}",
                    file.0,
                    r.records.len()
                )));
            }
            r.offset
        } else {
            if d.eh.as_ref().is_some_and(|e| e.whole) {
                return Err(eh(format!("eh_records: file {} has records and no range in a whole d.eh", file.0)));
            }
            // Not rewritten: the stored range stands (a changed file's too,
            // whose mismatch is the delta's, `(eh)`, not the state's).
            let changed = d.files.binary_search(&file).is_ok();
            let row = view.file(file).map_err(read)?;
            if row.eh_records as usize != n || fdes.get(&file).copied() != Some(row.eh_fdes) {
                let on = if changed { eh } else { corrupt };
                return Err(on(format!(
                    "eh_records: file {}: stored {} records {} FDEs, the front {n} records {:?} FDEs",
                    file.0,
                    row.eh_records,
                    row.eh_fdes,
                    fdes.get(&file)
                )));
            }
            u64::from(row.eh_offset)
        };
        keyed.push((offset, file, recs));
    }
    // An unchanged file with stored records the front has none of.
    for e in stored.eh_ranges().map_err(read)? {
        if d.files.binary_search(&e.0).is_err() && !at.contains_key(&e.0) {
            return Err(corrupt(format!(
                "eh_records: file {} has {} stored records, the front none",
                e.0.0, e.2.records
            )));
        }
    }
    keyed.sort_by_key(|k| (k.0, k.1));
    Ok(keyed.into_iter().flat_map(|k| k.2).collect())
}

/// `slots.symtab_locals`: the stored `(first, count)` of each file with a
/// `SYMTAB` row, checked equal to the front's; the internal file's the
/// front's (no stored block relied on: F-1).
fn symtab_locals(
    stored: &StoredLayout<'_>,
    computed: &Layout,
) -> Result<Column<FileId, mortar_core::id::Span>, Refusal> {
    let spans = &computed.slots.symtab_locals;
    for (file, sp) in spans.iter() {
        if file == FileId::INTERNAL {
            continue;
        }
        match stored.symtab_block(file).map_err(read)? {
            Some(b) if (b.first, b.capacity) == (sp.start, sp.len) => {}
            None if sp.len == 0 => {}
            b => {
                return Err(symtab(format!(
                    "symtab_locals: file {}: stored {:?}, the front ({}, {})",
                    file.0,
                    b.map(|b| (b.first, b.capacity)),
                    sp.start,
                    sp.len
                )));
            }
        }
    }
    Ok(spans.clone())
}

/// `slots.sizes`: the front's list; each synthetic chunk's translated size
/// must be it.
fn sizes(computed: &Layout, chunks: &[Chunk]) -> Result<Vec<(ChunkId, u64)>, Refusal> {
    for &(c, size) in &computed.slots.sizes {
        let x = &chunks[c.slot()];
        if x.size == size {
            continue;
        }
        let name = match x.kind {
            ChunkKind::Synthetic(s) => String::from_utf8_lossy(s.name).into_owned(),
            k => format!("{k:?}"),
        };
        let what = format!("sizes: chunk {} ({name}): translated {:#x}, the front's {size:#x}", c.0, x.size);
        return Err(if size > x.capacity {
            (IncrReason::ReserveExhausted(name), what)
        } else if name == ".symtab" || name == ".strtab" {
            symtab(what)
        } else {
            layout(what)
        });
    }
    Ok(computed.slots.sizes.to_vec())
}

/// **The materializer** (D-2, D-3): the stored layout `stored` of `view`
/// with the delta `d` and its section changes `changes` (both in the
/// update's ids), as a whole `Layout` in `front`'s ids. Every refusal
/// is by name ([`Refusal`]); a FINDING (the delta disagrees with the
/// front on a rule the full link keeps) is a refusal whose name starts
/// with `FINDING`.
///
/// # Errors
/// [`Refusal`]: `state-corrupt`, `reserve-exhausted`, or
/// `update-not-implemented (layout|image|eh|symtab)`.
///
/// The front's side: `link`, `keys` (its identities), `computed` (its own
/// layout: front 1's, D-1), and what its capture reads beyond `Link`: per
/// `Split::merge_of` row its pieces' merge class (`Link5::classes`), and
/// its `.eh_frame` runs (`Link5::eh`).
#[allow(clippy::too_many_arguments, reason = "the stored side, the delta, the front's side")]
pub fn materialize(
    view: &StateView<'_>,
    stored: &StoredLayout<'_>,
    d: &LayoutDelta,
    changes: &[SectionChange<'_>],
    link: &Link<'_, '_>,
    keys: &Keys<'_>,
    computed: &Layout,
    classes: &[MergeClass],
    eh: &[EhFile],
) -> Result<(Layout, LayoutDelta), Refusal> {
    let front = &FrontSide { link, keys, computed, classes, eh };
    materialize_in(view, stored, d, changes, front)
}

#[allow(clippy::too_many_lines, reason = "D-3's tables in order")]
fn materialize_in(
    view: &StateView<'_>,
    stored: &StoredLayout<'_>,
    d: &LayoutDelta,
    changes: &[SectionChange<'_>],
    front: &FrontSide<'_, '_, '_>,
) -> Result<(Layout, LayoutDelta), Refusal> {
    let link = front.link;
    let computed = front.computed;
    if computed.folds.folded.is_some() {
        return Err(image("folds: the front folds sections (ICF is refused in incremental mode)".into()));
    }
    let ids = ids(view, stored, d, changes, front)?;
    let mut g = Globals { whole: None, keys: front.keys, cache: HashMap::default() };
    let chunk_rows = stored.chunks().map_err(read)?;
    let chunks = chunks(d, &ids, computed)?;
    let order = stored
        .order()
        .map_err(read)?
        .into_iter()
        .map(|c| ids.chunk(c, "order"))
        .collect::<Result<Box<[_]>, _>>()?;
    let place = place(stored, d, &ids, computed, &chunks)?;
    let mut sym_place = Vec::new();
    for r in stored.sym_place().map_err(read)? {
        let AllocKey::Common(row) = r.key else {
            return Err(corrupt("sym_place: a copy allocation in a static state".into()));
        };
        sym_place.push(SymPlace {
            key: AllocKey::Common(g.get(view, link, &ids, row, "sym_place")?),
            chunk: ids.chunk(r.chunk, "sym_place")?,
            ..r
        });
    }
    sym_place.sort_by_key(|r| r.key);
    let defined = defined(view, link, &chunk_rows, d, &ids, &mut g, computed, &chunks)?;
    let (patches, redirects) = patches(view, link, stored, &ids, &mut g, computed)?;
    let mut holes = Vec::new();
    for row in 0..ids.chunks.len() {
        for h in d.holes_of(stored, ChunkId(row as u32)).map_err(read)? {
            if h.kind == HoleKind::DeadSlot {
                return Err(layout(format!(
                    "holes: a DeadSlot hole in CHUNKS row {row} at {:#x} (G-9)",
                    h.offset
                )));
            }
            holes.push(Hole { chunk: ids.chunk(h.chunk, "holes")?, ..h });
        }
    }
    holes.sort_unstable();
    let frag_off = frag_off(stored, d, &ids, front)?;
    let mut slots = slot_lists(view, link, stored, d, &ids, &mut g, computed)?;
    slots.eh_records = eh_records(view, stored, d, front)?.into();
    slots.symtab_locals = symtab_locals(stored, computed)?;
    slots.sizes = sizes(computed, &chunks)?.into();
    let sized = sized(computed, &chunks)?;
    let placement = Placement {
        kind: mortar_core::PlacementKind::Loadable,
        chunks: Column::from_vec(chunks).map_err(|_| image("chunks: too many".into()))?,
        order,
        segments: d.placement.segments.clone(),
        place: Column::from_vec(place).map_err(|_| image("sections: too many".into()))?,
        sym_place: sym_place.into(),
        defined: defined.into(),
        patches: patches.into(),
        redirects: redirects.into(),
        sized,
        tls: d.placement.tls,
        entry: d.placement.entry,
        holes: holes.into(),
    };
    let l = Layout { folds: computed.folds.clone(), merge: MergeMap { frag_off }, slots, placement };
    let d_f = front_delta(view, link, d, &ids, &mut g, &l)?;
    Ok((l, d_f))
}

/// `placement.sized` (an island or an address-dependent synthetic chunk,
/// L-115): the delta does not compute one (`delta.rs:29-31`); the front's
/// row is kept when the delta's chunk has the front's size, otherwise
/// `(layout)`. RULING NEEDED: the note refused any row, but every AArch64
/// link has empty islands (size 0, one per code output section), so the
/// note's rule refuses every AArch64 update. Equal size does not prove the
/// size right at the delta's addresses (an island's need is a distance);
/// the patches table is the stored one either way.
fn sized(computed: &Layout, chunks: &[Chunk]) -> Result<Box<[Sized]>, Refusal> {
    for r in &computed.placement.sized {
        let have = chunks.get(r.chunk.slot()).map(|c| c.size);
        if have != Some(r.size) {
            return Err(layout(format!(
                "sized: chunk {} ({:?}) is {:#x} in the front, {have:#x?} in the delta (an \
                 address-dependent size the delta does not compute)",
                r.chunk.0,
                computed.placement.chunks.get(r.chunk).map(|c| c.kind),
                r.size
            )));
        }
    }
    Ok(computed.placement.sized.clone())
}

/// D-4: the delta in the FRONT's ids, for `refill` and `update_map`
/// (`check_moved_code` reads `moved` through the link's `sh_index`):
/// `moved` and `revived` by their changes' `(file, index)`, `killed`
/// dropped (old rows: `refill` reads their holes, not their ids), every
/// chunk, output section and slot key translated, `placement` the
/// materialized one. `files` keep their numbers (D-2 requires it).
fn front_delta(
    view: &StateView<'_>,
    link: &Link<'_, '_>,
    d: &LayoutDelta,
    ids: &Ids<'_>,
    g: &mut Globals<'_>,
    l: &Layout,
) -> Result<LayoutDelta, Refusal> {
    let section = |id: SectionId, list: &str| -> Result<SectionId, Refusal> {
        ids.changed.get(&id.0).copied().ok_or_else(|| {
            layout(format!("D-4: d.{list} names section {}, which no change names", id.0))
        })
    };
    let sections = |v: &[SectionId], list: &str| -> Result<Vec<SectionId>, Refusal> {
        let mut out = v.iter().map(|&id| section(id, list)).collect::<Result<Vec<_>, _>>()?;
        out.sort_unstable();
        Ok(out)
    };
    let out_of = |o: OutSecId, what: &str| -> Result<OutSecId, Refusal> {
        ids.outsecs
            .get(o.0 as usize)
            .copied()
            .flatten()
            .ok_or_else(|| image(format!("D-4: {what}: OUTSECS row {} is no output section of this link", o.0)))
    };
    let mut holes = Vec::with_capacity(d.holes.len());
    for h in &d.holes {
        holes.push(Hole { chunk: ids.chunk(h.chunk, "D-4 holes")?, ..*h });
    }
    holes.sort_unstable();
    let mut hole_chunks =
        d.hole_chunks.iter().map(|&c| ids.chunk(c, "D-4 hole_chunks")).collect::<Result<Vec<_>, _>>()?;
    hole_chunks.sort_unstable();
    let mut regions_opened = Vec::with_capacity(d.regions_opened.len());
    for r in &d.regions_opened {
        regions_opened.push(mortar_core::incr::StoredRegion {
            chunk: ids.chunk(r.chunk, "D-4 regions_opened")?,
            out: out_of(r.out, "regions_opened")?,
            ..*r
        });
    }
    let mut growth_steps = Vec::with_capacity(d.growth_steps.len());
    for s in &d.growth_steps {
        growth_steps.push(mortar_core::incr::GrowthStep { out: out_of(s.out, "growth_steps")?, ..*s });
    }
    let eh = match &d.eh {
        None => None,
        Some(e) => Some(mortar_core::incr::EhLayout { chunk: ids.chunk(e.chunk, "D-4 eh")?, ..e.clone() }),
    };
    let mut frags = Vec::with_capacity(d.frags.len());
    for f in &d.frags {
        frags.push(mortar_core::incr::StoredFrag { out: out_of(f.out, "frags")?, ..*f });
    }
    let mut slot = |v: &[StoredSlot]| -> Result<Vec<StoredSlot>, Refusal> {
        v.iter()
            .map(|s| Ok(StoredSlot { key: g.key(view, link, ids, s.key, "D-4 slots")?, ..*s }))
            .collect()
    };
    Ok(LayoutDelta {
        placement: l.placement.clone(),
        files: d.files.clone(),
        hole_chunks,
        moved: sections(&d.moved, "moved")?,
        killed: Vec::new(),
        revived: sections(&d.revived, "revived")?,
        holes,
        regions_opened,
        growth_steps,
        eh,
        frags,
        slots_appended: slot(&d.slots_appended)?,
        slots_dead: slot(&d.slots_dead)?,
        slots_revived: slot(&d.slots_revived)?,
        slots_recounted: slot(&d.slots_recounted)?,
    })
}

// ---- the identity oracle (D-8 1) ---------------------------------------------------

/// The delta of an update that changes nothing (the brief's "EMPTY
/// delta"): no file, no move, no hole, no fragment, no slot; the whole
/// tables of `placement` (chunks, segments, `defined`, TLS, entry) the
/// stored ones.
///
/// # Errors
/// The stored layout's.
pub fn unchanged_delta(stored: &StoredLayout<'_>) -> Result<LayoutDelta, IncrReason> {
    let chunks: Vec<Chunk> = stored.chunks()?.into_iter().map(|c| c.1).collect();
    let (tls, entry) = stored.tls_and_entry()?;
    let corrupt = |_| IncrReason::StateCorrupt("too many rows".into());
    let placement = Placement {
        kind: mortar_core::PlacementKind::Loadable,
        chunks: Column::from_vec(chunks).map_err(corrupt)?,
        order: stored.order()?.into(),
        segments: Column::from_vec(stored.segments()?).map_err(corrupt)?,
        defined: stored.defined()?.into(),
        tls,
        entry,
        ..Placement::default()
    };
    Ok(LayoutDelta { placement, ..LayoutDelta::default() })
}

/// The first table and row where `a` and `b` differ, or `None` (D-8 1:
/// chunks, order, segments, place, sym_place, defined, patches,
/// redirects, sized, tls, entry, holes, frag_off, each slot list, tls_ld,
/// eh_records, symtab_locals, sizes, folds, the dynamic tables).
#[must_use]
pub fn first_difference(a: &Layout, b: &Layout) -> Option<String> {
    fn rows<T: PartialEq + std::fmt::Debug>(table: &str, a: &[T], b: &[T]) -> Option<String> {
        if let Some(i) = (0..a.len().min(b.len())).find(|&i| a[i] != b[i]) {
            return Some(format!("{table} row {i}: {:?} vs {:?}", a[i], b[i]));
        }
        (a.len() != b.len()).then(|| format!("{table}: {} rows vs {}", a.len(), b.len()))
    }
    let (p, q) = (&a.placement, &b.placement);
    let (s, t) = (&a.slots, &b.slots);
    rows("chunks", p.chunks.as_slice(), q.chunks.as_slice())
        .or_else(|| rows("order", &p.order, &q.order))
        .or_else(|| rows("segments", p.segments.as_slice(), q.segments.as_slice()))
        .or_else(|| rows("place", p.place.as_slice(), q.place.as_slice()))
        .or_else(|| rows("sym_place", &p.sym_place, &q.sym_place))
        .or_else(|| rows("defined", &p.defined, &q.defined))
        .or_else(|| rows("patches", &p.patches, &q.patches))
        .or_else(|| rows("redirects", &p.redirects, &q.redirects))
        .or_else(|| rows("sized", &p.sized, &q.sized))
        .or_else(|| (p.tls != q.tls).then(|| format!("tls: {:?} vs {:?}", p.tls, q.tls)))
        .or_else(|| (p.entry != q.entry).then(|| format!("entry: {:?} vs {:?}", p.entry, q.entry)))
        .or_else(|| (p.kind != q.kind).then(|| format!("kind: {:?} vs {:?}", p.kind, q.kind)))
        .or_else(|| rows("holes", &p.holes, &q.holes))
        .or_else(|| rows("frag_off", a.merge.frag_off.as_slice(), b.merge.frag_off.as_slice()))
        .or_else(|| rows("got", &s.got, &t.got))
        .or_else(|| rows("plt", &s.plt, &t.plt))
        .or_else(|| rows("iplt", &s.iplt, &t.iplt))
        .or_else(|| rows("tls_desc", &s.tls_desc, &t.tls_desc))
        .or_else(|| rows("tls_gd", &s.tls_gd, &t.tls_gd))
        .or_else(|| rows("tls_ie", &s.tls_ie, &t.tls_ie))
        .or_else(|| (s.tls_ld != t.tls_ld).then(|| format!("tls_ld: {} vs {}", s.tls_ld, t.tls_ld)))
        .or_else(|| rows("eh_records", &s.eh_records, &t.eh_records))
        .or_else(|| rows("symtab_locals", s.symtab_locals.as_slice(), t.symtab_locals.as_slice()))
        .or_else(|| rows("sizes", &s.sizes, &t.sizes))
        .or_else(|| (a.folds != b.folds).then(|| "folds".to_owned()))
        .or_else(|| (s != t).then(|| "slots: a dynamic table".to_owned()))
        .or_else(|| (a != b).then(|| "a field no table above names".to_owned()))
}

/// The identity oracle on one front (D-8 1; the testhooks run of an
/// identity hop): `materialize` of the stored layout of `view` with
/// [`unchanged_delta`], compared table by table with the front's own.
///
/// # Errors
/// The refusal's name, or the first difference.
pub fn identity_hop(
    view: &StateView<'_>,
    link: &Link<'_, '_>,
    keys: &Keys<'_>,
    computed: &Layout,
    classes: &[MergeClass],
    eh: &[EhFile],
) -> Result<(), String> {
    let rows = crate::state5::LayoutOf { view };
    let stored = StoredLayout::new(&rows);
    let d = unchanged_delta(&stored).map_err(|e| e.report())?;
    let (l, _) = materialize(view, &stored, &d, &[], link, keys, computed, classes, eh)
        .map_err(|e| format!("{}: {}", e.0.report(), e.1))?;
    match first_difference(&l, computed) {
        None => Ok(()),
        Some(what) => Err(format!("differs: {what}")),
    }
}
