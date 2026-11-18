//! The image of a `-r` link, RELA targets (`spec/image.md` §17).
//! Owner: **RL-4** (image at `-r`): I-R1 to I-R6, I-R8 (the build-id
//! note over the file with `--build-id`, none without), I-R11, I-R12;
//! `spec/arch.md` A-R1, A-R3, A-R4, A-R5; MD-R2's `.symtab`; I-R5's REL
//! call site, which calls [`crate::relrel::write`] (RL-9's) and so
//! refuses by name until RL-9 fills it. RL-4 also owns the `-r` blocks of
//! `mortar_synth/src/symtab.rs` and the `Relocatable` short cut of
//! `mortar_arch/src/{class.rs,need.rs}`. After RL-7: RL-8 (groups and
//! `.eh_frame`), RL-11 (`-s`/`-S`/`-x`), one after the other.
//!
//! WP-181 (RL-1) wrote this file with the signatures of [`write`] and
//! [`map_of`] and their refusal, the input [`RelocatableLink`], and the
//! map's `-r` records (`crate::map::RelocatableRecords`, with their
//! writer); RL-4 fills [`write`] and [`map_of`] here and may add private
//! items to this file.
//!
//! **How it is built (WP-184).** [`Frame`] reads the layout's chunks
//! (`RelocatableLayout`): the section header index of each chunk, the
//! pieces of each output section, the `.eh_frame` record map (the
//! supervisor's ruling S1: the kept records contiguous from offset 0 of
//! the synthetic `.eh_frame` chunk). [`Symbols`] is I-R4's `.symtab`:
//! the null entry, the section symbols, the kept locals per live file,
//! the globals of RV-R6 in `SymbolId` order, and the renumbering maps the
//! relocation writer uses. [`sizes`] is the size of every chunk the image
//! fills, as the image computes it: [`write`] compares it with the
//! layout's size of each chunk and refuses a difference by name (the
//! safety net of the rulings), never writing short or long.
//!
//! The file is built in one buffer of the file's size: the chunks are
//! disjoint slices of it, filled in parallel; the build id is hashed over
//! the buffer (descriptor zero) and patched; the buffer is written with
//! positioned writes. Memory is the output's size (a bench follow-up: a
//! windowed fill like the final link's needs no new rule).
//!
//! The rulings of the RL-3/RL-4 cells are the supervisor's
//! (`briefs/rl-sizes.md`, recorded in `spec/landed/image/WP-184.md`).

use std::borrow::Cow;
use std::ops::Range;
use std::path::Path;

use mortar_arch::{Arch, Howto, RelForm, Site};
use mortar_core::assignment::OutOf;
use mortar_core::inputs::{CopyError, Reader};
use mortar_core::layout::{AllocKey, Chunk, ChunkKind};
use mortar_core::par::Pool;
use mortar_core::plan::Plan;
use mortar_core::resolution::{Def, DropReason, RefFlags, SectionKind, SectionState};
use mortar_core::{
    ChunkId, FileId, Inputs, Liveness, OutSecId, RelocatableLayout, Resolution, SectionId, Split,
    SymbolId,
};
use mortar_diag::{Diagnostic, Key, Kind, Place, RefusalClass};
use mortar_digest::TreeStyle;
use mortar_elf::consts::{
    ELFOSABI_GNU, ELFOSABI_NONE, ET_REL, SHF_COMPRESSED, SHF_LINK_ORDER, SHN_ABS, SHN_COMMON,
    SHN_LORESERVE, SHN_UNDEF, SHN_XINDEX, SHT_NOBITS, SHT_REL, SHT_RELA, STB_GLOBAL,
    STB_GNU_UNIQUE, STB_LOCAL, STB_WEAK, STO_AARCH64_VARIANT_PCS, STT_FILE, STT_GNU_IFUNC,
    STT_NOTYPE, STT_OBJECT, STT_SECTION,
};
use mortar_elf::{Class, Machine, Object, SectionIndex, Sym, SymbolIndex, SymbolSection};

use crate::PHASE_IMAGE;
use crate::fill::{Failed, Sink as _};
use crate::headers::{self, Ehdr, Shdr};
use crate::layout_plan::file_ref;
use crate::link::Written;
use crate::map::{
    DiscardWhy, DynRecords, Map, MapFile, OutputKind as MapKind, RCommon, RFrame, RGroup, RPlace,
    RelocatableRecords,
};
use crate::output::{Output, OutputOptions};

/// The tables a `-r` image is a function of (`spec/model.md` §4.6 MD-R1,
/// §5), with the input bytes. RL-7's driver builds it from RL-2's
/// resolution and RL-3's layout.
pub struct RelocatableLink<'a, 's> {
    pub arch: &'a dyn Arch,
    pub pool: &'a Pool,
    pub plan: &'a Plan,
    pub inputs: &'a Inputs<'s>,
    /// RL-2's (`mortar_resolve::relocatable::resolve`).
    pub resolution: &'a Resolution,
    /// Every section of a live file that is not dropped (MD-R1).
    pub liveness: &'a Liveness,
    /// `.eh_frame` only (L-R5 splits nothing else).
    pub split: &'a Split,
    /// RL-3's (`mortar_layout::relocatable::layout`).
    pub layout: &'a RelocatableLayout<'s>,
    /// `Store::recheck` of the blob holding a file, as in a final link's
    /// `Link::recheck` (input.md §11.5).
    pub recheck: &'a (dyn Fn(FileId) -> Result<(), CopyError> + Sync),
    /// The build id's tree (`crate::link::tree_style` of
    /// `plan.shaping.build_id`); `None` without `--build-id` (I-R8).
    pub build_id: Option<TreeStyle>,
    /// The command line's words, for the map's `arg` records.
    pub args: &'a [Vec<u8>],
    /// RV-R3's names with no key
    /// (`mortar_resolve::relocatable::undefined_without_key`), written
    /// after the keys' entries (I-R4; WP-187).
    pub undefined: &'a [Box<[u8]>],
}

impl std::fmt::Debug for RelocatableLink<'_, '_> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("RelocatableLink").finish_non_exhaustive()
    }
}

// ---- names and small helpers ---------------------------------------------

/// The synthetic chunks of a `-r` layout, by name (ruling S9, S10, S11).
const SYMTAB: &[u8] = b".symtab";
const SYMTAB_SHNDX: &[u8] = b".symtab_shndx";
const STRTAB: &[u8] = b".strtab";
const SHSTRTAB: &[u8] = b".shstrtab";
const EH_FRAME: &[u8] = b".eh_frame";
const BUILD_ID: &[u8] = b".note.gnu.build-id";
const PROPERTY: &[u8] = b".note.gnu.property";
/// The note I-R4 keeps no local of.
const STACK_NOTE: &[u8] = b".note.GNU-stack";

/// `spec/synth.md` S-86's fixed table after the zero byte, and the
/// offsets of its names: the table `mortar_synth` writes
/// (`symtab::fixed_table`).
fn fixed_name(name: &[u8]) -> Option<u32> {
    match name {
        b"" => Some(0),
        b"$x" => Some(1),
        b"$d" => Some(4),
        mortar_synth::symtab::ERRATUM_843419 => Some(7),
        _ => None,
    }
}

fn internal(what: impl Into<String>) -> Diagnostic {
    Diagnostic::error(Kind::InternalError { what: what.into() })
}

fn key0(seq: u64) -> Key {
    Key::new(PHASE_IMAGE, 0, seq)
}

fn one(key: Key, d: Diagnostic) -> Failed {
    Failed { diagnostics: vec![(key, d)] }
}

/// The `-r` refusal of a feature a later package lifts.
fn later(name: &str, package: &str) -> Diagnostic {
    Diagnostic::refused(RefusalClass::Feature, name).note(format!("{package} lifts this at -r"))
}

/// The class-dependent sizes of the output.
#[derive(Debug, Clone, Copy)]
struct Shape {
    class: Class,
    /// `.symtab` entry size.
    sym: u64,
    /// The relocation record size of the target's form (A-R4).
    rel: u64,
    rela: bool,
}

impl Shape {
    fn of(arch: &dyn Arch) -> Self {
        let f = arch.facts();
        let is64 = f.class == Class::Elf64;
        let rela = f.rel_form == RelForm::Rela;
        let rel = match (is64, rela) {
            (true, true) => 24,
            (true, false) => 16,
            (false, true) => 12,
            (false, false) => 8,
        };
        Self { class: f.class, sym: if is64 { 24 } else { 16 }, rel, rela }
    }

    fn is64(self) -> bool {
        self.class == Class::Elf64
    }
}

fn object<'s>(inputs: &Inputs<'s>, file: FileId) -> Option<Object<'s>> {
    match inputs.files.get(file)?.reader {
        Reader::Object(o) => Some(o),
        _ => None,
    }
}

fn live_object(link: &RelocatableLink<'_, '_>, file: FileId) -> bool {
    file != FileId::INTERNAL
        && link.resolution.file_live.get(file).is_some_and(|l| l.is_live())
        && object(link.inputs, file).is_some()
}

/// The input header of section `s`.
fn input_shdr(
    link: &RelocatableLink<'_, '_>,
    s: SectionId,
) -> Result<mortar_elf::Shdr, Diagnostic> {
    let (file, index) = link
        .resolution
        .sh_index(s)
        .ok_or_else(|| internal(format!("section {} has no file", s.0)))?;
    let o = object(link.inputs, file)
        .ok_or_else(|| internal(format!("section {} is not an object's", s.0)))?;
    o.section(index)
        .map(|sec| *sec.header())
        .map_err(|e| internal(format!("file {} section {}: {e:?}", file.0, index.0)))
}

fn input_name(link: &RelocatableLink<'_, '_>, s: SectionId) -> Vec<u8> {
    link.resolution
        .sh_index(s)
        .and_then(|(file, index)| object(link.inputs, file)?.section(index).ok()?.name().ok())
        .map_or_else(Vec::new, <[u8]>::to_vec)
}

/// The bytes `range` of `file`: resident, or copied.
fn input_bytes<'s>(
    link: &RelocatableLink<'_, 's>,
    file: FileId,
    range: Range<u64>,
) -> Result<Cow<'s, [u8]>, Diagnostic> {
    if range.is_empty() {
        return Ok(Cow::Borrowed(&[]));
    }
    if let Some(b) = link.inputs.resident(file, range.clone())
        && b.len() as u64 == range.end - range.start
    {
        return Ok(Cow::Borrowed(b));
    }
    let len = usize::try_from(range.end - range.start)
        .map_err(|_| internal("an input range larger than the address space"))?;
    let mut buf = vec![0; len];
    link.inputs
        .copy_to(file, range, &mut buf)
        .map_err(|e| crate::source::copy_error(e, &file_ref(link.inputs, file)))?;
    Ok(Cow::Owned(buf))
}

/// The file bytes of input section `s` (empty for `SHT_NOBITS`).
fn section_bytes<'s>(
    link: &RelocatableLink<'_, 's>,
    s: SectionId,
    h: &mortar_elf::Shdr,
) -> Result<Cow<'s, [u8]>, Diagnostic> {
    if h.sh_type == SHT_NOBITS {
        return Ok(Cow::Borrowed(&[]));
    }
    let (file, _) =
        link.resolution.sh_index(s).ok_or_else(|| internal("a section without file"))?;
    input_bytes(link, file, h.sh_offset..h.sh_offset.saturating_add(h.sh_size))
}

// ---- the frame: chunks, indexes, pieces, the .eh_frame record map --------

/// One unit of an input `.eh_frame`: a record (CIE, FDE) of `Split`, a
/// zero terminator, or bytes that are neither (kept as they are). The
/// units of a section tile it (ruling S12).
#[derive(Debug, Clone, Copy)]
struct EhRec {
    /// The unit's offset and size in its input section.
    input: u64,
    size: u64,
    /// For an FDE, the input offset of its CIE.
    cie: Option<u64>,
    /// Its output offset; `None` for a removed record.
    out: Option<u64>,
}

