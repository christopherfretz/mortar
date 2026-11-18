//! The fill plan from the model's tables (I-27, I-38, I-41).
//!
//! `Placement` has no member list per `Body` chunk, so `place` is
//! inverted: every live, kept, placed, unfolded section with a place,
//! sorted by `(chunk, offset)`. A `Merged` chunk is one row, filled per
//! clip from merge's fragment order (I-38). Relocation sections are named
//! by their table in the input, read at fill time (I-82). Synthetic,
//! patch and structural chunks are the caller's (§13, I-40): it passes
//! their bytes or their chunk index.

use std::collections::BTreeMap;

use mortar_arch::{ProofRun, Reloc};
use mortar_core::assignment::Assignment;
use mortar_core::inputs::Reader;
use mortar_core::layout::{ChunkKind, Hole, HoleKind};
use mortar_core::par::Pool;
use mortar_core::resolution::SectionState;
use mortar_core::{
    ChunkId, Column, FileId, Inputs, Layout, Liveness, Resolution, SectionId, Split, SymEntry,
    SymKey,
};
use mortar_diag::{Diagnostic, FileRef, Kind};
use mortar_elf::{SectionIndex, SymbolIndex};
use mortar_merge::order::Order;
use mortar_synth::EhSource;

use crate::plan::{
    Builder, EhSection, MergeSource, MergedUnit, Pattern, Plan, RecRows, RelaTable, RelocSection,
    SynthUnit,
};
use crate::rows::{Rows, SectionFront};
use crate::target::Target;

/// `SHF_EXECINSTR`, `SHT_NOBITS`, `PT_LOAD`.
const SHF_EXECINSTR: u64 = 0x4;
const SHT_NOBITS: u32 = 8;
const SHT_RELA: u32 = 4;
const SHT_REL: u32 = 9;
const PT_LOAD: u32 = 1;

/// The tables a plan is built from.
#[derive(Clone, Copy)]
pub struct Model<'m, 's> {
    pub inputs: &'m Inputs<'s>,
    pub resolution: &'m Resolution,
    pub liveness: &'m Liveness,
    pub split: &'m Split,
    pub layout: &'m Layout,
    pub assignment: &'m Assignment<'m>,
    /// Phase 50's fragment order: the `Merged` chunks' contents (I-38).
    pub order: Option<&'m Order>,
    /// The `.eh_frame` source (phase 60's parse): its sections'
    /// relocations are applied over the output `.eh_frame` (S-52).
    pub eh: Option<&'m dyn EhSource>,
}

impl std::fmt::Debug for Model<'_, '_> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Model").finish_non_exhaustive()
    }
}

/// A synthetic chunk's row in the plan.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SynthRow {
    /// The index in the `chunks` slice given to fill.
    pub index: u32,
    /// `Division::Whole` (I-104).
    pub whole: bool,
    /// The output `.eh_frame` (S-52).
    pub eh: bool,
}

/// What the image does not own: the bytes of fixed chunks (the structural
/// units until their builder lands, I-40), the synthetic chunks' indexes
/// (§13), the proof of code bytes (`spec/arch.md` §3.6 rule 1), and
/// whether `-z separate-code` is on (I-41).
pub struct Extras<'e> {
    pub fixed: BTreeMap<ChunkId, Box<[u8]>>,
    pub synth: BTreeMap<ChunkId, SynthRow>,
    /// The proof runs of input section `(file, index)` of `len` bytes, in
    /// section offsets.
    pub proof: &'e dyn Fn(FileId, SectionIndex, u64) -> Vec<ProofRun>,
    pub separate_code: bool,
    /// The build id's descriptor in the file.
    pub sealed: Option<std::ops::Range<u64>>,
}

impl std::fmt::Debug for Extras<'_> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Extras")
            .field("fixed", &self.fixed.len())
            .field("synth", &self.synth)
            .finish_non_exhaustive()
    }
}

