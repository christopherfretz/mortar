//! The fill plan from the model's tables (I-27, I-38, I-41).
//!
//! `Placement` has no member list per `Body` chunk, so `place` is
//! inverted: every live, kept, placed, unfolded section with a place,
//! sorted by `(chunk, offset)`. Merged chunks get one row per run of
//! fragments contiguous both in the chunk and in one input section.
//! Synthetic, patch and structural chunks are the caller's (§13, I-40):
//! it passes their bytes or their chunk index.

use std::collections::BTreeMap;

use mortar_arch::{ProofRun, Reloc};
use mortar_core::assignment::{Assignment, OutOf};
use mortar_core::inputs::Reader;
use mortar_core::layout::ChunkKind;
use mortar_core::par::Pool;
use mortar_core::resolution::SectionState;
use mortar_core::{ChunkId, FileId, Inputs, Layout, Liveness, Resolution, SectionId, Split, SymKey, SymEntry};
use mortar_diag::{Diagnostic, FileRef, Kind};
use mortar_elf::{SectionIndex, SymbolIndex};

use crate::plan::{Builder, InputUnit, Pattern, Plan, RecOrder, RelocSection, SynthUnit};
use crate::target::Target;

/// `SHF_ALLOC`, `SHF_EXECINSTR`, `SHT_NOBITS`, `PT_LOAD`.
const SHF_ALLOC: u64 = 0x2;
const SHF_EXECINSTR: u64 = 0x4;
const SHT_NOBITS: u32 = 8;
const SHT_RELA: u32 = 4;
const PT_LOAD: u32 = 1;

/// The tables a plan is built from.
#[derive(Debug, Clone, Copy)]
pub struct Model<'m, 's> {
    pub inputs: &'m Inputs<'s>,
    pub resolution: &'m Resolution,
    pub liveness: &'m Liveness,
    pub split: &'m Split,
    pub layout: &'m Layout,
    pub assignment: &'m Assignment<'m>,
}

/// What the image does not own: the bytes of fixed chunks (the structural
/// units until their builder lands, I-40), the synthetic chunks' indexes
/// (§13), the proof of code bytes (`spec/arch.md` §3.6 rule 1), and
/// whether `-z separate-code` is on (I-41).
pub struct Extras<'e> {
    pub fixed: BTreeMap<ChunkId, Box<[u8]>>,
    /// `ChunkId` → (index in `chunks`, `Division::Whole`).
    pub synth: BTreeMap<ChunkId, (u32, bool)>,
    /// The proof runs of input section `(file, index)` of `len` bytes, in
    /// section offsets.
    pub proof: &'e dyn Fn(FileId, SectionIndex, u64) -> Vec<ProofRun>,
    pub separate_code: bool,
    /// The build id's descriptor in the file.
    pub sealed: Option<std::ops::Range<u64>>,
}

impl std::fmt::Debug for Extras<'_> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Extras").field("fixed", &self.fixed.len()).field("synth", &self.synth).finish_non_exhaustive()
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
/// `SymbolId` of `sym_map`.
pub fn records(inputs: &Inputs<'_>, file: FileId, rel: SectionIndex) -> Result<Vec<Reloc>, Diagnostic> {
    let Reader::Object(obj) = &inputs.files[file].reader else {
        return Err(internal(&format!("file {} has relocations and no object", file.0)));
    };
    let bad = |e: mortar_elf::Error| internal(&format!("file {}: relocation section {}: {e:?}", file.0, rel.0));
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
        for r in sec.rela().map_err(bad)? {
            out.push(Reloc { r_offset: r.r_offset, key: key(r.r_sym)?, r_type: r.r_type, addend: r.r_addend });
        }
    } else {
        // REL: the addend is in the bytes; M3 (x86).
        return Err(Diagnostic::refused(mortar_diag::RefusalClass::Feature, "REL relocations in fill (M3)"));
    }
    Ok(out)
}