/// The units of `.eh_frame` input `s` (ruling S12): its `Split` records
/// (size 4 + the length word), with the bytes between and after them as
/// zero terminators (4 bytes each) or, when not one, a unit of their own.
/// `at` is the number of kept bytes before the section, advanced past it.
fn eh_records(
    link: &RelocatableLink<'_, '_>,
    s: SectionId,
    at: &mut u64,
) -> Result<Vec<EhRec>, Diagnostic> {
    let h = input_shdr(link, s)?;
    let size = h.sh_size;
    let bytes = section_bytes(link, s, &h)?;
    let (offs, pieces) = match link.split.eh_pieces(s) {
        Some(span) => (&link.split.eh_off[span.slots()], &link.split.eh_piece[span.slots()]),
        None => (&[][..], &[][..]),
    };
    let word = |pos: u64| -> Option<u32> {
        let p = usize::try_from(pos).ok()?;
        let w = bytes.get(p..p.checked_add(4)?)?;
        Some(u32::from_le_bytes([w[0], w[1], w[2], w[3]]))
    };
    let bad = |what: &str| internal(format!(".eh_frame section {}: {what}", s.0));
    let mut out = Vec::with_capacity(offs.len() + 1);
    let mut pos = 0u64;
    let mut k = 0usize;
    while pos < size {
        let next = offs.get(k).map_or(size, |&o| u64::from(o));
        let (len, cie, record) = if next == pos {
            let w = word(pos).ok_or_else(|| bad("a record past the section's end"))?;
            if w == u32::MAX {
                return Err(bad("a 64-bit record length"));
            }
            let piece = pieces[k];
            let cie = if piece.is_cie() {
                None
            } else {
                Some(u64::from(
                    *offs
                        .get(piece.cie as usize)
                        .ok_or_else(|| bad("an FDE whose CIE is not one"))?,
                ))
            };
            k += 1;
            (4 + u64::from(w), cie, true)
        } else if next < pos {
            return Err(bad("records that overlap"));
        } else if word(pos) == Some(0) && pos + 4 <= next {
            (4, None, false)
        } else {
            (next - pos, None, false)
        };
        if pos + len > size {
            return Err(bad("a record past the section's end"));
        }
        let removed = record && link.layout.eh_removed.binary_search(&(s, pos)).is_ok();
        out.push(EhRec { input: pos, size: len, cie, out: (!removed).then_some(*at) });
        if !removed {
            *at += len;
        }
        pos += len;
    }
    if k != offs.len() {
        return Err(bad("a record that does not start where the one before ends"));
    }
    Ok(out)
}

/// The layout's chunks as the image reads them.
struct Frame {
    shape: Shape,
    /// Section header count, the null header included.
    shnum: u32,
    /// Per header index, its chunk (index 0: none).
    by_index: Vec<Option<ChunkId>>,
    sh_table: ChunkId,
    symtab: (ChunkId, u32),
    strtab: (ChunkId, u32),
    shstrtab: (ChunkId, u32),
    shndx: Option<(ChunkId, u32)>,
    build_id: Option<ChunkId>,
    property: Option<ChunkId>,
    /// The `.eh_frame` chunk and its output section.
    eh: Option<(ChunkId, OutSecId)>,
    /// Per output section: its header index (0: none).
    out_index: Vec<u32>,
    /// Per output section: its `Relocs` chunk.
    relocs_of: Vec<Option<ChunkId>>,
    /// Per output section: its pieces `(offset, section)`, by offset.
    pieces: Vec<Vec<(u64, SectionId)>>,
    /// The `.eh_frame` records in output order (removed ones included).
    eh_recs: Vec<EhRec>,
    /// Per live `EhFrame` section in `(FileId, index)` order: its records.
    /// and the number of kept bytes before it.
    eh_sections: Vec<(SectionId, Range<usize>, u64)>,
    /// Per header index: the `STT_SECTION` symbol's index (0: none).
    secsym: Vec<u32>,
    /// The number of section symbols.
    secsyms: u32,
}

fn chunk<'l>(link: &'l RelocatableLink<'_, '_>, id: ChunkId) -> Result<&'l Chunk, Diagnostic> {
    link.layout
        .layout
        .placement
        .chunks
        .get(id)
        .ok_or_else(|| internal(format!("chunk {} is not in the -r layout", id.0)))
}

/// Whether input section `s` is a live `.eh_frame` input (S1).
fn live_eh(link: &RelocatableLink<'_, '_>, s: SectionId) -> bool {
    link.resolution.sections.get(s).is_some_and(|r| r.kind == SectionKind::EhFrame && r.is_kept())
        && link.liveness.is_live(s)
}

impl Frame {
    #[allow(clippy::too_many_lines)]
    fn new(link: &RelocatableLink<'_, '_>) -> Result<Self, Failed> {
        let shape = Shape::of(link.arch);
        let pl = &link.layout.layout.placement;
        let fail = |d: Diagnostic| one(key0(0), d);
        if pl.kind != mortar_core::layout::PlacementKind::Relocatable {
            return Err(fail(internal("the -r image was given a loadable placement")));
        }
        let outs = link.layout.assignment.outsecs.len() as usize;
        let mut by_index: Vec<Option<ChunkId>> = vec![None];
        let mut header = None;
        let mut sh_table = None;
        let mut named: Vec<(&'static [u8], ChunkId, u32)> = Vec::new();
        let mut out_index = vec![0u32; outs];
        let mut relocs_of = vec![None; outs];
        for (id, c) in pl.chunks.iter() {
            match c.kind {
                ChunkKind::FileHeader => header = Some(id),
                ChunkKind::SectionHeaders => sh_table = Some(id),
                _ => {}
            }
            let Some(ix) = c.shndx else {
                if !matches!(c.kind, ChunkKind::FileHeader | ChunkKind::SectionHeaders) {
                    return Err(fail(internal(format!("chunk {} has no section index", id.0))));
                }
                continue;
            };
            let ix = ix as usize;
            if ix == 0 {
                return Err(fail(internal(format!("chunk {} has section index 0", id.0))));
            }
            if by_index.len() <= ix {
                by_index.resize(ix + 1, None);
            }
            if by_index[ix].replace(id).is_some() {
                return Err(fail(internal(format!("two chunks have section index {ix}"))));
            }
            match c.kind {
                ChunkKind::Body(p) => {
                    if p.part != 0 {
                        return Err(fail(internal("a -r output section with a second part")));
                    }
                    let slot = out_index
                        .get_mut(p.out.0 as usize)
                        .ok_or_else(|| fail(internal("a body of an unknown output section")))?;
                    *slot = ix as u32;
                }
                ChunkKind::Relocs(o) => {
                    let slot = relocs_of.get_mut(o.0 as usize).ok_or_else(|| {
                        fail(internal("relocations of an unknown output section"))
                    })?;
                    if slot.replace(id).is_some() {
                        return Err(fail(internal("two relocation chunks for one output section")));
                    }
                }
                ChunkKind::Synthetic(s) => named.push((s.name, id, ix as u32)),
                ChunkKind::Group(_) => {}
                ChunkKind::FileHeader
                | ChunkKind::SectionHeaders
                | ChunkKind::ProgramHeaders
                | ChunkKind::Merged(_)
                | ChunkKind::Patches(_) => {
                    return Err(fail(internal(format!(
                        "chunk {} has a kind a -r layout has not ({:?})",
                        id.0, c.kind
                    ))));
                }
            }
        }
        if let Some(gap) = by_index.iter().skip(1).position(Option::is_none) {
            return Err(fail(internal(format!("no chunk has section index {}", gap + 1))));
        }
        let find = |name: &[u8]| named.iter().find(|n| n.0 == name).map(|n| (n.1, n.2));
        for (name, id, _) in &named {
            if ![SYMTAB, SYMTAB_SHNDX, STRTAB, SHSTRTAB, EH_FRAME, BUILD_ID, PROPERTY]
                .contains(name)
            {
                return Err(fail(internal(format!(
                    "chunk {} is a synthetic {} a -r layout has not",
                    id.0,
                    String::from_utf8_lossy(name)
                ))));
            }
            if named.iter().filter(|n| n.0 == *name).count() > 1 {
                return Err(fail(internal(format!(
                    "two synthetic {} chunks",
                    String::from_utf8_lossy(name)
                ))));
            }
        }
        let need = |name: &[u8]| {
            find(name).ok_or_else(|| {
                fail(internal(format!("the -r layout has no {}", String::from_utf8_lossy(name))))
            })
        };
        let symtab = need(SYMTAB)?;
        let strtab = need(STRTAB)?;
        let shstrtab = need(SHSTRTAB)?;
        // S1/S11: the `.eh_frame` chunk and the one output section of every
        // live `EhFrame` input.
        let mut eh_out: Option<OutSecId> = None;
        let mut eh_sections = Vec::new();
        let mut eh_recs = Vec::new();
        let mut at = 0u64;
        for (file, _) in link.inputs.files.iter() {
            if !live_object(link, file) {
                continue;
            }
            let range = link.resolution.section_base[file];
            for s in range.iter() {
                if !live_eh(link, s) {
                    continue;
                }
                let o = link.layout.assignment.out_of.get(s).copied().and_then(OutOf::out);
                let Some(o) = o else {
                    return Err(fail(internal(format!(
                        "the .eh_frame section {} has no output section",
                        s.0
                    ))));
                };
                if eh_out.is_some_and(|e| e != o) {
                    return Err(fail(internal(
                        "the live .eh_frame inputs are in two output sections (ruling S1)",
                    )));
                }
                eh_out = Some(o);
                let first = eh_recs.len();
                let start = at;
                eh_recs.extend(eh_records(link, s, &mut at).map_err(fail)?);
                eh_sections.push((s, first..eh_recs.len(), start));
            }
        }
        let eh_chunk = find(EH_FRAME);
        let eh = match (eh_chunk, eh_out) {
            (Some((c, ix)), Some(o)) => {
                let slot = out_index
                    .get_mut(o.0 as usize)
                    .ok_or_else(|| fail(internal(".eh_frame of an unknown output section")))?;
                if *slot != 0 {
                    return Err(fail(internal(".eh_frame's output section also has a body")));
                }
                *slot = ix;
                Some((c, o))
            }
            (None, None) => None,
            (Some(_), None) => {
                return Err(fail(internal("an .eh_frame chunk with no live .eh_frame input")));
            }
            (None, Some(_)) => {
                return Err(fail(internal("live .eh_frame inputs and no .eh_frame chunk")));
            }
        };
        // The pieces: every section with a place, in its body.
        let mut pieces: Vec<Vec<(u64, SectionId)>> = vec![Vec::new(); outs];
        for (s, place) in pl.place.iter() {
            let Some((c, off)) = place.get() else { continue };
            let ck = chunk(link, c).map_err(fail)?;
            let ChunkKind::Body(p) = ck.kind else {
                return Err(fail(internal(format!(
                    "section {} is placed in chunk {}, which is not a body",
                    s.0, c.0
                ))));
            };
            if link.layout.assignment.out_of.get(s).copied().and_then(OutOf::out) != Some(p.out) {
                return Err(fail(internal(format!(
                    "section {} is placed outside its output section",
                    s.0
                ))));
            }
            pieces[p.out.0 as usize].push((off, s));
        }
        for p in &mut pieces {
            p.sort_unstable();
        }
        // I-R4 item 2: a section symbol per output section but the groups,
        // the relocation sections and the four tables.
        let mut secsym = vec![0u32; by_index.len()];
        let mut secsyms = 0u32;
        for (ix, c) in by_index.iter().enumerate().skip(1) {
            let Some(c) = c else { continue };
            let k = chunk(link, *c).map_err(fail)?.kind;
            let has = match k {
                ChunkKind::Group(_) | ChunkKind::Relocs(_) => false,
                ChunkKind::Synthetic(s) => {
                    ![SYMTAB, SYMTAB_SHNDX, STRTAB, SHSTRTAB].contains(&s.name)
                }
                _ => true,
            };
            if has {
                secsyms += 1;
                secsym[ix] = secsyms;
            }
        }
        if header.is_none() {
            return Err(fail(internal("the -r layout has no file header")));
        }
        Ok(Self {
            shape,
            shnum: by_index.len() as u32,
            by_index,
            sh_table: sh_table
                .ok_or_else(|| fail(internal("the -r layout has no section header table")))?,
            symtab,
            strtab,
            shstrtab,
            shndx: find(SYMTAB_SHNDX),
            build_id: find(BUILD_ID).map(|f| f.0),
            property: find(PROPERTY).map(|f| f.0),
            eh,
            out_index,
            relocs_of,
            pieces,
            eh_recs,
            eh_sections,
            secsym,
            secsyms,
        })
    }

    /// The records of `.eh_frame` input `s`.
    fn eh_of(&self, s: SectionId) -> Option<&[EhRec]> {
        let at = self.eh_sections.binary_search_by_key(&s, |e| e.0).ok()?;
        self.eh_recs.get(self.eh_sections[at].1.clone())
    }

    /// L-R6's record map (ruling S12): the output offset of byte `off` of
    /// `.eh_frame` input `s`, the number of kept bytes before it; the end
    /// of the section maps to the number of kept bytes up to it. `None`
    /// in a removed record.
    fn eh_map(&self, s: SectionId, off: u64) -> Option<u64> {
        let at = self.eh_sections.binary_search_by_key(&s, |e| e.0).ok()?;
        let (_, range, start) = self.eh_sections[at].clone();
        let recs = &self.eh_recs[range];
        let i = recs.partition_point(|r| r.input <= off);
        if i == 0 {
            // An empty section: its start (= its end).
            return (off == 0).then_some(start);
        }
        let r = recs[i - 1];
        if off < r.input + r.size {
            return Some(r.out? + (off - r.input));
        }
        if off == r.input + r.size && i == recs.len() {
            return Some(
                recs.iter().rev().find_map(|r| r.out.map(|o| o + r.size)).unwrap_or(start),
            );
        }
        None
    }

    /// The header index of output section `o`.
    fn index_of(&self, o: OutSecId) -> Option<u32> {
        self.out_index.get(o.0 as usize).copied().filter(|&i| i != 0)
    }

    /// Where offset `off` of a placed input section `s` is: its header
    /// index and offset in it; `.eh_frame` inputs through the record map.
    fn where_is(
        &self,
        link: &RelocatableLink<'_, '_>,
        s: SectionId,
        off: u64,
    ) -> Option<(u32, u64)> {
        if live_eh(link, s) {
            let (_, o) = self.eh?;
            return Some((self.index_of(o)?, self.eh_map(s, off)?));
        }
        let (c, at) = link.layout.layout.placement.place.get(s)?.get()?;
        let shndx = link.layout.layout.placement.chunks.get(c)?.shndx?;
        Some((shndx, at.checked_add(off)?))
    }
}

// ---- I-R4: the symbols ------------------------------------------------------

/// A `.symtab` entry's name.
#[derive(Debug, Clone, Copy)]
enum Name<'s> {
    Plain(&'s [u8]),
    /// A key's name and version: `name@version`.
    Versioned(&'s [u8], &'s [u8]),
}

impl Name<'_> {
    fn len(self) -> u64 {
        match self {
            Self::Plain(n) => n.len() as u64,
            Self::Versioned(n, v) => n.len() as u64 + 1 + v.len() as u64,
        }
    }

