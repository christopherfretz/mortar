//! Stage C (`spec/layout.md` §10, §12): which chunks exist, their kinds,
//! flags, alignments and address-independent sizes, their order, and the
//! offset of every placed section inside its chunk.
//!
//! Chunk ids follow model §4.4's creation order: the file header, the
//! program headers, the section headers, the synthetic chunks in the
//! caller's order, the RELRO padding, then per `OutSecId` `Body(o, 0)`,
//! `Patches(o, 0)`, `Body(o, 1)`, …, `Merged(o)`.

use mortar_core::assignment::Assignment;
use mortar_core::column::Column;
use mortar_core::id::{ChunkId, FileId, IdRange, OutSecId, SectionId, SymbolId};
use mortar_core::inputs::{Inputs, Reader, SymbolKey};
use mortar_core::layout::{
    AllocKey, Chunk, ChunkKind, CopyAlloc, CopyPlace, Folds, Hole, HoleKind, MergeMap, Part, Place,
    SegmentRules, Slots, SymPlace, Synth, derive_segments,
};
use mortar_core::liveness::Liveness;
use mortar_core::par::Pool;
use mortar_core::resolution::{Def, InternalDef, Resolution};
use mortar_core::split::Split;
use mortar_elf::Class;
use mortar_elf::consts::{
    SHF_ALLOC, SHF_EXECINSTR, SHF_WRITE, SHF_X86_64_LARGE, SHT_NOBITS, SHT_PROGBITS, STT_TLS,
};

use crate::assign::{OutInfo, is_c_identifier};
use crate::diag::{LayoutDiag, feature, internal, section_conflict};
use crate::headers::Headers;
use crate::order::{Ordered, text_category};
use crate::rank::{Rank, Z, home, leads, rank_in};
use crate::slack::{self, Neutral, Slack};
use crate::target::Target;

/// A synthetic chunk the caller (synthesis) asks for, with its
/// address-independent size (`spec/synth.md`, not yet written).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SynthChunk {
    pub synth: Synth,
    pub sh_type: u32,
    pub flags: u64,
    /// log2.
    pub align: u8,
    pub size: u64,
}

/// A hole the caller lays out inside a synthetic chunk (`spec/incr.md`
/// IN-34a: an `.eh_frame` range's padding), at an offset from the
/// chunk's start.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SynthHole {
    pub synth: Synth,
    pub offset: u64,
    pub size: u64,
    pub kind: HoleKind,
}

/// A common symbol resolution chose (L-88), with whether it is `STT_TLS`
/// (L-89).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Common {
    pub symbol: SymbolId,
    pub size: u64,
    /// log2.
    pub align: u8,
    pub tls: bool,
    /// x86-64: every mention is `SHN_X86_64_LCOMMON`, so it is allocated
    /// in `.lbss` (L-88, `spec/resolve.md` §3.3).
    pub large: bool,
}

/// The commons of a resolution, in `SymbolId` order, reading each one's
/// type from its defining file.
#[must_use]
pub fn commons(resolution: &Resolution, inputs: Option<&Inputs<'_>>) -> Vec<Common> {
    resolution
        .def
        .iter()
        .filter_map(|(s, d)| {
            let Def::Common(entry) = *d else { return None };
            let shape = resolution.common_of(s)?;
            let tls = inputs.is_some_and(|inputs| {
                let Some(Reader::Object(o)) = inputs.files.get(entry.file).map(|f| f.reader) else {
                    return false;
                };
                o.symbols()
                    .ok()
                    .flatten()
                    .and_then(|t| t.get(entry.index).ok())
                    .is_some_and(|sym| sym.st_info & 0xf == STT_TLS)
            });
            Some(Common {
                symbol: s,
                size: shape.size,
                align: shape.align,
                tls,
                large: shape.large,
            })
        })
        .collect()
}

/// A copy-relocation allocation as layout places it (`spec/dyn.md` D-42):
/// the allocation `index` of `Slots::copies`, its chunk, size and
/// alignment, and the least `SymbolId` among its aliases, which orders the
/// allocations inside their chunk.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CopyRow {
    pub index: u32,
    pub place: CopyPlace,
    /// The defining entry's `st_size` (D-42 "size"; never 0: refused
    /// before layout, `copy-relocation-of-zero-size`).
    pub size: u64,
    /// log2 of the alignment (D-42 "alignment").
    pub align: u8,
    /// The least `SymbolId` among the aliases at the allocation's
    /// `(file, st_value)` (D-42 "order within each chunk").
    pub first: SymbolId,
}

/// The copy rows of `slots`, with each allocation's size from `size`
/// (`Slots::copies` does not carry it): the least alias of each
/// allocation from `Slots::copy_of` (its record's alias when no key maps
/// to it).
///
/// # Errors
///
/// `internal-error` for an alignment that is not a power of two (D-42
/// computes one).
pub fn copy_rows(
    slots: &Slots,
    size: impl Fn(u32, &CopyAlloc) -> u64,
) -> Result<Vec<CopyRow>, LayoutDiag> {
    let mut first: Vec<SymbolId> = slots.copies.iter().map(|c| c.alias).collect();
    for &(s, i) in &*slots.copy_of {
        if let Some(f) = first.get_mut(i as usize) {
            *f = (*f).min(s);
        }
    }
    slots
        .copies
        .iter()
        .enumerate()
        .map(|(i, c)| {
            if !c.align.is_power_of_two() {
                return Err(internal("a copy allocation's alignment is not a power of two"));
            }
            Ok(CopyRow {
                index: i as u32,
                place: c.place,
                size: size(i as u32, c),
                align: c.align.trailing_zeros() as u8,
                first: first[i],
            })
        })
        .collect()
}