/// Builds the plan of `m`'s placement (I-27).
pub fn from_layout(
    m: &Model<'_, '_>,
    target: Target,
    extras: &Extras<'_>,
    window: u64,
    pool: &Pool,
) -> Result<Plan, Diagnostic> {
    let pl = &m.layout.placement;
    let chunks = &pl.chunks;
    let file_size = chunks.iter().filter(|(_, c)| c.in_file()).map(|(_, c)| c.offset + c.size).max().unwrap_or(0);
    let mut b = Builder::new(target, file_size);
    // Placed input sections, in (chunk, offset) order.
    let mut members: Vec<(ChunkId, u64, SectionId)> = Vec::new();
    for (s, place) in pl.place.iter() {
        let Some((c, off)) = place.get() else { continue };
        let row = &m.resolution.sections[s];
        if !matches!(row.state, SectionState::Kept) || !row.kind.is_placed() || !m.liveness.is_live(s) {
            continue;
        }
        if m.layout.folds.is_folded(s) {
            continue;
        }
        members.push((c, off, s));
    }
    pool.sort_by_key(&mut members, |&(c, off, _)| (c.0, off));
    let mut paths: BTreeMap<FileId, FileRef> = BTreeMap::new();
    for &(c, off, s) in &members {
        let chunk = &chunks[c];
        if !chunk.in_file() || !matches!(chunk.kind, ChunkKind::Body(_)) {
            continue;
        }
        let (file, index) =
            m.resolution.sh_index(s).ok_or_else(|| internal(&format!("section {} has no file", s.0)))?;
        let Reader::Object(obj) = &m.inputs.files[file].reader else { continue };
        let sec = obj.section(index).map_err(|e| internal(&format!("{e:?}")))?;
        if sec.sh_type() == SHT_NOBITS || sec.size() == 0 {
            continue;
        }
        let Some(range) = sec.file_range().map_err(|e| internal(&format!("{e:?}")))? else { continue };
        paths.entry(file).or_insert_with(|| file_ref(m.inputs, file));
        let size = sec.size();
        let name = b.name(sec.name().unwrap_or(b""));
        let alloc = sec.flags() & SHF_ALLOC != 0;
        let rel = m.resolution.sections[s].reloc;
        let relocs = if rel.0 == 0 {
            None
        } else {
            let records = records(m.inputs, file, rel)?;
            let order = RecOrder::of(&records);
            let proof = (extras.proof)(file, index, size).into_boxed_slice();
            Some(b.relocs(RelocSection { records: records.into_boxed_slice(), order, proof }))
        };
        let addr = if alloc { chunk.addr.map_or(0, |a| a + off) } else { 0 };
        let unit = InputUnit {
            file,
            index: index.0,
            section: s,
            src: range.start as u64,
            start: 0,
            section_size: size,
            addr,
            alloc,
            name,
            relocs,
        };
        b.input(chunk.offset + off, size, unit);
    }
    merged(m, &mut b, &mut paths)?;
    for (&c, bytes) in &extras.fixed {
        b.bytes(chunks[c].offset, bytes.clone());
    }
    for (&c, &(chunk, whole)) in &extras.synth {
        let ch = &chunks[c];
        let name = b.name(ch.name(m.assignment).unwrap_or(b""));
        b.synth(ch.offset, ch.size, SynthUnit { chunk, id: c, name, whole });
    }
    pads(m, target, extras.separate_code, &mut b);
    for (f, p) in paths {
        b.path(f, p);
    }
    if let Some(s) = &extras.sealed {
        b.sealed(s.clone());
    }
    b.finish(window, pool)
}