    /// The bytes it takes in `.strtab` (S-86): none for a fixed name.
    fn region(self) -> u64 {
        match self {
            Self::Plain(n) if fixed_name(n).is_some() => 0,
            n => n.len() + 1,
        }
    }

    fn put(self, out: &mut [u8]) {
        match self {
            Self::Plain(n) => out[..n.len()].copy_from_slice(n),
            Self::Versioned(n, v) => {
                out[..n.len()].copy_from_slice(n);
                out[n.len()] = b'@';
                out[n.len() + 1..n.len() + 1 + v.len()].copy_from_slice(v);
            }
        }
    }
}

/// One entry of items 3 and 4.
#[derive(Debug, Clone, Copy)]
struct Entry<'s> {
    name: Name<'s>,
    info: u8,
    other: u8,
    /// The output section index, or `SHN_ABS`/`SHN_COMMON`/`SHN_UNDEF`
    /// (an index `>= SHN_LORESERVE` that is a section is marked by
    /// `section`).
    shndx: u32,
    /// `shndx` is an output section's index (not a reserved value).
    section: bool,
    value: u64,
    size: u64,
}

impl Entry<'_> {
    fn needs_xindex(&self) -> bool {
        self.section && self.shndx >= u32::from(SHN_LORESERVE)
    }
}

/// I-R4's `.symtab`.
struct Symbols<'s> {
    locals: Vec<Entry<'s>>,
    globals: Vec<Entry<'s>>,
    /// Per file (by `FileId`): output index of each input local entry
    /// (0: not kept); empty for a file that is not a live object.
    local_out: Vec<Vec<u32>>,
    /// Per `SymbolId`: its output index (0: none).
    global_out: Vec<u32>,
    /// `sh_info`: the index of the first global.
    first_global: u32,
    xindex: bool,
}

/// The local entries of `file` that I-R4 keeps, with their input index.
fn kept_locals<'s>(
    link: &RelocatableLink<'_, 's>,
    frame: &Frame,
    file: FileId,
) -> Result<Vec<(u32, Entry<'s>)>, Diagnostic> {
    let Some(o) = object(link.inputs, file) else { return Ok(Vec::new()) };
    let bad = |e: mortar_elf::Error| internal(format!("file {}: symbol table: {e:?}", file.0));
    let Some(t) = o.symbols().map_err(bad)? else { return Ok(Vec::new()) };
    let mut out = Vec::new();
    for (index, s) in t.locals() {
        if index.0 == 0 || s.sym_type() == STT_SECTION {
            continue;
        }
        let name = t.name(&s).map_err(bad)?;
        let place = |shndx: u32, value: u64| Entry {
            name: Name::Plain(name),
            info: s.st_info,
            other: s.st_other,
            shndx,
            section: true,
            value,
            size: s.st_size,
        };
        let kept = match t.section_of(index, &s).map_err(bad)? {
            SymbolSection::Absolute => {
                Some(Entry { shndx: u32::from(SHN_ABS), section: false, ..place(0, s.st_value) })
            }
            SymbolSection::Section(i) => {
                let sid = link.resolution.section_id(file, i);
                match sid {
                    // The type-unit signature (L-R3): value 0, the output
                    // group's index.
                    Some(g)
                        if link
                            .resolution
                            .sections
                            .get(g)
                            .is_some_and(|r| r.kind == SectionKind::Group) =>
                    {
                        group_index(link, frame, g).map(|ix| place(ix, 0))
                    }
                    // I-R4: not in a section the `-r` itself makes from
                    // its inputs (the merged `.note.GNU-stack`; the
                    // property note and the Mason container are not
                    // placed, so `where_is` has none).
                    Some(sid) if input_name(link, sid) == STACK_NOTE => None,
                    Some(sid) => frame.where_is(link, sid, s.st_value).map(|(ix, v)| place(ix, v)),
                    None => None,
                }
            }
            // `STT_FILE` is `SHN_ABS` by the gABI; a file entry written
            // otherwise is still kept (I-R4 item 3).
            _ if s.sym_type() == STT_FILE => {
                Some(Entry { shndx: u32::from(SHN_ABS), section: false, ..place(0, s.st_value) })
            }
            _ => None,
        };
        if let Some(e) = kept {
            out.push((index.0, e));
        }
    }
    Ok(out)
}

/// The header index of the output `SHT_GROUP` written from input group
/// section `g`.
fn group_index(link: &RelocatableLink<'_, '_>, frame: &Frame, g: SectionId) -> Option<u32> {
    frame.by_index.iter().enumerate().find_map(|(ix, c)| {
        let c = link.layout.layout.placement.chunks.get((*c)?)?;
        (c.kind == ChunkKind::Group(g)).then_some(ix as u32)
    })
}

/// The `STT_*` of an undefined key over its live mentions: their type
/// when they all agree, else `STT_NOTYPE` (RV-R3).
const UNSET: u8 = 0xff;

impl<'l> Symbols<'l> {
    #[allow(clippy::too_many_lines)]
    fn new(link: &'l RelocatableLink<'_, '_>, frame: &Frame) -> Result<Self, Failed> {
        let inputs = link.inputs;
        let files = inputs.files.len() as usize;
        // Item 3, per live object in parallel.
        let per_file = link.pool.map(files, |f| {
            let file = FileId(f as u32);
            if live_object(link, file) {
                kept_locals(link, frame, file).map(Some)
            } else {
                Ok(None)
            }
        });
        let mut locals = Vec::new();
        let mut local_out: Vec<Vec<u32>> = Vec::with_capacity(files);
        let mut next = 1 + frame.secsyms;
        let mut diagnostics = Vec::new();
        for (f, r) in per_file.into_iter().enumerate() {
            match r {
                Ok(Some(rows)) => {
                    let n = object(inputs, FileId(f as u32))
                        .and_then(|o| o.symbols().ok().flatten())
                        .map_or(0, |t| t.first_global().0 as usize);
                    let mut map = vec![0u32; n];
                    for (index, e) in rows {
                        map[index as usize] = next;
                        next += 1;
                        locals.push(e);
                    }
                    local_out.push(map);
                }
                Ok(None) => local_out.push(Vec::new()),
                Err(d) => {
                    diagnostics.push((Key::new(PHASE_IMAGE, f as u32, 0), d));
                    local_out.push(Vec::new());
                }
            }
        }
        if !diagnostics.is_empty() {
            return Err(Failed { diagnostics });
        }
        let first_global = next;
        // Item 4: which keys a live object mentions, and the type of an
        // undefined key's mentions.
        let keys = inputs.symbols.len() as usize;
        let mut mentioned = vec![false; keys];
        let mut utype = vec![UNSET; keys];
        for (file, _) in inputs.files.iter() {
            if !live_object(link, file) {
                continue;
            }
            let Some(t) = object(inputs, file).and_then(|o| o.symbols().ok().flatten()) else {
                continue;
            };
            let map = inputs.sym_map_of(file);
            for (index, s) in t.globals() {
                let Some(&id) = map.get((index.0 - t.first_global().0) as usize) else { continue };
                let k = id.0 as usize;
                mentioned[k] = true;
                if s.st_shndx == SHN_UNDEF {
                    utype[k] = match utype[k] {
                        UNSET => s.sym_type(),
                        x if x == s.sym_type() => x,
                        _ => STT_NOTYPE,
                    };
                }
            }
        }
        let mut globals = Vec::new();
        let mut global_out = vec![0u32; keys];
        let fail = |id: SymbolId, d: Diagnostic| one(Key::new(PHASE_IMAGE, 0, u64::from(id.0)), d);
        for (id, key) in inputs.symbols.iter() {
            let def =
                link.resolution.def.get(id).copied().unwrap_or(Def::Undefined { weak: false });
            let props = link.resolution.props.get(id).copied().unwrap_or_default();
            let k = id.0 as usize;
            let root = props.refs.contains(RefFlags::FROM_OBJECT);
            let written = match def {
                Def::Internal(_) | Def::Shared(_) => false,
                Def::Undefined { .. } => mentioned[k] || root,
                Def::Defined(_) | Def::Common(_) => mentioned[k],
            };
            if !written {
                continue;
            }
            let vis = props.visibility & 3;
            let entry = match def {
                Def::Defined(e) => {
                    let (s, t) = def_entry(inputs, e.file, e.index)
                        .ok_or_else(|| fail(id, internal("a definition's entry cannot be read")))?;
                    let name = t.name(&s).map_err(|_| fail(id, internal("a definition's name")))?;
                    let binding = match s.binding() {
                        STB_GNU_UNIQUE => STB_GNU_UNIQUE,
                        STB_WEAK => STB_WEAK,
                        _ => STB_GLOBAL,
                    };
                    let info = binding << 4 | s.sym_type();
                    let other = (s.st_other & !3) | vis;
                    let base = Entry {
                        name: Name::Plain(name),
                        info,
                        other,
                        shndx: 0,
                        section: false,
                        value: s.st_value,
                        size: s.st_size,
                    };
                    match t.section_of(e.index, &s) {
                        Ok(SymbolSection::Absolute) => Entry { shndx: u32::from(SHN_ABS), ..base },
                        Ok(SymbolSection::Section(i)) => {
                            let at = link
                                .resolution
                                .section_id(e.file, i)
                                .and_then(|sid| frame.where_is(link, sid, s.st_value));
                            let Some((ix, value)) = at else {
                                return Err(fail(
                                    id,
                                    internal(format!(
                                        "{} is defined in a section that is not in the -r output",
                                        String::from_utf8_lossy(name)
                                    )),
                                ));
                            };
                            Entry { shndx: ix, section: true, value, ..base }
                        }
                        _ => {
                            return Err(fail(id, internal("a definition in no section")));
                        }
                    }
                }
                Def::Common(e) => {
                    let (s, t) = def_entry(inputs, e.file, e.index)
                        .ok_or_else(|| fail(id, internal("a common's entry cannot be read")))?;
                    let shape = link
                        .resolution
                        .common_of(id)
                        .ok_or_else(|| fail(id, internal("a common key with no shape")))?;
                    let _ = t;
                    let other = (s.st_other & !3) | vis;
                    let name = key_name(key.name, key.version);
                    match link.layout.layout.placement.sym_place_of(AllocKey::Common(id)) {
                        // RV-R5 with `-d`: a definition in `.bss`.
                        Some(p) => {
                            let ix = chunk(link, p.chunk)
                                .ok()
                                .and_then(|c| c.shndx)
                                .ok_or_else(|| fail(id, internal("a common in no section")))?;
                            Entry {
                                name,
                                info: STB_GLOBAL << 4 | STT_OBJECT,
                                other,
                                shndx: ix,
                                section: true,
                                value: p.offset,
                                size: shape.size,
                            }
                        }
                        // RV-R5: `SHN_COMMON`, value the alignment.
                        None => Entry {
                            name,
                            info: s.st_info,
                            other,
                            shndx: u32::from(SHN_COMMON),
                            section: false,
                            value: 1u64 << shape.align,
                            size: shape.size,
                        },
                    }
                }
                Def::Undefined { weak } => {
                    let mut other = vis;
                    if inputs.target.machine == Machine::AArch64
                        && props.refs.contains(RefFlags::VARIANT_PCS)
                    {
                        other |= STO_AARCH64_VARIANT_PCS;
                    }
                    let typ = if utype[k] == UNSET { STT_NOTYPE } else { utype[k] };
                    let binding = if weak { STB_WEAK } else { STB_GLOBAL };
                    Entry {
                        name: key_name(key.name, key.version),
                        info: binding << 4 | typ,
                        other,
                        shndx: u32::from(SHN_UNDEF),
                        section: false,
                        value: 0,
                        size: 0,
                    }
                }
                Def::Internal(_) | Def::Shared(_) => unreachable!("not written"),
            };
            global_out[k] = first_global + globals.len() as u32;
            globals.push(entry);
        }
        // RV-R3: the `-u` (and `EXTERN`) names no input mentions, which
        // have no key: `STB_GLOBAL` `SHN_UNDEF` `STT_NOTYPE` after the keys,
        // in argument order (WP-182 §C; WP-187).
        for name in link.undefined {
            globals.push(Entry {
                name: Name::Plain(name),
                info: STB_GLOBAL << 4 | STT_NOTYPE,
                other: 0,
                shndx: u32::from(SHN_UNDEF),
                section: false,
                value: 0,
                size: 0,
            });
        }
        let xindex = frame
            .secsym
            .iter()
            .enumerate()
            .any(|(ix, &n)| n != 0 && ix >= usize::from(SHN_LORESERVE))
            || locals.iter().chain(&globals).any(Entry::needs_xindex);
        Ok(Self { locals, globals, local_out, global_out, first_global, xindex })
    }