/// [`copy_extent`]'s result: the rows with their offsets, the chunk's
/// size and its alignment (log2).
pub type CopyPlan = (Vec<(CopyRow, u64)>, u64, u8);

/// The allocations of one copy chunk (D-42): in the order of their least
/// alias, each at the aligned prefix sum (L-22), as `(row, offset)`; and
/// the chunk's size and alignment (log2). What synthesis gives as the
/// size of `.dynbss` or `.bss.rel.ro` must be this size.
///
/// # Errors
///
/// `internal-error` when the offsets overflow.
pub fn copy_extent(rows: &[CopyRow], place: CopyPlace) -> Result<CopyPlan, LayoutDiag> {
    let mut mine: Vec<CopyRow> = rows.iter().copied().filter(|r| r.place == place).collect();
    mine.sort_by_key(|r| (r.first, r.index));
    let (mut size, mut align) = (0u64, 0u8);
    let mut out = Vec::with_capacity(mine.len());
    for r in mine {
        if r.align > 63 {
            return Err(internal("a copy allocation aligned beyond 2^63"));
        }
        let at = size
            .checked_next_multiple_of(1u64 << r.align)
            .ok_or_else(|| internal("copy offsets overflow"))?;
        size = at.checked_add(r.size).ok_or_else(|| internal("copy offsets overflow"))?;
        align = align.max(r.align);
        out.push((r, at));
    }
    Ok((out, size, align))
}

/// The output sections named by a confirmed `__start_SEC`/`__stop_SEC`
/// (L-43: they get a chunk even when empty), sorted.
#[must_use]
pub fn start_stop_outputs(
    assignment: &Assignment<'_>,
    resolution: &Resolution,
    symbols: &Column<SymbolId, SymbolKey<'_>>,
) -> Vec<OutSecId> {
    let mut out: Vec<OutSecId> = resolution
        .def
        .iter()
        .filter(|(s, d)| {
            matches!(d, Def::Internal(InternalDef::Provide { conditional: true }))
                && !assignment.is_withdrawn(*s)
        })
        .filter_map(|(s, _)| {
            let name = symbols.get(s)?.name;
            let sec = name.strip_prefix(b"__start_").or_else(|| name.strip_prefix(b"__stop_"))?;
            if !is_c_identifier(sec) {
                return None;
            }
            assignment.outsecs.iter().find(|(_, o)| o.name == sec).map(|(id, _)| id)
        })
        .collect();
    out.sort_unstable();
    out.dedup();
    out
}

/// What stage C hands to stage D.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Staged {
    /// The chunks; `addr` is `Some(0)` for alloc chunks and `offset` 0
    /// until stage D. Patch chunks have size 0 and the padding size 0.
    pub chunks: Column<ChunkId, Chunk>,
    /// File order.
    pub order: Box<[ChunkId]>,
    /// Each chunk's rank.
    pub ranks: Column<ChunkId, Rank>,
    /// The places of placed sections (offsets inside their chunk).
    pub place: Column<SectionId, Place>,
    /// The commons, in `.bss` (offsets inside their chunk).
    pub sym_place: Box<[SymPlace]>,
    /// The islands (`Patches` chunks), in `ChunkId` order.
    pub islands: Box<[ChunkId]>,
    /// The RELRO padding, when rank 7 is non-empty.
    pub relro_padding: Option<ChunkId>,
    /// The output section of each chunk that is part of one.
    pub out_of: Column<ChunkId, Option<OutSecId>>,
    /// The holes (`spec/incr.md` IN-3), sorted by `(chunk, offset)`:
    /// empty without slack.
    pub holes: Box<[Hole]>,
}

impl Staged {
    /// The island of the part a body chunk is: `Patches(part)`.
    #[must_use]
    pub fn island_of(&self, body: ChunkId) -> Option<ChunkId> {
        let ChunkKind::Body(part) = self.chunks.get(body)?.kind else { return None };
        self.islands.iter().copied().find(|&i| self.chunks[i].kind == ChunkKind::Patches(part))
    }
}