/// I-38: the fragment runs of every `Merged` chunk.
fn merged(m: &Model<'_, '_>, b: &mut Builder, paths: &mut BTreeMap<FileId, FileRef>) -> Result<(), Diagnostic> {
    let pl = &m.layout.placement;
    let mut merged_chunk: BTreeMap<u32, ChunkId> = BTreeMap::new();
    for (c, ch) in pl.chunks.iter() {
        if let ChunkKind::Merged(o) = ch.kind {
            merged_chunk.insert(o.0, c);
        }
    }
    if merged_chunk.is_empty() {
        return Ok(());
    }
    // (chunk, chunk offset, section, section offset, size)
    let mut frags = Vec::new();
    for (f, frag) in m.split.frags.iter() {
        let Some(off) = m.layout.merge.offset(f) else { continue };
        let OutOf::Out(o) = m.assignment.out_of[frag.section] else { continue };
        let Some(&c) = merged_chunk.get(&o.0) else { continue };
        frags.push((c, off, frag.section, u64::from(frag.offset), u64::from(frag.size)));
    }
    frags.sort_unstable_by_key(|f| (f.0.0, f.1));
    let mut i = 0;
    while i < frags.len() {
        let (c, off, s, soff, mut size) = frags[i];
        let mut j = i + 1;
        while let Some(&(c2, off2, s2, soff2, size2)) = frags.get(j) {
            if c2 != c || s2 != s || off2 != off + size || soff2 != soff + size {
                break;
            }
            size += size2;
            j += 1;
        }
        i = j;
        if size == 0 {
            continue;
        }
        let ch = &pl.chunks[c];
        let (file, index) = m.resolution.sh_index(s).ok_or_else(|| internal(&format!("section {} has no file", s.0)))?;
        let Reader::Object(obj) = &m.inputs.files[file].reader else { continue };
        let sec = obj.section(index).map_err(|e| internal(&format!("{e:?}")))?;
        let Some(range) = sec.file_range().map_err(|e| internal(&format!("{e:?}")))? else { continue };
        if m.resolution.sections[s].reloc.0 != 0 {
            // S-6: a merge section has no relocations; layout refuses one.
            return Err(internal(&format!("merge section {} of file {} has relocations", index.0, file.0)));
        }
        paths.entry(file).or_insert_with(|| file_ref(m.inputs, file));
        let alloc = sec.flags() & SHF_ALLOC != 0;
        let name = b.name(sec.name().unwrap_or(b""));
        let unit = InputUnit {
            file,
            index: index.0,
            section: s,
            src: range.start as u64 + soff,
            start: soff,
            section_size: sec.size(),
            addr: if alloc { ch.addr.map_or(0, |a| a + off - soff) } else { 0 },
            alloc,
            name,
            relocs: None,
        };
        b.input(ch.offset + off, size, unit);
    }
    Ok(())
}

/// I-41 / L-82 / L-83: the pad ranges whose gaps are not zero.
fn pads(m: &Model<'_, '_>, target: Target, separate_code: bool, b: &mut Builder) {
    let pl = &m.layout.placement;
    let mut chunks: Vec<(u64, u64, bool, bool)> = pl
        .chunks
        .iter()
        .filter(|(_, c)| c.in_file() && c.size > 0)
        .map(|(_, c)| {
            let exec = c.flags & SHF_EXECINSTR != 0;
            let init = matches!(c.name(m.assignment), Some(b".init" | b".fini"));
            (c.offset, c.offset + c.size, exec, init)
        })
        .collect();
    chunks.sort_unstable_by_key(|c| c.0);
    let segs: Vec<(u64, u64, bool)> = pl
        .segments
        .iter()
        .filter(|(_, s)| s.p_type == PT_LOAD)
        .map(|(_, s)| (s.offset, s.offset + s.filesz, s.flags & 1 != 0))
        .collect();
    let seg_of = |off: u64| segs.iter().position(|s| s.0 <= off && off < s.1.max(s.0 + 1));
    for (k, &(lo, hi, exec, init)) in chunks.iter().enumerate() {
        if !exec {
            continue;
        }
        b.pad(lo..hi, if init { target.nop } else { target.trap });
        if let Some(&(nlo, _, nexec, _)) = chunks.get(k + 1)
            && nexec
            && hi < nlo
            && seg_of(lo).is_some()
            && seg_of(lo) == seg_of(nlo)
        {
            b.pad(hi..nlo, target.trap);
        }
    }
    if separate_code {
        let mut segs = segs.clone();
        segs.sort_unstable_by_key(|s| s.0);
        for w in segs.windows(2) {
            if w[0].2 && w[0].1 < w[1].0 {
                b.pad(w[0].1..w[1].0, target.trap);
            }
        }
    }
    let _ = Pattern::ZERO;
}