    /// The number of entries.
    fn count(&self, frame: &Frame) -> u64 {
        1 + u64::from(frame.secsyms) + self.locals.len() as u64 + self.globals.len() as u64
    }

    fn strtab_size(&self) -> u64 {
        mortar_synth::symtab::fixed_table().len() as u64
            + self.locals.iter().chain(&self.globals).map(|e| e.name.region()).sum::<u64>()
    }
}

fn key_name<'s>(name: &'s [u8], version: Option<&'s [u8]>) -> Name<'s> {
    match version {
        Some(v) => Name::Versioned(name, v),
        None => Name::Plain(name),
    }
}

fn def_entry<'s>(
    inputs: &Inputs<'s>,
    file: FileId,
    index: SymbolIndex,
) -> Option<(Sym, mortar_elf::SymbolTable<'s>)> {
    let t = object(inputs, file)?.symbols().ok()??;
    Some((t.get(index).ok()?, t))
}

// ---- the sizes (the safety net) ---------------------------------------------

/// The size of every chunk a `-r` image fills, as the image computes it
/// from the tables (the rulings' safety net): RL-7's join can make one of
/// these and RL-3's sizing the single source.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Sizes {
    /// Per chunk, in `ChunkId` order: the size the image fills.
    pub chunks: Vec<(ChunkId, u64)>,
    /// Whether `.symtab_shndx` is needed (ruling S5).
    pub symtab_shndx: bool,
}

/// The sizes of every chunk of `link`'s layout, as the image computes
/// them (I-R4, I-R5, I-R6, I-R8; rulings S1-S10).
///
/// # Errors
///
/// A table the image cannot read, by name.
pub fn sizes(link: &RelocatableLink<'_, '_>) -> Result<Sizes, Failed> {
    let frame = Frame::new(link)?;
    let syms = Symbols::new(link, &frame)?;
    let groups = Groups::new(link, &frame)?;
    let property = property_plan(link)?;
    let build_id = build_id_plan(link)?;
    computed_sizes(link, &frame, &syms, &groups, property.as_ref(), build_id.as_ref())
}

fn computed_sizes(
    link: &RelocatableLink<'_, '_>,
    frame: &Frame,
    syms: &Symbols<'_>,
    groups: &Groups,
    property: Option<&mortar_synth::PropertyPlan>,
    build_id: Option<&mortar_synth::notes::BuildIdPlan>,
) -> Result<Sizes, Failed> {
    let pl = &link.layout.layout.placement;
    let shape = frame.shape;
    let (ehsize, _, shentsize) = headers::sizes(shape.class);
    let counts = reloc_counts(link, frame)?;
    let mut out = Vec::with_capacity(pl.chunks.len() as usize);
    for (id, c) in pl.chunks.iter() {
        let size = match c.kind {
            ChunkKind::FileHeader => ehsize,
            ChunkKind::SectionHeaders => u64::from(frame.shnum) * shentsize,
            ChunkKind::Body(p) => body_size(link, frame, p.out)?,
            ChunkKind::Relocs(o) => counts.get(o.0 as usize).copied().unwrap_or(0) * shape.rel,
            ChunkKind::Group(g) => groups.size_of(g),
            ChunkKind::Synthetic(s) => match s.name {
                SYMTAB => syms.count(frame) * shape.sym,
                SYMTAB_SHNDX => syms.count(frame) * 4,
                STRTAB => syms.strtab_size(),
                SHSTRTAB => shstrtab(link, frame).0.len() as u64,
                EH_FRAME => frame.eh_recs.iter().filter(|r| r.out.is_some()).map(|r| r.size).sum(),
                BUILD_ID => build_id.map_or(0, mortar_synth::notes::BuildIdPlan::size),
                PROPERTY => property.map_or(0, mortar_synth::PropertyPlan::size),
                _ => 0,
            },
            ChunkKind::ProgramHeaders | ChunkKind::Merged(_) | ChunkKind::Patches(_) => 0,
        };
        out.push((id, size));
    }
    Ok(Sizes { chunks: out, symtab_shndx: syms.xindex })
}

/// A body's size: the end of its last piece, or of its last `-d` common.
fn body_size(link: &RelocatableLink<'_, '_>, frame: &Frame, o: OutSecId) -> Result<u64, Failed> {
    let mut end = 0;
    for &(off, s) in frame.pieces.get(o.0 as usize).map_or(&[][..], Vec::as_slice) {
        let h = input_shdr(link, s).map_err(|d| one(key0(u64::from(s.0)), d))?;
        end = end.max(off + h.sh_size);
    }
    let ix = frame.index_of(o);
    for p in &link.layout.layout.placement.sym_place {
        if let AllocKey::Common(_) = p.key
            && link.layout.layout.placement.chunks.get(p.chunk).and_then(|c| c.shndx) == ix
        {
            end = end.max(p.offset + p.size);
        }
    }
    Ok(end)
}

/// Per output section: the number of output records of its relocation
/// section (I-R5, ruling S6; S1 for `.eh_frame`).
fn reloc_counts(link: &RelocatableLink<'_, '_>, frame: &Frame) -> Result<Vec<u64>, Failed> {
    let mut counts = vec![0u64; frame.pieces.len()];
    let fail = |s: SectionId, d| one(key0(u64::from(s.0)), d);
    for (o, pieces) in frame.pieces.iter().enumerate() {
        for &(_, s) in pieces {
            if let Some(h) = reloc_shdr(link, s).map_err(|d| fail(s, d))? {
                counts[o] += h.sh_size / record_size(frame.shape, h.sh_type);
            }
        }
    }
    if let Some((_, o)) = frame.eh {
        for (s, ..) in &frame.eh_sections {
            let Some(h) = reloc_shdr(link, *s).map_err(|d| fail(*s, d))? else { continue };
            if frame.eh_recs.iter().all(|r| r.out.is_some()) {
                counts[o.0 as usize] += h.sh_size / record_size(frame.shape, h.sh_type);
            } else {
                let (file, _) =
                    link.resolution.sh_index(*s).unwrap_or((FileId(0), SectionIndex(0)));
                let recs = read_records(link, file, &h, frame.shape).map_err(|d| fail(*s, d))?;
                counts[o.0 as usize] +=
                    recs.iter().filter(|r| frame.eh_map(*s, r.r_offset).is_some()).count() as u64;
            }
        }
    }
    Ok(counts)
}

fn record_size(shape: Shape, sh_type: u32) -> u64 {
    match (shape.is64(), sh_type == SHT_RELA) {
        (true, true) => 24,
        (true, false) => 16,
        (false, true) => 12,
        (false, false) => 8,
    }
}

/// The header of the relocation section of input section `s`, if any.
fn reloc_shdr(
    link: &RelocatableLink<'_, '_>,
    s: SectionId,
) -> Result<Option<mortar_elf::Shdr>, Diagnostic> {
    let Some(row) = link.resolution.sections.get(s) else { return Ok(None) };
    if row.reloc.0 == 0 {
        return Ok(None);
    }
    let (file, _) =
        link.resolution.sh_index(s).ok_or_else(|| internal("a section without file"))?;
    let o = object(link.inputs, file).ok_or_else(|| internal("relocations of no object"))?;
    let h = *o
        .section(row.reloc)
        .map_err(|e| internal(format!("file {} section {}: {e:?}", file.0, row.reloc.0)))?
        .header();
    Ok(Some(h))
}

/// One input record, raw: `r_sym` is the input's index.
#[derive(Debug, Clone, Copy)]
struct InRec {
    r_offset: u64,
    r_sym: u32,
    r_type: u32,
    /// `None` on REL.
    addend: Option<i64>,
}

/// The records of the relocation section with header `h` of `file`.
fn read_records(
    link: &RelocatableLink<'_, '_>,
    file: FileId,
    h: &mortar_elf::Shdr,
    shape: Shape,
) -> Result<Vec<InRec>, Diagnostic> {
    let bytes = input_bytes(link, file, h.sh_offset..h.sh_offset.saturating_add(h.sh_size))?;
    let rela = h.sh_type == SHT_RELA;
    let n = record_size(shape, h.sh_type) as usize;
    let mut out = Vec::with_capacity(bytes.len() / n);
    for r in bytes.chunks_exact(n) {
        let w = |at: usize, len: usize| -> u64 {
            let mut b = [0u8; 8];
            b[..len].copy_from_slice(&r[at..at + len]);
            u64::from_le_bytes(b)
        };
        let (off, info, addend) = if shape.is64() {
            (w(0, 8), w(8, 8), rela.then(|| w(16, 8) as i64))
        } else {
            (w(0, 4), w(4, 4), rela.then(|| i64::from(w(8, 4) as u32 as i32)))
        };
        let (r_sym, r_type) = if shape.is64() {
            ((info >> 32) as u32, info as u32)
        } else {
            ((info >> 8) as u32, (info & 0xff) as u32)
        };
        out.push(InRec { r_offset: off, r_sym, r_type, addend });
    }
    Ok(out)
}

// ---- groups (L-R3, ruling S8) --------------------------------------------

/// The content of each output group: the flag word and the member list.
struct Groups {
    /// Sorted by input group section.
    rows: Vec<(SectionId, Vec<u32>)>,
}