/// The inputs of stage C.
#[derive(Debug, Clone, Copy)]
pub struct ChunkInputs<'t, 'n> {
    pub assignment: &'t Assignment<'n>,
    pub info: &'t Column<OutSecId, OutInfo>,
    pub headers: &'t Headers<'n>,
    pub ordered: &'t Ordered,
    pub folds: &'t Folds,
    pub split: &'t Split,
    pub merge: &'t MergeMap,
    pub liveness: &'t Liveness,
    pub commons: &'t [Common],
    pub synthetic: &'t [SynthChunk],
    /// Output sections that need a chunk even when empty (L-43).
    pub keep_empty: &'t [OutSecId],
    /// `-z relro` (the default); `-z norelro` puts rank 7 in the RW
    /// segment with no padding and no `PT_GNU_RELRO` (§20.2).
    pub relro: bool,
    /// `-z now`: `.got.plt` is RELRO (`rank_with`, L-60).
    pub now: bool,
    /// The copy-relocation allocations (D-42), placed in the synthetic
    /// chunks `.dynbss` and `.bss.rel.ro`; empty in a static link.
    pub copies: &'t [CopyRow],
    /// The slack policy (`spec/incr.md` IN-33).
    pub slack: Slack,
    /// Per file, its section ids (`Resolution::section_base`): the files
    /// of regions (IN-30) and debug contributions (IN-50). Needed with
    /// slack only.
    pub section_base: Option<&'t Column<FileId, IdRange<SectionId>>>,
    /// The holes inside synthetic chunks (IN-34a).
    pub synth_holes: &'t [SynthHole],
}

/// One body part: its members and their offsets, size and alignment,
/// and its holes (offset, size, kind).
#[derive(Debug, Clone, Default)]
struct PartPlan {
    members: Vec<(SectionId, u64)>,
    size: u64,
    align: u8,
    holes: Vec<(u64, u64, HoleKind)>,
}