fn internal(what: &str) -> Diagnostic {
    Diagnostic::error(Kind::InternalError { what: format!("invalid fill plan: {what}") })
}

/// The path of a file, for diagnostics.
#[must_use]
pub fn file_ref(inputs: &Inputs<'_>, file: FileId) -> FileRef {
    let f = &inputs.files[file];
    let path = String::from_utf8_lossy(f.path).into_owned();
    match &f.member {
        Some(m) => FileRef::in_archive(path, String::from_utf8_lossy(m.name).into_owned()),
        None => FileRef::new(path),
    }
}

/// The records of relocation section `rel` of `file`, keys resolved
/// (`spec/arch.md` §3.5): a local entry is its own key, a global entry the
/// `SymbolId` of `sym_map`. Records the store does not hold resident (the
/// relocations of a non-alloc section, `spec/input.md` §11.1) are read
/// with one `copy_to`.
pub fn records(
    inputs: &Inputs<'_>,
    file: FileId,
    rel: SectionIndex,
) -> Result<Vec<Reloc>, Diagnostic> {
    records_in(inputs, file, rel, &mut Vec::new())
}

/// [`records`], copying records that are not resident into `buf`.
fn records_in(
    inputs: &Inputs<'_>,
    file: FileId,
    rel: SectionIndex,
    buf: &mut Vec<u8>,
) -> Result<Vec<Reloc>, Diagnostic> {
    let Reader::Object(obj) = &inputs.files[file].reader else {
        return Err(internal(&format!("file {} has relocations and no object", file.0)));
    };
    let bad = |e: mortar_elf::Error| {
        internal(&format!("file {}: relocation section {}: {e:?}", file.0, rel.0))
    };
    let sec = obj.section(rel).map_err(bad)?;
    let first = match obj.symbols().map_err(bad)? {
        Some(t) => t.first_global().0,
        None => 0,
    };
    let map = inputs.sym_map_of(file);
    let key = |r_sym: u32| -> Result<SymKey, Diagnostic> {
        if r_sym < first {
            Ok(SymKey::local(SymEntry { file, index: SymbolIndex(r_sym) }))
        } else {
            map.get((r_sym - first) as usize)
                .map(|&id| SymKey::global(id))
                .ok_or_else(|| internal(&format!("file {}: symbol {r_sym} has no key", file.0)))
        }
    };
    let mut out = Vec::new();
    if sec.sh_type() == SHT_RELA {
        let table = match sec.rela() {
            Ok(t) => t,
            Err(mortar_elf::Error::NotResident { offset, size }) => {
                let len = usize::try_from(size)
                    .map_err(|_| bad(mortar_elf::Error::NotResident { offset, size }))?;
                buf.clear();
                buf.resize(len, 0);
                inputs
                    .copy_to(file, offset..offset.saturating_add(size), buf)
                    .map_err(|e| crate::source::copy_error(e, &file_ref(inputs, file)))?;
                sec.rela_in(buf).map_err(bad)?
            }
            Err(e) => return Err(bad(e)),
        };
        for r in table {
            out.push(Reloc {
                r_offset: r.r_offset,
                key: key(r.r_sym)?,
                r_type: r.r_type,
                addend: r.r_addend,
            });
        }
    } else {
        // REL (WP-030c): the addend is in the field; `crate::rel` reads it
        // at fill, so the decoded addend is 0 (arch.md §3.3, `Records`).
        let table = match sec.rel() {
            Ok(t) => t,
            Err(mortar_elf::Error::NotResident { offset, size }) => {
                let len = usize::try_from(size)
                    .map_err(|_| bad(mortar_elf::Error::NotResident { offset, size }))?;
                buf.clear();
                buf.resize(len, 0);
                inputs
                    .copy_to(file, offset..offset.saturating_add(size), buf)
                    .map_err(|e| crate::source::copy_error(e, &file_ref(inputs, file)))?;
                sec.rel_in(buf).map_err(bad)?
            }
            Err(e) => return Err(bad(e)),
        };
        for r in table {
            out.push(Reloc {
                r_offset: r.r_offset,
                key: key(r.r_sym)?,
                r_type: r.r_type,
                addend: 0,
            });
        }
    }
    Ok(out)
}