impl Groups {
    fn new(link: &RelocatableLink<'_, '_>, frame: &Frame) -> Result<Self, Failed> {
        let mut rows = Vec::new();
        for c in frame.by_index.iter().flatten() {
            let ChunkKind::Group(g) = chunk(link, *c).map_err(|d| one(key0(0), d))?.kind else {
                continue;
            };
            let fail = |d| one(key0(u64::from(g.0)), d);
            let words = group_words(link, g).map_err(fail)?;
            let (file, _) = link
                .resolution
                .sh_index(g)
                .ok_or_else(|| fail(internal("a group without file")))?;
            let mut list = vec![words.first().copied().unwrap_or(0)];
            for &m in words.iter().skip(1) {
                let Some(sid) = link.resolution.section_id(file, SectionIndex(m)) else {
                    return Err(fail(internal(format!("group {} names section {m}", g.0))));
                };
                if link.resolution.sections.get(sid).is_some_and(|r| r.kind == SectionKind::Reloc) {
                    continue;
                }
                let Some(o) = link.layout.assignment.out_of.get(sid).copied().and_then(OutOf::out)
                else {
                    continue;
                };
                let Some(ix) = frame.index_of(o) else { continue };
                list.push(ix);
                if let Some(r) = frame.relocs_of.get(o.0 as usize).copied().flatten() {
                    let rix = chunk(link, r).map_err(fail)?.shndx.unwrap_or(0);
                    list.push(rix);
                }
            }
            rows.push((g, list));
        }
        rows.sort_by_key(|r| r.0);
        Ok(Self { rows })
    }

    fn of(&self, g: SectionId) -> Option<&[u32]> {
        let at = self.rows.binary_search_by_key(&g, |r| r.0).ok()?;
        Some(&self.rows[at].1)
    }

    fn size_of(&self, g: SectionId) -> u64 {
        self.of(g).map_or(0, |w| 4 * w.len() as u64)
    }
}

/// The words of input group section `g`.
fn group_words(link: &RelocatableLink<'_, '_>, g: SectionId) -> Result<Vec<u32>, Diagnostic> {
    let h = input_shdr(link, g)?;
    let bytes = section_bytes(link, g, &h)?;
    Ok(bytes.chunks_exact(4).map(|w| u32::from_le_bytes([w[0], w[1], w[2], w[3]])).collect())
}

/// The output index of the signature symbol of input group `g` (L-R3).
fn group_signature(
    link: &RelocatableLink<'_, '_>,
    frame: &Frame,
    syms: &Symbols<'_>,
    g: SectionId,
) -> Result<u32, Diagnostic> {
    let h = input_shdr(link, g)?;
    let (file, _) = link.resolution.sh_index(g).ok_or_else(|| internal("a group without file"))?;
    sym_index(link, frame, syms, file, h.sh_info).map(|(ix, _)| ix)
}

/// I-R5's renaming of symbol `r_sym` of `file`: its output index and,
/// for an `STT_SECTION` symbol, the delta added to the addend.
fn sym_index(
    link: &RelocatableLink<'_, '_>,
    frame: &Frame,
    syms: &Symbols<'_>,
    file: FileId,
    r_sym: u32,
) -> Result<(u32, i64), Diagnostic> {
    if r_sym == 0 {
        return Ok((0, 0));
    }
    let t = object(link.inputs, file)
        .and_then(|o| o.symbols().ok().flatten())
        .ok_or_else(|| internal(format!("file {} has records and no symbol table", file.0)))?;
    let first = t.first_global().0;
    if r_sym >= first {
        let id = link
            .inputs
            .sym_map_of(file)
            .get((r_sym - first) as usize)
            .ok_or_else(|| internal(format!("file {}: symbol {r_sym} has no key", file.0)))?;
        let ix = syms.global_out.get(id.0 as usize).copied().unwrap_or(0);
        if ix == 0 {
            return Err(internal(format!(
                "file {}: symbol {r_sym} names a key the -r output does not write",
                file.0
            )));
        }
        return Ok((ix, 0));
    }
    let s = t
        .get(SymbolIndex(r_sym))
        .map_err(|e| internal(format!("file {}: symbol {r_sym}: {e:?}", file.0)))?;
    if s.sym_type() != STT_SECTION {
        let ix = syms.local_out.get(file.0 as usize).and_then(|m| m.get(r_sym as usize)).copied();
        let dropped_copy = || {
            matches!(t.section_of(SymbolIndex(r_sym), &s), Ok(SymbolSection::Section(i))
                if link.resolution.section_id(file, i).and_then(|t| link.resolution.sections.get(t))
                    .is_some_and(|r| r.state == SectionState::Dropped(DropReason::Comdat)))
        };
        return match ix {
            Some(ix) if ix != 0 => Ok((ix, 0)),
            _ if dropped_copy() => Err(later(
                "relocations against a dropped COMDAT copy at -r",
                "RL-8 (spec/image.md I-R7)",
            )),
            _ => Err(internal(format!(
                "file {}: a record names local symbol {r_sym}, which the -r output does not keep",
                file.0
            ))),
        };
    }
    let SymbolSection::Section(i) = t
        .section_of(SymbolIndex(r_sym), &s)
        .map_err(|e| internal(format!("file {}: symbol {r_sym}: {e:?}", file.0)))?
    else {
        return Err(internal(format!("file {}: a section symbol of no section", file.0)));
    };
    let target = link
        .resolution
        .section_id(file, i)
        .ok_or_else(|| internal(format!("file {}: section {} has no id", file.0, i.0)))?;
    let row = link.resolution.sections.get(target).copied();
    if let Some(SectionState::Dropped(DropReason::Comdat)) = row.map(|r| r.state) {
        return Err(later(
            "relocations against a dropped COMDAT copy at -r",
            "RL-8 (spec/image.md I-R7)",
        ));
    }
    if live_eh(link, target) {
        // The addend is an offset in `T`; it goes through the record
        // map (`sym_eh`).
        let (_, o) = frame.eh.ok_or_else(|| internal("no .eh_frame output"))?;
        let ix = frame.index_of(o).ok_or_else(|| internal(".eh_frame has no index"))?;
        return Ok((frame.secsym[ix as usize], i64::MIN));
    }
    let Some((ix, off)) = frame.where_is(link, target, 0) else {
        return Err(internal(format!(
            "file {}: a record against section {}, which is not in the -r output",
            file.0, i.0
        )));
    };
    let n = frame.secsym.get(ix as usize).copied().unwrap_or(0);
    if n == 0 {
        return Err(internal(format!("output section {ix} has no section symbol")));
    }
    Ok((n, off as i64))
}

// ---- .shstrtab and the section headers -----------------------------------------

/// `.shstrtab`'s bytes and each header's name offset (S-90, ruling S3).
fn shstrtab(link: &RelocatableLink<'_, '_>, frame: &Frame) -> (Vec<u8>, Vec<u32>) {
    let outsecs = &link.layout.assignment.outsecs;
    let rel = if frame.shape.rela { &b".rela"[..] } else { &b".rel"[..] };
    let names: Vec<Vec<u8>> = frame
        .by_index
        .iter()
        .map(|c| {
            let Some(c) = c.and_then(|c| link.layout.layout.placement.chunks.get(c)) else {
                return Vec::new();
            };
            match c.kind {
                ChunkKind::Body(p) => outsecs.get(p.out).map_or_else(Vec::new, |o| o.name.to_vec()),
                ChunkKind::Relocs(o) => {
                    let mut n = rel.to_vec();
                    n.extend_from_slice(outsecs.get(o).map_or(&[][..], |o| o.name));
                    n
                }
                ChunkKind::Group(_) => b".group".to_vec(),
                ChunkKind::Synthetic(s) => s.name.to_vec(),
                _ => Vec::new(),
            }
        })
        .collect();
    let refs: Vec<&[u8]> = names.iter().map(Vec::as_slice).collect();
    headers::shstrtab(&refs)
}

// ---- the build id and the property note ------------------------------------------

/// The errors a `mortar_synth` plan reported into `diag`, keyed in phase
/// 100; `what` when it reported none.
fn collected(diag: mortar_diag::Collector, what: &str) -> Failed {
    let report = diag.finish();
    let diagnostics: Vec<(Key, Diagnostic)> = report
        .diagnostics()
        .iter()
        .filter(|d| d.severity() == mortar_diag::Severity::Error)
        .map(|d| (key0(0), d.clone()))
        .collect();
    Failed {
        diagnostics: if diagnostics.is_empty() {
            vec![(key0(0), internal(what))]
        } else {
            diagnostics
        },
    }
}

/// The build-id note of `link` (I-R8): the final link's plan
/// (`mortar_synth::notes::BuildIdPlan`, the single source of its size,
/// ruling S16). `None` without `--build-id`.
///
/// # Errors
///
/// A style the final link refuses, refused the same way (I-R8).
pub fn build_id_plan(
    link: &RelocatableLink<'_, '_>,
) -> Result<Option<mortar_synth::notes::BuildIdPlan>, Failed> {
    let diag = mortar_diag::Collector::new(mortar_diag::Policy::default());
    match mortar_synth::notes::BuildIdPlan::new(link.plan.shaping.build_id.as_ref(), &diag) {
        Ok(p) => Ok(p),
        Err(_) => Err(collected(diag, "the build-id plan failed")),
    }
}

/// S-94/S-95 over the live inputs' `.note.gnu.property` sections (L-R1,
/// ruling S10): the final link's plan, so that the layout's size and
/// these bytes come from one function. `None` when no input has a note
/// with a property that survives.
pub fn property_plan(
    link: &RelocatableLink<'_, '_>,
) -> Result<Option<mortar_synth::PropertyPlan>, Failed> {
    let diag = mortar_diag::Collector::new(mortar_diag::Policy::default());
    match property_plan_with(link, &diag) {
        Ok(p) => Ok(p),
        Err(f) if diag.has_errors() => {
            let _ = f;
            Err(collected(diag, "the property note merge failed"))
        }
        Err(f) => Err(f),
    }
}

/// [`property_plan`], with the merge's warnings (S-95's, as in a final
/// link) emitted into `diag` (the link's collector; WP-187).
///
/// # Errors
///
/// [`property_plan`]'s. The errors are also in `diag`.
pub fn property_plan_with(
    link: &RelocatableLink<'_, '_>,
    diag: &mortar_diag::Collector,
) -> Result<Option<mortar_synth::PropertyPlan>, Failed> {
    use mortar_synth::notes::ObjectNotes;
    let mut paths = Vec::new();
    let mut per: Vec<Vec<(SectionId, Vec<u8>, u64)>> = Vec::new();
    for (file, row) in link.inputs.files.iter() {
        if !live_object(link, file) {
            continue;
        }
        let mut secs = Vec::new();
        for s in link.resolution.section_base[file].iter() {
            let Some(r) = link.resolution.sections.get(s) else { continue };
            if !r.is_kept() || r.kind != SectionKind::Note {
                continue;
            }
            let h = input_shdr(link, s).map_err(|d| one(key0(u64::from(s.0)), d))?;
            if input_name(link, s) != PROPERTY {
                continue;
            }
            let bytes = section_bytes(link, s, &h).map_err(|d| one(key0(u64::from(s.0)), d))?;
            secs.push((s, bytes.into_owned(), h.sh_addralign.max(1)));
        }
        let f = row.display();
        paths.push(match f.member() {
            Some(m) => format!("{}({m})", f.path()),
            None => f.path().to_owned(),
        });
        per.push(secs);
    }
    let borrowed: Vec<Vec<(SectionId, &[u8], u64)>> =
        per.iter().map(|v| v.iter().map(|(s, b, a)| (*s, b.as_slice(), *a)).collect()).collect();
    let objects: Vec<ObjectNotes<'_>> =
        paths.iter().zip(&borrowed).map(|(p, s)| ObjectNotes { path: p, sections: s }).collect();
    let arch = link.arch;
    let processor = |t: u32| arch.property_merge(t);
    match mortar_synth::PropertyPlan::new(
        &objects,
        link.arch.facts().class,
        &processor,
        false,
        diag,
    ) {
        Ok(p) => Ok(p.exists().then_some(p)),
        Err(_) => Err(Failed {
            diagnostics: vec![(key0(0), internal("the property note merge failed"))],
        }),
    }
}

// ---- write ---------------------------------------------------------------

/// The size of one positioned write of the output.
const STEP: usize = 64 << 20;