/// The gaps an output section's body takes between its members
/// (`spec/incr.md` IN-30, IN-50): none in a plain link.
#[derive(Clone, Copy)]
enum Gaps<'a> {
    None,
    /// IN-30: a region per run of members with one key (file, category),
    /// followed by [`Slack::region`]'s slack as a [`HoleKind::Fill`] hole.
    Regions(&'a dyn Fn(SectionId) -> u64, Slack),
    /// IN-50: a contribution per run of one file's members, allotted its
    /// size class; the rest a [`HoleKind::Debug`] hole. With `abbrev`, the
    /// neutral abbreviation table follows the last member.
    Debug { file: &'a dyn Fn(SectionId) -> u64, slack: Slack, form: Neutral, abbrev: bool },
}

/// The open region of [`plan_parts`]: its key, start offset in the
/// current part and alignment.
#[derive(Clone, Copy)]
struct Open {
    key: u64,
    start: u64,
    align: u8,
}

/// Closes `open` at the end of `part`: its slack (at most up to `limit`)
/// becomes a hole.
fn close(part: &mut PartPlan, open: Open, gaps: Gaps<'_>, limit: Option<u64>) {
    let size = part.size - open.start;
    let (pad, kind) = match gaps {
        Gaps::None => return,
        Gaps::Regions(_, slack) => (slack.region(size, open.align), HoleKind::Fill),
        Gaps::Debug { slack, form, .. } => (slack.allotment(size, form), HoleKind::Debug),
    };
    let room = limit.map_or(u64::MAX, |l| l.saturating_sub(part.size));
    let pad = if matches!(gaps, Gaps::Regions(..)) { pad.min(room) } else { pad };
    if pad == 0 || (matches!(gaps, Gaps::Debug { .. }) && pad > room) {
        return;
    }
    let Some(end) = part.size.checked_add(pad) else { return };
    part.holes.push((part.size, pad, kind));
    part.size = end;
}

/// The parts of one output section's body (L-22, L-110): the aligned
/// prefix sum, cut before the member that would make a part larger than
/// `limit`; with `gaps`, each region's or contribution's slack after it
/// (IN-30, IN-50).
fn plan_parts(
    members: &[SectionId],
    headers: &Headers<'_>,
    limit: Option<u64>,
    gaps: Gaps<'_>,
) -> Result<Vec<PartPlan>, LayoutDiag> {
    let mut parts = vec![PartPlan::default()];
    let key_of = |s: SectionId| match gaps {
        Gaps::None => 0,
        Gaps::Regions(key, _) => key(s),
        Gaps::Debug { file, .. } => file(s),
    };
    let mut open: Option<Open> = None;
    for &s in members {
        let h = &headers[s];
        if h.align > 63 {
            return Err(internal("a section alignment beyond 2^63"));
        }
        if let Some(limit) = limit
            && h.size > limit
        {
            return Err(feature("code section larger than branch reach"));
        }
        let key = key_of(s);
        if let Some(o) = open
            && o.key != key
        {
            close(parts.last_mut().expect("one part at least"), o, gaps, limit);
            open = None;
        }
        let a = 1u64 << h.align;
        let place = |p: &PartPlan| p.size.checked_next_multiple_of(a);
        let mut part = parts.last_mut().expect("one part at least");
        let mut at = place(part).ok_or_else(|| internal("section offsets overflow"))?;
        let mut end = at.checked_add(h.size).ok_or_else(|| internal("section offsets overflow"))?;
        if let Some(limit) = limit
            && end > limit
            && !part.members.is_empty()
        {
            if let Some(o) = open.take() {
                close(part, o, gaps, Some(limit));
            }
            parts.push(PartPlan::default());
            part = parts.last_mut().expect("just pushed");
            at = 0;
            end = h.size;
        }
        let o = open.get_or_insert(Open { key, start: at, align: h.align });
        o.align = o.align.max(h.align);
        part.members.push((s, at));
        part.size = end;
        part.align = part.align.max(h.align);
    }
    let part = parts.last_mut().expect("one part at least");
    if let Some(o) = open {
        close(part, o, gaps, limit);
    }
    if let Gaps::Debug { slack, abbrev: true, .. } = gaps
        && slack.on
        && !part.members.is_empty()
    {
        part.holes.push((part.size, slack::ABBREV_TABLE, HoleKind::AbbrevTable));
        part.size += slack::ABBREV_TABLE;
    }
    Ok(parts)
}

/// Header chunk sizes by class.
const fn header_sizes(class: Class) -> (u64, u64, u64, u8) {
    match class {
        // (ehdr, phdr entry, shdr entry, alignment)
        Class::Elf64 => (64, 56, 64, 3),
        Class::Elf32 => (52, 32, 40, 2),
    }
}

/// Stage C.
///
/// # Errors
///
/// L-89 (a TLS common), L-110 (a code section larger than `S`), and
/// `internal-error` for a size that overflows or a copy chunk that is
/// missing or not the size of its allocations (D-42).
#[allow(clippy::too_many_lines)]
pub fn chunk(
    inputs: &ChunkInputs<'_, '_>,
    target: &Target,
    rules: &SegmentRules,
    pool: &Pool,
) -> Result<Staged, LayoutDiag> {
    let a = inputs.assignment;
    let outs = a.outsecs.len() as usize;
    if inputs.commons.iter().any(|c| c.tls) {
        return Err(feature("TLS common symbol"));
    }
    let named = |name: &[u8]| a.outsecs.iter().find(|(_, o)| o.name == name).map(|(id, _)| id);
    let bss = named(b".bss");
    let lbss = named(b".lbss");
    // L-88: the commons of each section, in SymbolId order: the small ones
    // in `.bss`, x86-64's large ones in `.lbss` (L-36a), which must then be
    // the large section (an input named `.lbss` without the flag is not).
    let commons_of = |o: OutSecId| -> Vec<&Common> {
        let large = Some(o) == lbss;
        inputs.commons.iter().filter(|c| c.large == large).collect()
    };
    if let Some(l) = lbss
        && inputs.commons.iter().any(|c| c.large)
        && a.outsecs[l].flags & SHF_X86_64_LARGE == 0
    {
        return Err(section_conflict(".lbss".into(), "SHF_X86_64_LARGE", "not SHF_X86_64_LARGE"));
    }
    // spec/incr.md IN-30, IN-50: the file of a section, for regions and
    // debug contributions.
    let slack = inputs.slack;
    let mut starts: Vec<(u32, u32)> = match inputs.section_base.filter(|_| slack.on) {
        Some(b) => b.iter().filter(|(_, r)| r.len() > 0).map(|(f, r)| (r.start.0, f.0)).collect(),
        None => Vec::new(),
    };
    starts.sort_unstable();
    let file_of = |s: SectionId| -> u64 {
        let at = starts.partition_point(|&(start, _)| start <= s.0);
        at.checked_sub(1).map_or(u64::from(u32::MAX), |i| u64::from(starts[i].1))
    };
    let region_key = |s: SectionId| -> u64 {
        let h = &inputs.headers[s];
        let cat = if h.flags & SHF_EXECINSTR != 0 { u64::from(text_category(h.name)) } else { 0 };
        file_of(s) << 8 | cat
    };
    let debug_info = a.outsecs.iter().any(|(id, o)| {
        o.name == b".debug_info" && !inputs.ordered.members[id].is_empty()
    });
    // Parts per output section, parallel over output sections (L-22).
    let planned: Vec<Result<Vec<PartPlan>, LayoutDiag>> = pool.unit_cost(10000).map(outs, |o| {
        let id = OutSecId(o as u32);
        let out = &a.outsecs[id];
        let exec = out.flags & SHF_EXECINSTR != 0;
        let limit = target.islands.filter(|_| exec).map(|i| i.part_limit);
        let gaps = if !slack.on || inputs.section_base.is_none() {
            Gaps::None
        } else if out.flags & SHF_ALLOC != 0 {
            match slack::class(out.name, out.flags) {
                slack::Class::Tolerant => Gaps::Regions(&region_key, slack),
                _ => Gaps::None,
            }
        } else if let Some(form) = slack::neutral(out.name) {
            let abbrev = out.name == b".debug_abbrev" && debug_info;
            Gaps::Debug { file: &file_of, slack, form, abbrev }
        } else {
            Gaps::None
        };
        plan_parts(&inputs.ordered.members[id], inputs.headers, limit, gaps)
    });
    let mut plans = Vec::with_capacity(outs);
    for p in planned {
        plans.push(p?);
    }
    // Merged chunks: per output section, the extent and alignment of its
    // live fragments (MergeMap offsets).
    let mut merged: Vec<Option<(u64, u8)>> = vec![None; outs];
    for (f, frag) in inputs.split.frags.iter() {
        let Some(off) = inputs.merge.offset(f) else { continue };
        if !inputs.liveness.frag_live.get(f).copied().unwrap_or(true) {
            continue;
        }
        let Some(o) = a.out(frag.section) else { continue };
        let end = off.saturating_add(u64::from(frag.size));
        let m = merged[o.0 as usize].get_or_insert((0, 0));
        m.0 = m.0.max(end);
        m.1 = m.1.max(frag.align);
    }
    // Synthetic chunks named like an output section join it; `.dynbss`
    // joins `.bss` (`rank::home`).
    let joins: Vec<Option<OutSecId>> = inputs
        .synthetic
        .iter()
        .map(|s| {
            let name = home(s.synth.name);
            a.outsecs.iter().find(|(_, o)| o.name == name).map(|(id, _)| id)
        })
        .collect();
    let mut has_synth = vec![false; outs];
    for j in joins.iter().flatten() {
        has_synth[j.0 as usize] = true;
    }
    // Which output sections have chunks (L-43).
    let has_commons = |id: OutSecId| {
        (Some(id) == bss && inputs.commons.iter().any(|c| !c.large))
            || (Some(id) == lbss && inputs.commons.iter().any(|c| c.large))
    };
    // L-159: an allocated output section whose live members are all empty
    // and that nothing else fills has no chunk; its members are placed
    // beside a neighbour once the order is known (below).
    let empty_only: Vec<bool> = (0..outs)
        .map(|o| {
            let id = OutSecId(o as u32);
            let members = &inputs.ordered.members[id];
            a.outsecs[id].flags & SHF_ALLOC != 0
                && !members.is_empty()
                && members.iter().all(|&s| inputs.headers[s].size == 0)
                && merged[o].is_none()
                && !has_synth[o]
                && !has_commons(id)
                && !inputs.keep_empty.contains(&id)
                && !a.outsecs[id].keep
        })
        .collect();
    let present: Vec<bool> = (0..outs)
        .map(|o| {
            let id = OutSecId(o as u32);
            (!inputs.ordered.members[id].is_empty()
                || merged[o].is_some()
                || has_commons(id)
                || inputs.keep_empty.contains(&id)
                || a.outsecs[id].keep)
                && !empty_only[o]
        })
        .collect();
    let keywords = Z { now: inputs.now, relro: inputs.relro };
    let rank_of_out = |o: usize| {
        let out = &a.outsecs[OutSecId(o as u32)];
        rank_in(out.name, out.sh_type, out.flags, keywords)
    };
    // RELRO padding exists when rank 7 is non-empty (L-64).
    let relro_nonempty = (0..outs).any(|o| present[o] && rank_of_out(o).is_relro())
        || inputs
            .synthetic
            .iter()
            .any(|s| rank_in(s.synth.name, s.sh_type, s.flags, keywords).is_relro());

    let (ehdr, phent, shent, hdr_align) = header_sizes(target.class);
    let mut chunks: Vec<Chunk> = Vec::new();
    let mut ranks: Vec<Rank> = Vec::new();
    let mut out_of: Vec<Option<OutSecId>> = Vec::new();
    // Sort key per chunk: (rank, group, synthetic index, sub).
    let mut keys: Vec<(Rank, u32, u32, u32)> = Vec::new();
    let blank = |kind, sh_type, flags, align, size| Chunk {
        kind,
        sh_type,
        flags,
        align,
        size,
        capacity: size,
        addr: None,
        offset: 0,
        shndx: None,
        relro: false,
    };
    let push = |chunks: &mut Vec<Chunk>,
                ranks: &mut Vec<Rank>,
                keys: &mut Vec<(Rank, u32, u32, u32)>,
                out_of: &mut Vec<Option<OutSecId>>,
                c: Chunk,
                r: Rank,
                key: (u32, u32, u32),
                o: Option<OutSecId>| {
        chunks.push(c);
        ranks.push(r);
        keys.push((r, key.0, key.1, key.2));
        out_of.push(o);
        ChunkId(chunks.len() as u32 - 1)
    };
    push(
        &mut chunks,
        &mut ranks,
        &mut keys,
        &mut out_of,
        blank(ChunkKind::FileHeader, SHT_PROGBITS, SHF_ALLOC, hdr_align, ehdr),
        Rank::HEADERS,
        (0, 0, 0),
        None,
    );
    let ph = push(
        &mut chunks,
        &mut ranks,
        &mut keys,
        &mut out_of,
        blank(ChunkKind::ProgramHeaders, SHT_PROGBITS, SHF_ALLOC, hdr_align, 0),
        Rank::HEADERS,
        (0, 0, 1),
        None,
    );
    let sh = push(
        &mut chunks,
        &mut ranks,
        &mut keys,
        &mut out_of,
        blank(ChunkKind::SectionHeaders, SHT_PROGBITS, 0, hdr_align, 0),
        Rank::SECTION_HEADERS,
        (0, 0, 0),
        None,
    );
    // The copy chunks and their allocations (D-42): each chunk's size is
    // its allocations' extent.
    let mut sym_place = Vec::new();
    let copy_chunks: [(CopyPlace, &[u8]); 2] =
        [(CopyPlace::DynBss, b".dynbss"), (CopyPlace::RelRo, b".bss.rel.ro")];
    let mut copy_plans = Vec::new();
    for (place, name) in copy_chunks {
        let (rows, size, align) = copy_extent(inputs.copies, place)?;
        let at = inputs.synthetic.iter().position(|s| s.synth.name == name);
        match at {
            None if rows.is_empty() => {}
            None => {
                return Err(internal(format!(
                    "copy allocations with no {} chunk",
                    String::from_utf8_lossy(name)
                )));
            }
            Some(i) if inputs.synthetic[i].size != size => {
                return Err(internal(format!(
                    "{} has size {:#x}, its copy allocations {size:#x}",
                    String::from_utf8_lossy(name),
                    inputs.synthetic[i].size
                )));
            }
            Some(i) => copy_plans.push((i, rows, align)),
        }
    }
    for (i, s) in inputs.synthetic.iter().enumerate() {
        let r = match joins[i] {
            Some(o) => rank_of_out(o.0 as usize),
            None => rank_in(s.synth.name, s.sh_type, s.flags, keywords),
        };
        let group = joins[i].map_or(u32::MAX, |o| o.0);
        // A joining copy chunk leads its section (`rank::leads`); every
        // other one follows the section's chunks.
        let key = if joins[i].is_some() && leads(s.synth.name) {
            (group, 0, 0)
        } else {
            (group, i as u32, u32::MAX - 1)
        };
        let mut align = s.align;
        let plan = copy_plans.iter().find(|p| p.0 == i);
        if let Some((_, _, a)) = plan {
            align = align.max(*a);
        }
        let mut c = blank(ChunkKind::Synthetic(s.synth), s.sh_type, s.flags, align, s.size);
        c.relro = inputs.relro && r.is_relro();
        let id = push(&mut chunks, &mut ranks, &mut keys, &mut out_of, c, r, key, joins[i]);
        if let Some((_, rows, _)) = plan {
            for &(row, offset) in rows {
                sym_place.push(SymPlace {
                    key: AllocKey::Copy(row.index),
                    chunk: id,
                    offset,
                    size: row.size,
                    align: row.align,
                });
            }
        }
    }
    let relro_padding = (inputs.relro && relro_nonempty).then(|| {
        let mut c = blank(
            ChunkKind::Synthetic(Synth::RELRO_PADDING),
            SHT_NOBITS,
            SHF_ALLOC | SHF_WRITE,
            0,
            0,
        );
        c.relro = true;
        push(
            &mut chunks,
            &mut ranks,
            &mut keys,
            &mut out_of,
            c,
            Rank::RELRO_PADDING,
            (u32::MAX, u32::MAX, 0),
            None,
        )
    });
    // Per output section: bodies, islands, merged.
    let mut islands = Vec::new();
    let mut place_rows: Vec<(SectionId, ChunkId, u64)> = Vec::new();
    let mut holes: Vec<Hole> = Vec::new();
    for o in 0..outs {
        if !present[o] {
            continue;
        }
        let id = OutSecId(o as u32);
        let out = &a.outsecs[id];
        let r = rank_of_out(o);
        let exec = out.flags & SHF_EXECINSTR != 0;
        let with_islands = exec && target.islands.is_some();
        let n_parts = plans[o].len();
        // From 1: a leading copy chunk is (o, 0, 0).
        let mut sub = 1u32;
        let has_body = !inputs.ordered.members[id].is_empty()
            || has_commons(id)
            || (merged[o].is_none() && !has_synth[o]);
        for (k, plan) in plans[o].iter().enumerate() {
            if !has_body {
                break;
            }
            let mut size = plan.size;
            let mut align = plan.align;
            if with_islands {
                // The island after the body is aligned; the output
                // section's `sh_addralign` is the largest of its chunks',
                // so its first chunk must be aligned to it too (gABI
                // "Sections": `sh_addr` is congruent to 0 modulo it).
                align = align.max(target.islands.map_or(2, |i| i.align));
            }
            let body = ChunkId(chunks.len() as u32);
            if (Some(id) == bss || Some(id) == lbss) && k + 1 == n_parts {
                // L-88: commons after the .bss (.lbss) inputs, in SymbolId
                // order.
                for c in commons_of(id) {
                    let at = size
                        .checked_next_multiple_of(1u64 << c.align.min(63))
                        .ok_or_else(|| internal("common offsets overflow"))?;
                    sym_place.push(SymPlace {
                        key: AllocKey::Common(c.symbol),
                        chunk: body,
                        offset: at,
                        size: c.size,
                        align: c.align,
                    });
                    size = at.checked_add(c.size).ok_or_else(|| internal("commons overflow"))?;
                    align = align.max(c.align);
                }
            }
            let part = Part { out: id, part: k as u32 };
            let mut c = blank(ChunkKind::Body(part), out.sh_type, out.flags, align, size);
            c.relro = inputs.relro && r.is_relro();
            push(&mut chunks, &mut ranks, &mut keys, &mut out_of, c, r, (id.0, 0, sub), Some(id));
            sub += 1;
            for &(s, off) in &plan.members {
                place_rows.push((s, body, off));
            }
            for &(offset, size, kind) in &plan.holes {
                holes.push(Hole { chunk: body, offset, size, kind });
            }
            if with_islands {
                let isl = target.islands.map_or(2, |i| i.align);
                let mut c = blank(ChunkKind::Patches(part), SHT_PROGBITS, out.flags, isl, 0);
                c.relro = inputs.relro && r.is_relro();
                let i = push(
                    &mut chunks,
                    &mut ranks,
                    &mut keys,
                    &mut out_of,
                    c,
                    r,
                    (id.0, 0, sub),
                    Some(id),
                );
                sub += 1;
                islands.push(i);
            }
        }
        if let Some((size, align)) = merged[o] {
            let mut c = blank(ChunkKind::Merged(id), out.sh_type, out.flags, align, size);
            c.relro = inputs.relro && r.is_relro();
            push(&mut chunks, &mut ranks, &mut keys, &mut out_of, c, r, (id.0, 0, sub), Some(id));
        }
    }
    // Order (L-60): by (rank, within, OutSecId), synthetic chunks by their
    // index, chunks of one output section in creation order.
    let mut order: Vec<ChunkId> = (0..chunks.len() as u32).map(ChunkId).collect();
    order.sort_by_key(|c| keys[c.0 as usize]);
    // Section indices, densely in order (L-86), header chunks aside.
    let mut next = 0u32;
    let mut last_group: Option<(u32, u32)> = None;
    for &c in &order {
        let chunk = &mut chunks[c.0 as usize];
        if matches!(
            chunk.kind,
            ChunkKind::FileHeader | ChunkKind::ProgramHeaders | ChunkKind::SectionHeaders
        ) {
            continue;
        }
        let k = keys[c.0 as usize];
        let group = if k.1 == u32::MAX { (u32::MAX, c.0) } else { (k.1, 0) };
        if last_group != Some(group) {
            next += 1;
            last_group = Some(group);
        }
        chunk.shndx = Some(next);
    }
    // The section header table: the null entry and one per index.
    chunks[sh.0 as usize].size = u64::from(next + 1) * shent;
    chunks[sh.0 as usize].capacity = chunks[sh.0 as usize].size;
    // Alloc chunks get a placeholder address; the program header table's
    // size is the number of derived segments, which depends only on which
    // chunks exist (L-66).
    for c in &mut chunks {
        if c.flags & SHF_ALLOC != 0 {
            c.addr = Some(0);
        }
    }
    let chunks = Column::from_vec(chunks).map_err(|_| internal("too many chunks"))?;
    let count = derive_segments(&chunks, &order, rules).len() as u64;
    let mut chunks = chunks;
    chunks[ph].size = count * phent;
    chunks[ph].capacity = chunks[ph].size;
    // L-159: the members of an output section with no chunk, at the end
    // of the last chunk before it in order of its rank and load class
    // (skipping islands and the RELRO padding, whose sizes come later), or
    // else at the start of the first chunk after it.
    for o in (0..outs).filter(|&o| empty_only[o]) {
        let id = OutSecId(o as u32);
        let out = &a.outsecs[id];
        let r = rank_of_out(o);
        let class = blank(ChunkKind::Merged(id), out.sh_type, out.flags, 0, 0).load_class();
        let pos = order.partition_point(|c| keys[c.0 as usize] < (r, id.0, 0, 0));
        let same =
            |c: &ChunkId| ranks[c.0 as usize].rank == r.rank && chunks[*c].load_class() == class;
        let sized = |c: &ChunkId| {
            let ch = &chunks[*c];
            !matches!(ch.kind, ChunkKind::Patches(_)) && !ch.is_relro_padding()
        };
        let before = order[..pos].iter().rev().take_while(|c| same(c)).find(|c| sized(c));
        let at = match before {
            Some(&c) => Some((c, chunks[c].size)),
            None => order[pos..]
                .iter()
                .find(|c| same(c))
                .or_else(|| order[pos..].iter().find(|c| chunks[**c].is_alloc() && sized(c)))
                .map(|&c| (c, 0))
                .or_else(|| {
                    let c =
                        *order[..pos].iter().rev().find(|c| chunks[**c].is_alloc() && sized(c))?;
                    Some((c, chunks[c].size))
                }),
        };
        if let Some((c, off)) = at {
            for &s in &inputs.ordered.members[id] {
                place_rows.push((s, c, off));
            }
        }
    }
    // Places (rule 1): members, then folded sections at their target's.
    let mut place = Column::filled(inputs.headers.len(), Place::NONE);
    for (s, c, off) in place_rows {
        place[s] = Place::at(c, off);
    }
    if let Some(folded) = &inputs.folds.folded {
        for (s, t) in folded.iter() {
            if let Some(t) = t.get() {
                place[s] = place[t];
            }
        }
    }
    // The first chunk of an output section (a leading copy chunk, or the
    // body before the merged chunk of `.rodata` whose `.rodata.cst16`
    // pieces are wider than its members) is aligned to the largest
    // alignment of the section's chunks: `sh_addralign` is that largest
    // one, and gABI "Sections" wants `sh_addr` congruent to 0 modulo it
    // (L-5, as the islands above). A section whose first chunk is
    // already the widest is unchanged.
    let mut widest: Vec<u8> = vec![0; outs];
    for (c, o) in out_of.iter().enumerate() {
        if let Some(o) = o {
            let w = &mut widest[o.0 as usize];
            *w = (*w).max(chunks[ChunkId(c as u32)].align);
        }
    }
    let mut first_seen: Vec<bool> = vec![false; outs];
    for &c in &order {
        let Some(o) = out_of[c.0 as usize] else { continue };
        if std::mem::replace(&mut first_seen[o.0 as usize], true) {
            continue;
        }
        chunks[c].align = chunks[c].align.max(widest[o.0 as usize]);
    }
    // spec/incr.md IN-34a: the holes the caller lays out in synthetic
    // chunks.
    for h in inputs.synth_holes {
        let Some(c) = order.iter().copied().find(
            |&c| matches!(chunks[c].kind, ChunkKind::Synthetic(s) if s == h.synth),
        ) else {
            return Err(internal("a hole in a synthetic chunk that does not exist"));
        };
        holes.push(Hole { chunk: c, offset: h.offset, size: h.size, kind: h.kind });
    }
    holes.sort_unstable();
    if slack.on {
        capacities(&mut chunks, &order, &out_of, a, slack);
    }
    // Rule 5: sorted by key (commons, then copies by index).
    sym_place.sort_by_key(|p: &SymPlace| p.key);
    Ok(Staged {
        chunks,
        order: order.into(),
        ranks: Column::from_vec(ranks).map_err(|_| internal("too many chunks"))?,
        place,
        sym_place: sym_place.into(),
        islands: islands.into(),
        relro_padding,
        out_of: Column::from_vec(out_of).map_err(|_| internal("too many chunks"))?,
        holes: holes.into(),
    })
}

/// `spec/incr.md` IN-33's capacities past `size` (layout.md L-141): per
/// output section (or synthetic chunk standing alone) the last chunk in
/// order takes the section's tail, packed capacity or debug tail. Stage D
/// takes it back from the last chunk of each `PT_LOAD` (L-9) and adds the
/// segment reserves.
fn capacities(
    chunks: &mut Column<ChunkId, Chunk>,
    order: &[ChunkId],
    out_of: &[Option<OutSecId>],
    a: &Assignment<'_>,
    slack: Slack,
) {
    let mut k = 0;
    while k < order.len() {
        let c = order[k];
        let group = out_of[c.0 as usize];
        let mut end = k + 1;
        if group.is_some() {
            while end < order.len() && out_of[order[end].0 as usize] == group {
                end += 1;
            }
        }
        let last = order[end - 1];
        let size: u64 = order[k..end].iter().map(|&c| chunks[c].size).sum();
        let ch = &chunks[last];
        let name: Option<&[u8]> = match (group, ch.kind) {
            (Some(o), _) => Some(a.outsecs[o].name),
            (None, ChunkKind::Synthetic(s)) if !ch.is_relro_padding() => Some(s.name),
            _ => None,
        };
        k = end;
        let Some(name) = name else { continue };
        let extra = if ch.flags & SHF_ALLOC != 0 {
            match slack::class(name, ch.flags) {
                slack::Class::Tolerant => slack.tail(size),
                slack::Class::Packed => slack.packed(size),
                slack::Class::None => 0,
            }
        } else if slack::is_debug(name, ch.flags) {
            slack.debug_tail(size)
        } else {
            0
        };
        let ch = &mut chunks[last];
        ch.capacity = ch.capacity.saturating_add(extra);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::headers::Header;

    fn headers(sizes: &[(u64, u8)]) -> Headers<'static> {
        Column::from_vec(
            sizes
                .iter()
                .map(|&(size, align)| Header {
                    name: b".text",
                    sh_type: SHT_PROGBITS,
                    flags: SHF_ALLOC | SHF_EXECINSTR,
                    size,
                    align,
                    link: None,
                    entsize: 0,
                })
                .collect(),
        )
        .unwrap()
    }

    // spec: layout.md §7 L-22 (offsets are the aligned prefix sum)
    #[test]
    fn l22_aligned_prefix_sum() {
        let h = headers(&[(3, 0), (8, 3), (1, 0), (4, 4)]);
        let m: Vec<SectionId> = (0..4).map(SectionId).collect();
        let parts = plan_parts(&m, &h, None).unwrap();
        assert_eq!(parts.len(), 1);
        let offs: Vec<u64> = parts[0].members.iter().map(|x| x.1).collect();
        assert_eq!(offs, [0, 8, 16, 32]);
        assert_eq!((parts[0].size, parts[0].align), (36, 4));
    }

    // spec: layout.md §14.1 L-110 (parts cut before the section that passes S; larger than S refused)
    #[test]
    fn l110_parts() {
        let h = headers(&[(60, 2), (40, 2), (8, 2), (100, 2)]);
        let m: Vec<SectionId> = (0..4).map(SectionId).collect();
        let parts = plan_parts(&m, &h, Some(100)).unwrap();
        let shape: Vec<Vec<(u32, u64)>> =
            parts.iter().map(|p| p.members.iter().map(|x| (x.0.0, x.1)).collect()).collect();
        assert_eq!(shape, [vec![(0, 0), (1, 60)], vec![(2, 0)], vec![(3, 0)]]);
        let big = headers(&[(101, 2)]);
        let e = plan_parts(&[SectionId(0)], &big, Some(100)).unwrap_err();
        assert_eq!(e.refusal().map(|r| r.1), Some("code section larger than branch reach"));
    }
}