/// Builds the plan of `m`'s placement (I-27); each placed input
/// section's row is [`crate::rows::row_for`] over `rows` (the same rows'
/// placement as `m`'s).
pub fn from_layout<R: Rows>(
    m: &Model<'_, '_>,
    rows: &R,
    target: Target,
    extras: &Extras<'_>,
    window: u64,
    pool: &Pool,
) -> Result<Plan, Diagnostic> {
    let pl = &m.layout.placement;
    let chunks = &pl.chunks;
    let file_size = chunks
        .iter()
        .filter(|(_, c)| c.in_file())
        .map(|(_, c)| c.offset + c.size)
        .max()
        .unwrap_or(0);
    let mut b = Builder::new(target, file_size);
    // Placed input sections, in (chunk, offset) order.
    let mut members: Vec<(ChunkId, u64, SectionId)> = Vec::new();
    for (s, place) in pl.place.iter() {
        let Some((c, off)) = place.get() else { continue };
        if is_member(m, s) {
            members.push((c, off, s));
        }
    }
    pool.unit_cost(15).sort_by_key(&mut members, |&(c, off, _)| (c.0, off));
    let mut paths: BTreeMap<FileId, FileRef> = BTreeMap::new();
    for &(c, off, s) in &members {
        let Some(front) = member_front(m, s, &chunks[c])? else { continue };
        paths.entry(front.file).or_insert_with(|| file_ref(m.inputs, front.file));
        crate::rows::row_for(rows, s, (c, off), &front, extras.proof, &mut b)?;
    }
    merged(m, &mut b, &mut paths)?;
    debug_holes(m, target, &mut b)?;
    for (&c, bytes) in &extras.fixed {
        b.bytes(chunks[c].offset, bytes.clone());
    }
    for (&c, row) in &extras.synth {
        let ch = &chunks[c];
        let name = match ch.kind {
            ChunkKind::Synthetic(s) => s.name,
            _ => ch.name(m.assignment).unwrap_or(b""),
        };
        let name = b.name(name);
        let unit = SynthUnit { chunk: row.index, id: c, name, whole: row.whole, eh: row.eh };
        b.synth(ch.offset, ch.size, unit);
        if row.eh {
            eh_sections(m, &mut b, &mut paths)?;
        }
    }
    let (chunk_views, seg_views) = pad_views(m);
    pads(chunk_views, &seg_views, target, extras.separate_code, &mut b);
    for (f, p) in paths {
        b.path(f, p);
    }
    if let Some(s) = &extras.sealed {
        b.sealed(s.clone());
    }
    b.finish(window, pool)
}

/// Whether section `s` is a member of the plan: kept, placed, live and
/// not folded (its place is the caller's).
pub(crate) fn is_member(m: &Model<'_, '_>, s: SectionId) -> bool {
    let row = &m.resolution.sections[s];
    matches!(row.state, SectionState::Kept)
        && row.kind.is_placed()
        && m.liveness.is_live(s)
        && !m.layout.folds.is_folded(s)
}