/// A REL addend to write through RL-9's `relrel::write` once the bodies
/// are in place (I-R5, A-R2).
#[derive(Debug, Clone, Copy)]
struct RelPatch {
    /// The field's file offset.
    at: u64,
    howto: Howto,
    delta: i64,
    key: Key,
    file: FileId,
    section: SectionIndex,
    r_offset: u64,
}

/// One chunk's slice of the file and what filling it reported.
struct Part<'b> {
    chunk: ChunkId,
    bytes: &'b mut [u8],
    errors: Vec<(Key, Diagnostic)>,
    patches: Vec<RelPatch>,
}

/// Everything the fill reads.
struct Image<'l, 'a, 's> {
    link: &'l RelocatableLink<'a, 's>,
    frame: Frame,
    syms: Symbols<'l>,
    groups: Groups,
    property: Option<mortar_synth::PropertyPlan>,
    build_id: Option<mortar_synth::notes::BuildIdPlan>,
    names: (Vec<u8>, Vec<u32>),
}

/// Writes the `ET_REL` output of `link` to `path` (I-R1 to I-R12), as a
/// final link's `crate::link::write` does (the temporary name, the mode
/// `opts.mode`, `0o666` at `-r` (I-59)).
///
/// # Errors
///
/// Every diagnostic of the phase, keyed in phase 100: a refusal by name
/// (`.eh_frame` records removed before RL-8, a REL addend before RL-9, an
/// input relocation section of the other type, A-R4), a relocation the
/// architecture does not know or whose field is outside its section
/// (A-R1), a chunk whose layout size differs from the image's (internal
/// error naming the chunk and both sizes), an input that changed.
pub fn write(
    link: &RelocatableLink<'_, '_>,
    path: &Path,
    opts: &OutputOptions,
) -> Result<Written, Failed> {
    if !link.layout.eh_removed.is_empty() {
        return Err(one(
            key0(0),
            mortar_diag::relocatable::refused_eh_records_removed(),
        ));
    }
    let img = prepare(link)?;
    let bytes = img.fill()?;
    // I-R8: the id over the whole file, descriptor zero (I-73).
    let mut bytes = bytes;
    let mut sealed = None;
    let mut id = None;
    if let (Some(c), Some(plan)) = (img.frame.build_id, &img.build_id)
        && plan.hashed()
    {
        let n = u64::from(plan.desc_size());
        let style = link.build_id.ok_or_else(|| {
            one(
                key0(0),
                internal("a hashed --build-id style with no tree (RelocatableLink::build_id)"),
            )
        })?;
        let at = chunk(link, c).map_err(|d| one(key0(0), d))?.offset + plan.desc_range().start;
        let range = at..at + n;
        let leaf_fn = style.tree_fn();
        let block = mortar_digest::IMAGE_BLOCK as usize;
        let windows = bytes.len().div_ceil(block);
        let leaves: Vec<(u64, mortar_digest::Leaf)> = link.pool.map(windows, |w| {
            let lo = w * block;
            let hi = (lo + block).min(bytes.len());
            (w as u64, mortar_digest::leaf(leaf_fn, &bytes[lo..hi]))
        });
        let v = crate::digest::build_id(style, bytes.len() as u64, &leaves)
            .map_err(|e| one(key0(0), internal(format!("the build id's tree: {e:?}"))))?;
        bytes[range.start as usize..range.end as usize].copy_from_slice(&v[..n as usize]);
        sealed = Some(range);
        id = Some(v[..n as usize].to_vec());
    }
    let out = Output::create(path, opts).map_err(|kd| Failed { diagnostics: vec![kd] })?;
    for (k, piece) in bytes.chunks(STEP).enumerate() {
        if let Err(e) = out.write_at((k * STEP) as u64, piece) {
            out.abort();
            return Err(one(
                key0(0),
                Diagnostic::error(Kind::CannotWriteOutput {
                    path: path.display().to_string(),
                    reason: mortar_diag::os_reason(&e),
                }),
            ));
        }
    }
    // input.md §11.5: the files the store still holds, once more.
    let mut changed = Vec::new();
    for (file, _) in link.inputs.files.iter() {
        if let Err(e) = (link.recheck)(file) {
            changed.push((
                Key::new(PHASE_IMAGE, file.0, 0),
                crate::source::copy_error(e, &file_ref(link.inputs, file)),
            ));
        }
    }
    if !changed.is_empty() {
        out.abort();
        return Err(Failed { diagnostics: changed });
    }
    out.commit().map_err(|kd| Failed { diagnostics: vec![kd] })?;
    Ok(Written {
        build_id: id,
        leaves: None,
        sealed,
        file_size: bytes.len() as u64,
        file: None,
        rows: img.frame.by_index.len(),
        jobs: 1,
    })
}

/// The tables of the image and the size check (the safety net).
fn prepare<'l, 'a, 's>(link: &'l RelocatableLink<'a, 's>) -> Result<Image<'l, 'a, 's>, Failed> {
    // A-R3 (I-R1): the executable rule's merge of the live inputs'.
    let frame = Frame::new(link)?;
    let syms = Symbols::new(link, &frame)?;
    let groups = Groups::new(link, &frame)?;
    let property = property_plan(link)?;
    let build_id = build_id_plan(link)?;
    let sizes = computed_sizes(link, &frame, &syms, &groups, property.as_ref(), build_id.as_ref())?;
    let pl = &link.layout.layout.placement;
    let mut errors = Vec::new();
    for &(id, size) in &sizes.chunks {
        let Some(c) = pl.chunks.get(id) else { continue };
        if c.size != size {
            errors.push((
                key0(u64::from(id.0)),
                internal(format!(
                    "the -r layout gives chunk {} ({}) size {:#x}, the image fills {:#x}",
                    id.0,
                    chunk_label(link, c),
                    c.size,
                    size
                )),
            ));
        }
    }
    if sizes.symtab_shndx != frame.shndx.is_some() {
        errors.push((
            key0(0),
            internal(format!(
                "the -r layout {} .symtab_shndx, the image {} one (ruling S5)",
                if frame.shndx.is_some() { "has" } else { "has no" },
                if sizes.symtab_shndx { "needs" } else { "needs no" }
            )),
        ));
    }
    if build_id.is_some() != frame.build_id.is_some() {
        errors
            .push((key0(0), internal("the -r layout and --build-id disagree on the note (I-R8)")));
    }
    if property.is_some() != frame.property.is_some() {
        errors.push((
            key0(0),
            internal("the -r layout and the inputs disagree on .note.gnu.property (ruling S10)"),
        ));
    }
    if !errors.is_empty() {
        return Err(Failed { diagnostics: errors });
    }
    let names = shstrtab(link, &frame);
    Ok(Image { link, frame, syms, groups, property, build_id, names })
}

fn chunk_label(link: &RelocatableLink<'_, '_>, c: &Chunk) -> String {
    let outsecs = &link.layout.assignment.outsecs;
    let name = |o: OutSecId| {
        outsecs.get(o).map_or_else(String::new, |o| String::from_utf8_lossy(o.name).into_owned())
    };
    match c.kind {
        ChunkKind::Body(p) => name(p.out),
        ChunkKind::Relocs(o) => format!("relocations of {}", name(o)),
        ChunkKind::Group(g) => format!("group of section {}", g.0),
        ChunkKind::Synthetic(s) => String::from_utf8_lossy(s.name).into_owned(),
        k => format!("{k:?}"),
    }
}

