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
use mortar_core::id::{ChunkId, OutSecId, SectionId, SymbolId};
use mortar_core::inputs::{Inputs, Reader, SymbolKey};
use mortar_core::layout::{
    AllocKey, Chunk, ChunkKind, Folds, MergeMap, Part, Place, SegmentRules, SymPlace, Synth,
    derive_segments,
};
use mortar_core::liveness::Liveness;
use mortar_core::par::Pool;
use mortar_core::resolution::{Def, InternalDef, Resolution};
use mortar_core::split::Split;
use mortar_elf::Class;
use mortar_elf::consts::{SHF_ALLOC, SHF_EXECINSTR, SHF_WRITE, SHT_NOBITS, SHT_PROGBITS, STT_TLS};

use crate::assign::{OutInfo, is_c_identifier};
use crate::diag::{LayoutDiag, feature, internal};
use crate::headers::Headers;
use crate::order::Ordered;
use crate::rank::{Rank, rank_of};
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

/// A common symbol resolution chose (L-88), with whether it is `STT_TLS`
/// (L-89).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Common {
    pub symbol: SymbolId,
    pub size: u64,
    /// log2.
    pub align: u8,
    pub tls: bool,
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
            Some(Common { symbol: s, size: shape.size, align: shape.align, tls })
        })
        .collect()
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
}

/// One body part: its members and their offsets, size and alignment.
#[derive(Debug, Clone, Default)]
struct PartPlan {
    members: Vec<(SectionId, u64)>,
    size: u64,
    align: u8,
}

/// The parts of one output section's body (L-22, L-110): the aligned
/// prefix sum, cut before the member that would make a part larger than
/// `limit`.
fn plan_parts(
    members: &[SectionId],
    headers: &Headers<'_>,
    limit: Option<u64>,
) -> Result<Vec<PartPlan>, LayoutDiag> {
    let mut parts = vec![PartPlan::default()];
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
        let a = 1u64 << h.align;
        let place = |p: &PartPlan| p.size.checked_next_multiple_of(a);
        let mut part = parts.last_mut().expect("one part at least");
        let mut at = place(part).ok_or_else(|| internal("section offsets overflow"))?;
        let mut end = at.checked_add(h.size).ok_or_else(|| internal("section offsets overflow"))?;
        if let Some(limit) = limit
            && end > limit
            && !part.members.is_empty()
        {
            parts.push(PartPlan::default());
            part = parts.last_mut().expect("just pushed");
            at = 0;
            end = h.size;
        }
        part.members.push((s, at));
        part.size = end;
        part.align = part.align.max(h.align);
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
/// `internal-error` for a size that overflows.
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
    let bss = a.outsecs.iter().find(|(_, o)| o.name == b".bss").map(|(id, _)| id);
    // Parts per output section, parallel over output sections (L-22).
    let planned: Vec<Result<Vec<PartPlan>, LayoutDiag>> = pool.map(outs, |o| {
        let id = OutSecId(o as u32);
        let out = &a.outsecs[id];
        let exec = out.flags & SHF_EXECINSTR != 0;
        let limit = target.islands.filter(|_| exec).map(|i| i.part_limit);
        plan_parts(&inputs.ordered.members[id], inputs.headers, limit)
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
    // Synthetic chunks named like an output section join it.
    let joins: Vec<Option<OutSecId>> = inputs
        .synthetic
        .iter()
        .map(|s| a.outsecs.iter().find(|(_, o)| o.name == s.synth.name).map(|(id, _)| id))
        .collect();
    let mut has_synth = vec![false; outs];
    for j in joins.iter().flatten() {
        has_synth[j.0 as usize] = true;
    }
    // Which output sections have chunks (L-43).
    let commons_in_bss = !inputs.commons.is_empty();
    let present: Vec<bool> = (0..outs)
        .map(|o| {
            let id = OutSecId(o as u32);
            !inputs.ordered.members[id].is_empty()
                || merged[o].is_some()
                || (commons_in_bss && Some(id) == bss)
                || inputs.keep_empty.contains(&id)
                || a.outsecs[id].keep
        })
        .collect();
    let rank_of_out = |o: usize| {
        let out = &a.outsecs[OutSecId(o as u32)];
        rank_of(out.name, out.sh_type, out.flags)
    };
    // RELRO padding exists when rank 7 is non-empty (L-64).
    let relro_nonempty = (0..outs).any(|o| present[o] && rank_of_out(o).is_relro())
        || inputs.synthetic.iter().any(|s| rank_of(s.synth.name, s.sh_type, s.flags).is_relro());

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
    for (i, s) in inputs.synthetic.iter().enumerate() {
        let r = match joins[i] {
            Some(o) => rank_of_out(o.0 as usize),
            None => rank_of(s.synth.name, s.sh_type, s.flags),
        };
        let group = joins[i].map_or(u32::MAX, |o| o.0);
        let mut c = blank(ChunkKind::Synthetic(s.synth), s.sh_type, s.flags, s.align, s.size);
        c.relro = r.is_relro();
        push(
            &mut chunks,
            &mut ranks,
            &mut keys,
            &mut out_of,
            c,
            r,
            (group, i as u32, u32::MAX - 1),
            joins[i],
        );
    }
    let relro_padding = relro_nonempty.then(|| {
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
    let mut sym_place = Vec::new();
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
        let mut sub = 0u32;
        let has_body = !inputs.ordered.members[id].is_empty()
            || (commons_in_bss && Some(id) == bss)
            || (merged[o].is_none() && !has_synth[o]);
        for (k, plan) in plans[o].iter().enumerate() {
            if !has_body {
                break;
            }
            let mut size = plan.size;
            let mut align = plan.align;
            let body = ChunkId(chunks.len() as u32);
            if Some(id) == bss && k + 1 == n_parts {
                // L-88: commons after the .bss inputs, in SymbolId order.
                for c in inputs.commons {
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
            c.relro = r.is_relro();
            push(&mut chunks, &mut ranks, &mut keys, &mut out_of, c, r, (id.0, 0, sub), Some(id));
            sub += 1;
            for &(s, off) in &plan.members {
                place_rows.push((s, body, off));
            }
            if with_islands {
                let isl = target.islands.map_or(2, |i| i.align);
                let mut c = blank(ChunkKind::Patches(part), SHT_PROGBITS, out.flags, isl, 0);
                c.relro = r.is_relro();
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
            c.relro = r.is_relro();
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
    Ok(Staged {
        chunks,
        order: order.into(),
        ranks: Column::from_vec(ranks).map_err(|_| internal("too many chunks"))?,
        place,
        sym_place: sym_place.into(),
        islands: islands.into(),
        relro_padding,
        out_of: Column::from_vec(out_of).map_err(|_| internal("too many chunks"))?,
    })
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