/// The front of member `s` placed in `chunk` ([`crate::rows::row_for`]'s
/// input), or `None` when it takes no row: its chunk is not an in-file
/// `Body` chunk, it is not in an object, or it has no file bytes.
pub(crate) fn member_front<'m>(
    m: &Model<'m, '_>,
    s: SectionId,
    chunk: &mortar_core::layout::Chunk,
) -> Result<Option<SectionFront<'m>>, Diagnostic> {
    if !chunk.in_file() || !matches!(chunk.kind, ChunkKind::Body(_)) {
        return Ok(None);
    }
    let (file, index) = m
        .resolution
        .sh_index(s)
        .ok_or_else(|| internal(&format!("section {} has no file", s.0)))?;
    let Reader::Object(obj) = &m.inputs.files[file].reader else { return Ok(None) };
    let sec = obj.section(index).map_err(|e| internal(&format!("{e:?}")))?;
    if sec.sh_type() == SHT_NOBITS || sec.size() == 0 {
        return Ok(None);
    }
    let Some(range) = sec.file_range().map_err(|e| internal(&format!("{e:?}")))? else {
        return Ok(None);
    };
    let rel = m.resolution.sections[s].reloc;
    Ok(Some(SectionFront {
        file,
        index,
        size: sec.size(),
        flags: sec.flags(),
        src: range.start as u64,
        name: sec.name().unwrap_or(b""),
        rela: if rel.0 == 0 { None } else { Some(table(m.inputs, file, rel)?) },
    }))
}

/// `spec/incr.md` IN-51b: the bytes of every debug hole (a neutral unit
/// of its section's form) and of the neutral abbreviation table.
fn debug_holes(m: &Model<'_, '_>, target: Target, b: &mut Builder) -> Result<(), Diagnostic> {
    let pl = &m.layout.placement;
    if pl.holes.is_empty() {
        return Ok(());
    }
    let abbrev = abbrev_offset(&pl.holes, &pl.chunks)?;
    for h in &pl.holes {
        let c = &pl.chunks[h.chunk];
        if !c.in_file() {
            continue;
        }
        let name = c.name(m.assignment).unwrap_or(b"");
        if let Some(bytes) = hole_bytes(h, name, target, abbrev)? {
            b.bytes(c.offset + h.offset, bytes.into_boxed_slice());
        }
    }
    Ok(())
}

/// The neutral abbreviation table's offset in its output section (0
/// without one): what every neutral unit's `debug_abbrev_offset` is
/// (IN-51b). `chunks` is the placement's, whole: an output section's
/// start is its first chunk's offset.
///
/// # Errors
///
/// `internal-error` for a table past 4 GiB.
pub fn abbrev_offset(
    holes: &[Hole],
    chunks: &Column<ChunkId, mortar_core::layout::Chunk>,
) -> Result<u32, Diagnostic> {
    // The start of an output section: its first chunk's offset (by
    // section index).
    let start = |shndx: Option<u32>| {
        chunks
            .iter()
            .filter(|(_, c)| c.shndx == shndx && c.in_file())
            .map(|(_, c)| c.offset)
            .min()
            .unwrap_or(0)
    };
    let abbrev = holes.iter().find(|h| h.kind == HoleKind::AbbrevTable).map_or(0, |h| {
        let c = &chunks[h.chunk];
        c.offset + h.offset - start(c.shndx)
    });
    u32::try_from(abbrev).map_err(|_| internal("a neutral abbrev table past 4 GiB"))
}

/// The bytes of hole `h` in a chunk named `name` (IN-51b): a neutral
/// unit of its section's form, or the neutral abbreviation table; `None`
/// for a hole whose bytes are not a row's (zero, or a synthetic chunk's).
///
/// # Errors
///
/// `internal-error` when no neutral unit has the hole's size.
pub fn hole_bytes(
    h: &Hole,
    name: &[u8],
    target: Target,
    abbrev: u32,
) -> Result<Option<Vec<u8>>, Diagnostic> {
    let addr_size = match target.class {
        mortar_elf::Class::Elf64 => 8,
        mortar_elf::Class::Elf32 => 4,
    };
    Ok(Some(match h.kind {
        HoleKind::AbbrevTable => {
            let mut t = crate::neutral::ABBREV_TABLE.to_vec();
            t.resize(usize::try_from(h.size).unwrap_or(0).max(t.len()), 0);
            t
        }
        HoleKind::Debug | HoleKind::DeadDebug => {
            crate::neutral::unit(name, h.size, 5, addr_size, abbrev).ok_or_else(|| {
                internal(&format!(
                    "no neutral unit of {:#x} bytes in {}",
                    h.size,
                    String::from_utf8_lossy(name)
                ))
            })?
        }
        _ => return Ok(None),
    }))
}