impl Image<'_, '_, '_> {
    /// The file's bytes.
    fn fill(&self) -> Result<Vec<u8>, Failed> {
        let link = self.link;
        let pl = &link.layout.layout.placement;
        let mut spans: Vec<(u64, u64, ChunkId)> = pl
            .chunks
            .iter()
            .filter(|(_, c)| c.in_file() && c.size > 0)
            .map(|(id, c)| (c.offset, c.size, id))
            .collect();
        spans.sort_unstable();
        let file_size = spans.iter().map(|s| s.0 + s.1).max().unwrap_or(0);
        let mut buf =
            vec![0u8; usize::try_from(file_size).map_err(|_| one(key0(0), internal("file size")))?];
        let mut parts = Vec::with_capacity(spans.len());
        let mut rest: &mut [u8] = &mut buf;
        let mut at = 0u64;
        for &(off, size, id) in &spans {
            if off < at {
                return Err(one(
                    key0(u64::from(id.0)),
                    internal(format!("chunk {} overlaps the chunk before it", id.0)),
                ));
            }
            let (_, tail) = rest.split_at_mut((off - at) as usize);
            let (mine, tail) = tail.split_at_mut(size as usize);
            parts.push(Part { chunk: id, bytes: mine, errors: Vec::new(), patches: Vec::new() });
            rest = tail;
            at = off + size;
        }
        link.pool.for_each_mut(&mut parts, |_, p| {
            if let Err(e) = self.fill_chunk(p.chunk, p.bytes, &mut p.patches) {
                p.errors.push(e);
            }
        });
        let mut diagnostics = Vec::new();
        let mut patches = Vec::new();
        for p in parts {
            diagnostics.extend(p.errors);
            patches.extend(p.patches);
        }
        if !diagnostics.is_empty() {
            diagnostics.sort_by_key(|d| d.0);
            return Err(Failed { diagnostics });
        }
        // I-R5 on REL: the new addend goes into the field (A-R2), by
        // RL-9's `relrel::write`.
        for p in patches {
            let width = p.howto.implicit.map_or(p.howto.field.width(), |i| i.bytes) as usize;
            let lo = p.at as usize;
            let Some(field) = buf.get_mut(lo..lo + width) else {
                diagnostics.push((p.key, internal("a REL field outside the file")));
                continue;
            };
            if let Err(d) = crate::relrel::write(link.arch, &p.howto, field, p.delta) {
                let name =
                    String::from_utf8_lossy(&input_name_of(link, p.file, p.section)).into_owned();
                diagnostics.push((
                    p.key,
                    d.at(Place::section(file_ref(link.inputs, p.file), name, p.r_offset)),
                ));
            }
        }
        if !diagnostics.is_empty() {
            return Err(Failed { diagnostics });
        }
        Ok(buf)
    }

    fn fill_chunk(
        &self,
        id: ChunkId,
        out: &mut [u8],
        patches: &mut Vec<RelPatch>,
    ) -> Result<(), (Key, Diagnostic)> {
        let link = self.link;
        let c = chunk(link, id).map_err(|d| (key0(0), d))?;
        let key = key0(u64::from(id.0));
        match c.kind {
            ChunkKind::FileHeader => self.write_ehdr(out).map_err(|d| (key, d)),
            ChunkKind::SectionHeaders => self.write_shdrs(out).map_err(|d| (key, d)),
            ChunkKind::Body(p) => self.write_body(p.out, out),
            ChunkKind::Relocs(o) => self.write_relocs(o, c, out, patches),
            ChunkKind::Group(g) => {
                let words =
                    self.groups.of(g).ok_or_else(|| (key, internal("a group with no content")))?;
                for (w, b) in words.iter().zip(out.chunks_exact_mut(4)) {
                    b.copy_from_slice(&w.to_le_bytes());
                }
                Ok(())
            }
            ChunkKind::Synthetic(s) => match s.name {
                SYMTAB => self.write_symtab(out).map_err(|d| (key, d)),
                // Written with `.symtab`'s pass over the entries.
                SYMTAB_SHNDX => self.write_shndx(out).map_err(|d| (key, d)),
                STRTAB => self.write_strtab(out).map_err(|d| (key, d)),
                SHSTRTAB => {
                    out.copy_from_slice(&self.names.0);
                    Ok(())
                }
                EH_FRAME => self.write_eh(out),
                BUILD_ID => {
                    if let Some(p) = &self.build_id {
                        let mut w = mortar_synth::Window::new(0, out);
                        p.write(&mut w);
                    }
                    Ok(())
                }
                PROPERTY => {
                    if let Some(p) = &self.property {
                        let mut w = mortar_synth::Window::new(0, out);
                        p.write(&mut w);
                    }
                    Ok(())
                }
                _ => Err((key, internal("an unknown synthetic chunk"))),
            },
            _ => Err((key, internal("a chunk kind a -r layout has not"))),
        }
    }

    /// I-R1, I-R2, A-R3.
    fn write_ehdr(&self, out: &mut [u8]) -> Result<(), Diagnostic> {
        let link = self.link;
        let f = &self.frame;
        let facts = link.arch.facts();
        let mut flags = link
            .inputs
            .files
            .iter()
            .filter(|(file, _)| live_object(link, *file))
            .filter_map(|(file, _)| object(link.inputs, file).map(|o| o.header().e_flags));
        let e_flags = link.arch.e_flags(&mut flags).map_err(|r| r.diagnostic())?;
        let gnu = self
            .syms
            .locals
            .iter()
            .chain(&self.syms.globals)
            .any(|e| e.info & 0xf == STT_GNU_IFUNC || e.info >> 4 == STB_GNU_UNIQUE);
        let shoff = chunk(link, f.sh_table)?.offset;
        let shstrndx = f.shstrtab.1;
        let h = Ehdr {
            osabi: if gnu { ELFOSABI_GNU } else { ELFOSABI_NONE },
            e_type: ET_REL,
            e_machine: facts.e_machine,
            e_entry: 0,
            e_phoff: 0,
            e_shoff: shoff,
            e_flags,
            e_phnum: 0,
            e_shnum: if f.shnum >= u32::from(SHN_LORESERVE) { 0 } else { f.shnum as u16 },
            e_shstrndx: if shstrndx >= u32::from(SHN_LORESERVE) {
                SHN_XINDEX
            } else {
                shstrndx as u16
            },
        };
        out.copy_from_slice(&headers::ehdr(f.shape.class, &h));
        Ok(())
    }

    /// The section header table (L-R8, rulings S6-S9).
    fn write_shdrs(&self, out: &mut [u8]) -> Result<(), Diagnostic> {
        let link = self.link;
        let f = &self.frame;
        let mut rows = Vec::with_capacity(f.shnum as usize);
        let mut zero = Shdr::default();
        if f.shnum >= u32::from(SHN_LORESERVE) {
            zero.sh_size = u64::from(f.shnum);
        }
        if f.shstrtab.1 >= u32::from(SHN_LORESERVE) {
            zero.sh_link = f.shstrtab.1;
        }
        rows.push(zero);
        for (ix, c) in f.by_index.iter().enumerate().skip(1) {
            let id = c.ok_or_else(|| internal("a header index without chunk"))?;
            let c = chunk(link, id)?;
            let mut h = Shdr {
                sh_name: self.names.1[ix],
                sh_type: c.sh_type,
                sh_flags: c.flags,
                sh_addr: 0,
                sh_offset: c.offset,
                sh_size: c.size,
                sh_link: 0,
                sh_info: 0,
                sh_addralign: 1u64 << c.align,
                sh_entsize: 0,
            };
            match c.kind {
                ChunkKind::Body(p) => {
                    let (entsize, link_to) = self.body_attrs(p.out)?;
                    h.sh_entsize = entsize;
                    if c.flags & SHF_LINK_ORDER != 0 {
                        h.sh_link = link_to.ok_or_else(|| {
                            internal(format!(
                                "{}: SHF_LINK_ORDER with no linked-to section",
                                chunk_label(link, c)
                            ))
                        })?;
                    }
                }
                ChunkKind::Relocs(o) => {
                    h.sh_link = f.symtab.1;
                    h.sh_info = f
                        .index_of(o)
                        .ok_or_else(|| internal("relocations of a section with no index"))?;
                    h.sh_entsize = f.shape.rel;
                }
                ChunkKind::Group(g) => {
                    h.sh_link = f.symtab.1;
                    h.sh_info = group_signature(link, f, &self.syms, g)?;
                    h.sh_entsize = 4;
                }
                ChunkKind::Synthetic(s) => match s.name {
                    SYMTAB => {
                        h.sh_link = f.strtab.1;
                        h.sh_info = self.syms.first_global;
                        h.sh_entsize = f.shape.sym;
                    }
                    SYMTAB_SHNDX => {
                        h.sh_link = f.symtab.1;
                        h.sh_entsize = 4;
                    }
                    EH_FRAME => {
                        h.sh_entsize = common_entsize(link, f.eh_sections.iter().map(|e| e.0))?;
                    }
                    _ => {}
                },
                _ => {}
            }
            rows.push(h);
        }
        out.copy_from_slice(&headers::shdrs(f.shape.class, &rows));
        Ok(())
    }

    /// S7: a body's `sh_entsize` (the pieces' common one, else 0) and the
    /// header index its `SHF_LINK_ORDER` links to.
    fn body_attrs(&self, o: OutSecId) -> Result<(u64, Option<u32>), Diagnostic> {
        let link = self.link;
        let pieces = self.frame.pieces.get(o.0 as usize).map_or(&[][..], Vec::as_slice);
        let entsize = common_entsize(link, pieces.iter().map(|p| p.1))?;
        let mut to = None;
        for &(_, s) in pieces {
            let h = input_shdr(link, s)?;
            if h.sh_flags & SHF_LINK_ORDER == 0 || h.sh_link == 0 {
                continue;
            }
            let (file, _) =
                link.resolution.sh_index(s).ok_or_else(|| internal("a section without file"))?;
            let target = link.resolution.section_id(file, SectionIndex(h.sh_link));
            let ix = target.and_then(|t| self.frame.where_is(link, t, 0)).map(|w| w.0);
            if to.is_none() {
                to = ix;
            }
        }
        Ok((entsize, to))
    }

    fn write_body(&self, o: OutSecId, out: &mut [u8]) -> Result<(), (Key, Diagnostic)> {
        let link = self.link;
        for &(off, s) in self.frame.pieces.get(o.0 as usize).map_or(&[][..], Vec::as_slice) {
            let key = key0(u64::from(s.0));
            let h = input_shdr(link, s).map_err(|d| (key, d))?;
            if let Some(r) = link.resolution.sections.get(s)
                && r.kind == SectionKind::Meta
            {
                return Err((
                    key,
                    later("Mason metadata containers at -r", "RL-12 (spec/meta.md MT-30)"),
                ));
            }
            if h.sh_flags & SHF_COMPRESSED != 0 {
                return Err((
                    key,
                    later(
                        "compressed input sections at -r",
                        "a later -r package (spec/layout.md L-R1a)",
                    ),
                ));
            }
            if h.sh_type == SHT_NOBITS || h.sh_size == 0 {
                continue;
            }
            let (file, _) = link
                .resolution
                .sh_index(s)
                .ok_or_else(|| (key, internal("a section without file")))?;
            let lo = off as usize;
            let dst = out
                .get_mut(lo..lo + h.sh_size as usize)
                .ok_or_else(|| (key, internal("a piece outside its output section")))?;
            crate::source::read(link.inputs, file, h.sh_offset..h.sh_offset + h.sh_size, dst)
                .map_err(|e| (key, crate::source::copy_error(e, &file_ref(link.inputs, file))))?;
        }
        Ok(())
    }

    /// I-R6 (ruling S1): the kept records, contiguous, each FDE's CIE
    /// pointer recomputed.
    fn write_eh(&self, out: &mut [u8]) -> Result<(), (Key, Diagnostic)> {
        let link = self.link;
        for (s, range, _) in &self.frame.eh_sections {
            let key = key0(u64::from(s.0));
            let h = input_shdr(link, *s).map_err(|d| (key, d))?;
            let bytes = section_bytes(link, *s, &h).map_err(|d| (key, d))?;
            let recs = &self.frame.eh_recs[range.clone()];
            for r in recs {
                let Some(o) = r.out else { continue };
                let src = bytes
                    .get(r.input as usize..(r.input + r.size) as usize)
                    .ok_or_else(|| (key, internal("an .eh_frame record outside its section")))?;
                let dst = out
                    .get_mut(o as usize..(o + r.size) as usize)
                    .ok_or_else(|| (key, internal("an .eh_frame record outside the chunk")))?;
                dst.copy_from_slice(src);
                if let Some(cie) = r.cie {
                    let c = self
                        .frame
                        .eh_map(*s, cie)
                        .ok_or_else(|| (key, internal("an FDE whose CIE is not written")))?;
                    // The pointer is the distance from its own field back
                    // to the CIE (the `.eh_frame` form of the CIE pointer).
                    let ptr = (o + 4)
                        .checked_sub(c)
                        .ok_or_else(|| (key, internal("an FDE before its CIE")))?;
                    let ptr = u32::try_from(ptr)
                        .map_err(|_| (key, internal("a CIE pointer past 4 GiB")))?;
                    dst.get_mut(4..8)
                        .ok_or_else(|| (key, internal("an FDE of fewer than 8 bytes")))?
                        .copy_from_slice(&ptr.to_le_bytes());
                }
            }
        }
        Ok(())
    }

    /// I-R5 (ruling S6): the records of the pieces of `o`, in piece order
    /// then input order.
    fn write_relocs(
        &self,
        o: OutSecId,
        c: &Chunk,
        out: &mut [u8],
        patches: &mut Vec<RelPatch>,
    ) -> Result<(), (Key, Diagnostic)> {
        let f = &self.frame;
        let mut at = 0usize;
        let eh = f.eh.is_some_and(|(_, e)| e == o);
        let list: Vec<(Option<u64>, SectionId)> = if eh {
            f.eh_sections.iter().map(|e| (None, e.0)).collect()
        } else {
            f.pieces
                .get(o.0 as usize)
                .map_or_else(Vec::new, |p| p.iter().map(|&(off, s)| (Some(off), s)).collect())
        };
        // The file offset of the target output section's bytes, for a REL
        // field.
        let base = f
            .index_of(o)
            .and_then(|ix| f.by_index[ix as usize])
            .and_then(|id| self.link.layout.layout.placement.chunks.get(id))
            .map(|c| c.offset);
        for (piece, s) in list {
            self.relocs_of_section(s, piece, base, out, &mut at, patches)?;
        }
        if at as u64 != c.size {
            return Err((key0(0), internal("the relocation records do not fill their chunk")));
        }
        Ok(())
    }

    #[allow(clippy::too_many_lines)]
    fn relocs_of_section(
        &self,
        s: SectionId,
        piece: Option<u64>,
        base: Option<u64>,
        out: &mut [u8],
        at: &mut usize,
        patches: &mut Vec<RelPatch>,
    ) -> Result<(), (Key, Diagnostic)> {
        let link = self.link;
        let f = &self.frame;
        let shape = f.shape;
        let (file, index) = link
            .resolution
            .sh_index(s)
            .ok_or_else(|| (key0(0), internal("a section without file")))?;
        let key = |n: u32| Key::new(PHASE_IMAGE, file.0, u64::from(index.0) << 32 | u64::from(n));
        let Some(rh) = reloc_shdr(link, s).map_err(|d| (key(0), d))? else { return Ok(()) };
        let rel_index = link.resolution.sections[s].reloc;
        let place = || {
            let name = String::from_utf8_lossy(&input_name_of(link, file, rel_index)).into_owned();
            Place::section(file_ref(link.inputs, file), name, 0)
        };
        // A-R4: the other form is refused by name.
        let want = if shape.rela { SHT_RELA } else { SHT_REL };
        if rh.sh_type != want {
            let name = if rh.sh_type == SHT_REL { "SHT_REL" } else { "SHT_RELA" };
            return Err((key(0), Diagnostic::refused(RefusalClass::SectionType, name).at(place())));
        }
        let h = input_shdr(link, s).map_err(|d| (key(0), d))?;
        let bytes = section_bytes(link, s, &h).map_err(|d| (key(0), d))?;
        let recs = read_records(link, file, &rh, shape).map_err(|d| (key(0), d))?;
        let sname = || String::from_utf8_lossy(&input_name_of(link, file, index)).into_owned();
        for (n, r) in recs.iter().enumerate() {
            let k = key(n as u32);
            // A-R1: the checks of §3.4 that need no target.
            let howto = link
                .arch
                .classify(r.r_type, Site { bytes: &bytes, offset: r.r_offset })
                .map_err(|e| {
                    (
                        k,
                        e.diagnostic().at(Place::section(
                            file_ref(link.inputs, file),
                            sname(),
                            r.r_offset,
                        )),
                    )
                })?;
            let width = howto.implicit.map_or(howto.field.width(), |i| i.bytes);
            let site = Site { bytes: &bytes, offset: r.r_offset };
            if site.at(usize::from(width)).is_none() {
                return Err((
                    k,
                    Diagnostic::error(Kind::Malformed {
                        what: format!(
                            "relocation {} at offset {:#x} lies outside its section",
                            howto.name, r.r_offset
                        ),
                    })
                    .at(Place::section(
                        file_ref(link.inputs, file),
                        sname(),
                        0,
                    )),
                ));
            }
            let out_off = match piece {
                Some(p) => p + r.r_offset,
                None => match f.eh_map(s, r.r_offset) {
                    Some(v) => v,
                    // A record of a removed FDE (RL-8; refused before).
                    None => continue,
                },
            };
            let (sym, delta) = sym_index(link, f, &self.syms, file, r.r_sym).map_err(|d| (k, d))?;
            let delta = if delta == i64::MIN {
                // A section symbol of an `.eh_frame` input: the addend is
                // an offset in it, mapped by L-R6's record map.
                let t = section_of_sym(link, file, r.r_sym).map_err(|d| (k, d))?;
                let a = r.addend.unwrap_or(0);
                let mapped = u64::try_from(a)
                    .ok()
                    .and_then(|a| f.eh_map(t, a))
                    .ok_or_else(|| (k, internal("a record into a removed .eh_frame record")))?;
                if r.addend.is_none() {
                    return Err((
                        k,
                        later("REL records against .eh_frame at -r", "RL-9 (spec/arch.md A-R2)"),
                    ));
                }
                mapped as i64 - a
            } else {
                delta
            };
            let rec = out
                .get_mut(*at..*at + shape.rel as usize)
                .ok_or_else(|| (k, internal("more relocation records than the chunk holds")))?;
            if shape.is64() {
                rec[0..8].copy_from_slice(&out_off.to_le_bytes());
                let info = u64::from(sym) << 32 | u64::from(r.r_type);
                rec[8..16].copy_from_slice(&info.to_le_bytes());
            } else {
                let off32 = u32::try_from(out_off)
                    .map_err(|_| (k, internal("an offset past 4 GiB in ELF32")))?;
                rec[0..4].copy_from_slice(&off32.to_le_bytes());
                if sym >= 1 << 24 {
                    return Err((
                        k,
                        later("more than 2^24 symbols in an ELF32 -r output", "no package"),
                    ));
                }
                rec[4..8].copy_from_slice(&(sym << 8 | (r.r_type & 0xff)).to_le_bytes());
            }
            match r.addend {
                Some(a) => {
                    let a = a.wrapping_add(delta);
                    if shape.is64() {
                        rec[16..24].copy_from_slice(&a.to_le_bytes());
                    } else {
                        rec[8..12].copy_from_slice(&(a as i32).to_le_bytes());
                    }
                }
                None if delta != 0 => {
                    // The field in the file: the output section's offset plus
                    // the record's output offset (a body's or `.eh_frame`'s).
                    let Some(base) = base.map(|b| b + out_off) else {
                        return Err((k, internal("a REL field with no place")));
                    };
                    patches.push(RelPatch {
                        at: base,
                        howto,
                        delta,
                        key: k,
                        file,
                        section: index,
                        r_offset: r.r_offset,
                    });
                }
                None => {}
            }
            *at += shape.rel as usize;
        }
        Ok(())
    }

    /// `.symtab`: I-R4's four blocks.
    fn write_symtab(&self, out: &mut [u8]) -> Result<(), Diagnostic> {
        let f = &self.frame;
        let n = f.shape.sym as usize;
        let mut put =
            |i: usize, name: u32, info: u8, other: u8, shndx: u16, value: u64, size: u64| {
                let e = &mut out[i * n..(i + 1) * n];
                e[0..4].copy_from_slice(&name.to_le_bytes());
                if f.shape.is64() {
                    e[4] = info;
                    e[5] = other;
                    e[6..8].copy_from_slice(&shndx.to_le_bytes());
                    e[8..16].copy_from_slice(&value.to_le_bytes());
                    e[16..24].copy_from_slice(&size.to_le_bytes());
                } else {
                    e[4..8].copy_from_slice(&(value as u32).to_le_bytes());
                    e[8..12].copy_from_slice(&(size as u32).to_le_bytes());
                    e[12] = info;
                    e[13] = other;
                    e[14..16].copy_from_slice(&shndx.to_le_bytes());
                }
            };
        let short = |ix: u32| if ix >= u32::from(SHN_LORESERVE) { SHN_XINDEX } else { ix as u16 };
        let mut i = 1;
        for (ix, &k) in f.secsym.iter().enumerate() {
            if k != 0 {
                put(i, 0, STB_LOCAL << 4 | STT_SECTION, 0, short(ix as u32), 0, 0);
                i += 1;
            }
        }
        let mut str_at = mortar_synth::symtab::fixed_table().len() as u64;
        for e in self.syms.locals.iter().chain(&self.syms.globals) {
            let name = match e.name {
                Name::Plain(nm) if fixed_name(nm).is_some() => fixed_name(nm).unwrap_or(0),
                nm => {
                    let at = str_at;
                    str_at += nm.len() + 1;
                    u32::try_from(at).map_err(|_| internal(".strtab past 4 GiB"))?
                }
            };
            let shndx = if e.section { short(e.shndx) } else { e.shndx as u16 };
            put(i, name, e.info, e.other, shndx, e.value, e.size);
            i += 1;
        }
        Ok(())
    }

    /// `.symtab_shndx` (ruling S5): the index of each entry that needs
    /// one, 0 for every other.
    fn write_shndx(&self, out: &mut [u8]) -> Result<(), Diagnostic> {
        let f = &self.frame;
        let mut words = out.chunks_exact_mut(4).skip(1);
        for (ix, &k) in f.secsym.iter().enumerate() {
            if k != 0 {
                let w = words.next().ok_or_else(|| internal(".symtab_shndx is short"))?;
                if ix >= usize::from(SHN_LORESERVE) {
                    w.copy_from_slice(&(ix as u32).to_le_bytes());
                }
            }
        }
        for e in self.syms.locals.iter().chain(&self.syms.globals) {
            let w = words.next().ok_or_else(|| internal(".symtab_shndx is short"))?;
            if e.needs_xindex() {
                w.copy_from_slice(&e.shndx.to_le_bytes());
            }
        }
        Ok(())
    }

    /// `.strtab` (S-86, ruling S2).
    fn write_strtab(&self, out: &mut [u8]) -> Result<(), Diagnostic> {
        let fixed = mortar_synth::symtab::fixed_table();
        out[..fixed.len()].copy_from_slice(fixed);
        let mut at = fixed.len();
        for e in self.syms.locals.iter().chain(&self.syms.globals) {
            if e.name.region() == 0 {
                continue;
            }
            let len = e.name.len() as usize;
            let dst = out.get_mut(at..at + len + 1).ok_or_else(|| internal(".strtab is short"))?;
            e.name.put(dst);
            at += len + 1;
        }
        Ok(())
    }
}

/// The section of `STT_SECTION` symbol `r_sym` of `file`.
fn section_of_sym(
    link: &RelocatableLink<'_, '_>,
    file: FileId,
    r_sym: u32,
) -> Result<SectionId, Diagnostic> {
    let t = object(link.inputs, file)
        .and_then(|o| o.symbols().ok().flatten())
        .ok_or_else(|| internal("no symbol table"))?;
    let s = t.get(SymbolIndex(r_sym)).map_err(|e| internal(format!("{e:?}")))?;
    match t.section_of(SymbolIndex(r_sym), &s) {
        Ok(SymbolSection::Section(i)) => {
            link.resolution.section_id(file, i).ok_or_else(|| internal("a section with no id"))
        }
        _ => Err(internal("a section symbol of no section")),
    }
}

fn input_name_of(link: &RelocatableLink<'_, '_>, file: FileId, index: SectionIndex) -> Vec<u8> {
    object(link.inputs, file)
        .and_then(|o| o.section(index).ok()?.name().ok())
        .map_or_else(Vec::new, <[u8]>::to_vec)
}

/// The common `sh_entsize` of `sections`, else 0 (ruling S7).
fn common_entsize(
    link: &RelocatableLink<'_, '_>,
    sections: impl Iterator<Item = SectionId>,
) -> Result<u64, Diagnostic> {
    let mut common: Option<u64> = None;
    for s in sections {
        let e = input_shdr(link, s)?.sh_entsize;
        match common {
            None => common = Some(e),
            Some(c) if c == e => {}
            Some(_) => return Ok(0),
        }
    }
    Ok(common.unwrap_or(0))
}

// ---- the map (I-R12, spec/plumb.md §14.2) -----------------------------------

/// The map of `link` (`spec/plumb.md` §14.2): `file`, the `-r` records
/// ([`Map::relocatable`]), `discard`, `output relocatable` and `arg`.
///
/// # Errors
///
/// A table the image cannot read, by name.
pub fn map_of(link: &RelocatableLink<'_, '_>) -> Result<Map, Failed> {
    let frame = Frame::new(link)?;
    let pl = &link.layout.layout.placement;
    let mut m = Map::default();
    let mut r = RelocatableRecords::default();
    let mut ids: Vec<Option<u32>> = vec![None; link.inputs.files.len() as usize];
    for (file, row) in link.inputs.files.iter() {
        if !live_object(link, file) {
            continue;
        }
        let id = m.files.len() as u32;
        ids[file.0 as usize] = Some(id);
        m.files.push(MapFile {
            path: row.path.to_vec(),
            member: row.member.as_ref().map(|mm| (mm.name.to_vec(), mm.offset)),
        });
        let mut places = Vec::new();
        let mut frames = Vec::new();
        let mut discards = Vec::new();
        let range = link.resolution.section_base[file];
        for (i, s) in range.iter().enumerate().skip(1) {
            let i = i as u32;
            let Some(row) = link.resolution.sections.get(s) else { continue };
            let h = input_shdr(link, s).map_err(|d| one(key0(u64::from(s.0)), d))?;
            let bytes = h.sh_size > 0 && h.sh_type != SHT_NOBITS;
            if let Some((c, off)) = pl.place.get(s).and_then(|p| p.get()) {
                let out = chunk(link, c).map_err(|d| one(key0(0), d))?.shndx.unwrap_or(0);
                places.push(RPlace { section: i, out, offset: off, size: h.sh_size });
                continue;
            }
            if let Some(recs) = frame.eh_of(s) {
                frames.extend(recs.iter().map(|e| RFrame {
                    section: i,
                    input_offset: e.input,
                    output_offset: e.out,
                }));
                continue;
            }
            let why = match row.state {
                SectionState::Dropped(DropReason::Consumed) => None,
                SectionState::Dropped(DropReason::Comdat) => Some(DiscardWhy::Comdat),
                SectionState::Dropped(_) => Some(DiscardWhy::Unspecified),
                SectionState::Kept => {
                    let consumed = !row.kind.carries_output()
                        || matches!(link.layout.assignment.out_of.get(s), Some(OutOf::Consumed))
                        || (row.kind == SectionKind::Note && input_name(link, s) == PROPERTY);
                    (!consumed).then_some(DiscardWhy::Unspecified)
                }
            };
            if let Some(why) = why
                && bytes
            {
                discards.push((i, why));
            }
        }
        places.sort_by_key(|p| (p.out, p.offset, p.section));
        r.rplace.push(places);
        r.rframe.push(frames);
        m.discards.push(discards);
    }
    for (ix, c) in frame.by_index.iter().enumerate() {
        let Some(c) = c.and_then(|c| pl.chunks.get(c)) else { continue };
        if let ChunkKind::Group(g) = c.kind
            && let Some((file, index)) = link.resolution.sh_index(g)
            && let Some(id) = ids.get(file.0 as usize).copied().flatten()
        {
            r.rgroup.push(RGroup { out: ix as u32, file: id, section: index.0 });
        }
    }
    for p in &pl.sym_place {
        let AllocKey::Common(id) = p.key else { continue };
        let Some(key) = link.inputs.symbols.get(id) else { continue };
        let mut name = key.name.to_vec();
        if let Some(v) = key.version {
            name.push(b'@');
            name.extend_from_slice(v);
        }
        r.rcommon.push(RCommon {
            name,
            out: pl.chunks.get(p.chunk).and_then(|c| c.shndx).unwrap_or(0),
            offset: p.offset,
            size: p.size,
            align: 1u64 << p.align,
        });
    }
    r.rcommon.sort_by(|a, b| a.name.cmp(&b.name));
    m.relocatable = Some(r);
    m.dynamic = Some(DynRecords { kind: Some(MapKind::Relocatable), ..DynRecords::default() });
    m.args = link.args.to_vec();
    Ok(m)
}