/// The table of relocation section `rel` of `file` (`SHT_RELA`; REL is
/// M3).
fn table(inputs: &Inputs<'_>, file: FileId, rel: SectionIndex) -> Result<RelaTable, Diagnostic> {
    let Reader::Object(obj) = &inputs.files[file].reader else {
        return Err(internal(&format!("file {} has relocations and no object", file.0)));
    };
    let bad = |e: mortar_elf::Error| {
        internal(&format!("file {}: relocation section {}: {e:?}", file.0, rel.0))
    };
    let sec = obj.section(rel).map_err(bad)?;
    // REL (i386, WP-030c): the addend is in the relocated field, read at
    // fill (`crate::rel`).
    let rel_form = sec.sh_type() == SHT_REL;
    let class = obj.class();
    let entsize = RelaTable { file, offset: 0, count: 0, class, rel: rel_form }.entsize();
    let range = sec.file_range().map_err(bad)?.unwrap_or(0..0);
    let len = (range.end - range.start) as u64;
    if !len.is_multiple_of(entsize) {
        return Err(internal(&format!(
            "file {}: relocation section {} is not whole records",
            file.0, rel.0
        )));
    }
    let count = u32::try_from(len / entsize).map_err(|_| {
        internal(&format!("file {}: relocation section {} too large", file.0, rel.0))
    })?;
    Ok(RelaTable { file, offset: range.start as u64, count, class, rel: rel_form })
}

/// I-38: every `Merged` chunk is one row, and every merge section a
/// live fragment is copied from is a source.
fn merged(
    m: &Model<'_, '_>,
    b: &mut Builder,
    paths: &mut BTreeMap<FileId, FileRef>,
) -> Result<(), Diagnostic> {
    let pl = &m.layout.placement;
    let mut any = false;
    for (c, ch) in pl.chunks.iter() {
        let ChunkKind::Merged(o) = ch.kind else { continue };
        if !ch.in_file() || ch.size == 0 {
            continue;
        }
        let Some(order) = m.order else {
            return Err(internal(&format!("merged chunk {} and no fragment order", c.0)));
        };
        let Ok(k) = order.chunks.binary_search_by_key(&o, |mc| mc.out) else {
            return Err(internal(&format!("merged chunk {} is not in the fragment order", c.0)));
        };
        let name = b.name(m.assignment.outsecs[o].name);
        b.merged(ch.offset, ch.size, MergedUnit { chunk: k as u32, name });
        any = true;
    }
    if !any {
        return Ok(());
    }
    let mut secs: Vec<SectionId> = m
        .split
        .frags
        .iter()
        .filter(|(f, _)| m.layout.merge.offset(*f).is_some())
        .map(|(_, frag)| frag.section)
        .collect();
    secs.sort_unstable();
    secs.dedup();
    for s in secs {
        let (file, index) = m
            .resolution
            .sh_index(s)
            .ok_or_else(|| internal(&format!("section {} has no file", s.0)))?;
        let Reader::Object(obj) = &m.inputs.files[file].reader else { continue };
        let sec = obj.section(index).map_err(|e| internal(&format!("{e:?}")))?;
        let Some(range) = sec.file_range().map_err(|e| internal(&format!("{e:?}")))? else {
            continue;
        };
        if m.resolution.sections[s].reloc.0 != 0 {
            // S-6: a merge section has no relocations; resolve demotes one.
            return Err(internal(&format!(
                "merge section {} of file {} has relocations",
                index.0, file.0
            )));
        }
        paths.entry(file).or_insert_with(|| file_ref(m.inputs, file));
        b.merge_source(MergeSource { section: s, file, src: range.start as u64, size: sec.size() });
    }
    Ok(())
}

/// S-52: every live input `.eh_frame` section with relocations.
fn eh_sections(
    m: &Model<'_, '_>,
    b: &mut Builder,
    paths: &mut BTreeMap<FileId, FileRef>,
) -> Result<(), Diagnostic> {
    let Some(src) = m.eh else {
        return Err(internal("the .eh_frame chunk and no .eh_frame source"));
    };
    for sec in &src.frames().sections {
        let s = sec.section;
        let rel = m.resolution.sections[s].reloc;
        if rel.0 == 0 || !m.liveness.is_live(s) {
            continue;
        }
        let (file, index) = m
            .resolution
            .sh_index(s)
            .ok_or_else(|| internal(&format!("section {} has no file", s.0)))?;
        paths.entry(file).or_insert_with(|| file_ref(m.inputs, file));
        let rows = RecRows::Rela(table(m.inputs, file, rel)?);
        let relocs = b.relocs(RelocSection { section: s, rows, proof: Box::new([]) });
        b.eh_section(EhSection { section: s, file, index: index.0, relocs });
    }
    Ok(())
}

/// One chunk as I-41's pad pass reads it: its file offset, the end of its
/// capacity (`spec/incr.md` IN-33: a code chunk's capacity is trap fill
/// too), whether it is code and whether it is `.init`/`.fini`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ChunkView {
    pub offset: u64,
    pub end: u64,
    pub exec: bool,
    pub init: bool,
}

/// One `PT_LOAD` segment as I-41's pad pass reads it: its file range and
/// whether it is executable.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SegView {
    pub offset: u64,
    pub end: u64,
    pub exec: bool,
}

/// The pad pass's views of `m`'s placement: every in-file chunk with
/// bytes, and every `PT_LOAD` segment, in column order.
pub(crate) fn pad_views(m: &Model<'_, '_>) -> (Vec<ChunkView>, Vec<SegView>) {
    let pl = &m.layout.placement;
    let chunks = pl
        .chunks
        .iter()
        .filter(|(_, c)| c.in_file() && c.size > 0)
        .map(|(_, c)| ChunkView {
            offset: c.offset,
            end: c.offset + c.capacity,
            exec: c.flags & SHF_EXECINSTR != 0,
            init: matches!(c.name(m.assignment), Some(b".init" | b".fini")),
        })
        .collect();
    let segs = pl
        .segments
        .iter()
        .filter(|(_, s)| s.p_type == PT_LOAD)
        .map(|(_, s)| SegView {
            offset: s.offset,
            end: s.offset + s.filesz,
            exec: s.flags & 1 != 0,
        })
        .collect();
    (chunks, segs)
}

/// I-41 / L-82 / L-83: the pad ranges whose gaps are not zero, over views
/// of the chunks and the `PT_LOAD` segments (WP-255 §9.1: a whole plan
/// and a clip plan call this same maker, whole).
pub fn pads(
    mut chunks: Vec<ChunkView>,
    segs: &[SegView],
    target: Target,
    separate_code: bool,
    b: &mut Builder,
) {
    chunks.sort_unstable_by_key(|c| c.offset);
    let seg_of =
        |off: u64| segs.iter().position(|s| s.offset <= off && off < s.end.max(s.offset + 1));
    for (k, c) in chunks.iter().enumerate() {
        if !c.exec {
            continue;
        }
        let (lo, hi) = (c.offset, c.end);
        b.pad(lo..hi, if c.init { target.nop } else { target.trap });
        if let Some(n) = chunks.get(k + 1)
            && n.exec
            && hi < n.offset
            && seg_of(lo).is_some()
            && seg_of(lo) == seg_of(n.offset)
        {
            b.pad(hi..n.offset, target.trap);
        }
    }
    if separate_code {
        let mut segs = segs.to_vec();
        segs.sort_unstable_by_key(|s| s.offset);
        for w in segs.windows(2) {
            if w[0].exec && w[0].end < w[1].offset {
                b.pad(w[0].end..w[1].offset, target.trap);
            }
        }
    }
    let _ = Pattern::ZERO;
}
