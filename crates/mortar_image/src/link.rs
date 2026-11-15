//! The image phase over the front's tables (phase 100 and the output file,
//! phase 105): the one entry point the driver calls once layout has
//! reached its fixed point ([`write`]).
//!
//! What it joins:
//! - the fill tables (`spec/arch.md` §3.6 `FillTables`, image.md §6.1
//!   `target_alloc`, synth.md S-17/S-19/S-50 references into split
//!   sections) over the layout's addresses (model §4.5), with the record
//!   keys decoded as the scan decoded them ([`RecordKeys`]);
//! - the synthetic chunks through `mortar_synth`'s plans (§13), with the
//!   answers their `write` reads ([`SlotValues`], [`SymAddrs`], the FDE
//!   code addresses);
//! - the structural units from `Placement` (I-40, [`structure`]);
//! - the fill plan ([`crate::layout_plan::from_layout`]), fill and the
//!   output file (§7), then `input-changed` once more on every input the
//!   store still holds (`spec/input.md` §11.5) before the rename.

use std::path::Path;

use mortar_arch::{
    Arch, DemandFlags, FillTables, OutputKind, ProofRun, SlotKind, SymFacts, SymNames, SymValue,
    TlsBlock,
};
use mortar_core::assignment::Assignment;
use mortar_core::demands::Demands;
use mortar_core::inputs::{CopyError, Reader};
use mortar_core::layout::{Addresses, AllocKey, ChunkKind, DefValue, Layout, SlotIndex};
use mortar_core::par::Pool;
use mortar_core::records::RecIndex;
use mortar_core::resolution::{Def, SectionKind};
use mortar_core::{
    ChunkId, FileId, Inputs, Liveness, Resolution, SectionId, Split, SymEntry, SymKey, SymKind,
    SymbolId,
};
use mortar_diag::{Diagnostic, FileRef, Key, Kind};
use mortar_digest::TreeStyle;
use mortar_elf::{SectionIndex, SymbolIndex, SymbolSection, SymbolTable};
use mortar_merge::order::Order;
use mortar_merge::refs;
use mortar_synth::chunks::SynthKind;
use mortar_synth::eh::CodeRef;
use mortar_synth::got::{GotAddrs, GotPlan, Region, SlotValues};
use mortar_synth::hdr::{HdrAddrs, HdrPlan};
use mortar_synth::image::{EhChunk, GotChunk, GotPart, HdrChunk, SymTable, SymtabChunk};
use mortar_synth::symtab::{InternalValue, SymAddrs, SymCtx, SymtabPlan};
use mortar_synth::{
    BuildIdPlan, Division, EhPlan, EhSource, EhValue, EhWho, PropertyPlan, ShstrtabPlan, SynthChunk,
};

use crate::fill::{self, EhFill, Failed, Filled, Image, MergedFrags, Tables};
use crate::headers::{self, Ehdr, Phdr, Shdr};
use crate::layout_plan::{self, Extras, Model, SynthRow};
use crate::output::{Output, OutputOptions};
use crate::source::RecordKeys;
use crate::target::Target;
use crate::{IMAGE_BLOCK, PHASE_IMAGE, PHASE_OUTPUT};

const SHF_ALLOC: u64 = 0x2;
const SHF_MERGE: u64 = 0x10;
const SHT_NOBITS: u32 = 8;
const SHT_INIT_ARRAY: u32 = 14;
const SHT_FINI_ARRAY: u32 = 15;
const SHT_PREINIT_ARRAY: u32 = 16;
const STT_SECTION: u8 = 3;
const SHN_LORESERVE: u32 = 0xff00;
const SHN_XINDEX: u16 = 0xffff;
const ET_EXEC: u16 = 2;
const ET_DYN: u16 = 3;
const ET_REL: u16 = 1;
const ELFOSABI_GNU: u8 = 3;

/// What the scan knows of a key (`spec/arch.md` §3.4 `SymFacts`), and
/// its name: the driver answers with the scan's own view (`Walker`), so
/// that fill sees the target kind the scan decided on.
pub trait KeyFacts: SymNames {
    fn facts(&self, key: SymKey) -> SymFacts;
}

/// The synthetic chunks' plans (phase 80, re-planned after layout's fixed
/// point) and the chunks that exist (`(kind, ChunkId)`, S-3 order).
#[derive(Clone, Copy)]
pub struct SynthPlans<'a> {
    pub chunks: &'a [(SynthKind, ChunkId)],
    pub hdr: Option<&'a HdrPlan>,
    pub got: &'a GotPlan,
    /// `.symtab`, `.strtab`, `.symtab_shndx` and the context their plan
    /// was made with (`spec/synth.md` §7.1); `None` under `-s`.
    pub symtab: Option<(&'a SymtabPlan, SymCtx<'a>)>,
    pub shstrtab: &'a ShstrtabPlan,
    pub build_id: Option<&'a BuildIdPlan>,
    pub property: &'a PropertyPlan,
}

impl std::fmt::Debug for SynthPlans<'_> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SynthPlans").field("chunks", &self.chunks).finish_non_exhaustive()
    }
}

/// The options the image reads.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LinkOptions {
    pub output: OutputKind,
    /// `-z separate-code` (I-41, L-83).
    pub separate_code: bool,
    /// The build id's tree ([`tree_style`] of `plan.shaping.build_id`);
    /// `None` without a hashed note.
    pub build_id: Option<TreeStyle>,
    /// The output's BTI property is set (a thunk's target must be a
    /// landing pad, `Arch::thunk_target_ok`).
    pub bti: bool,
}

/// The tree of a `--build-id` style (I-72); `None` for a style the
/// sealing step does not hash (`0xHEX`; `md5` and `uuid` are refused by
/// phase 80).
#[must_use]
pub fn tree_style(style: &mortar_core::plan::BuildId) -> Option<TreeStyle> {
    use mortar_core::plan::BuildId;
    match style {
        BuildId::Default => Some(TreeStyle::Default),
        BuildId::Sha1 => Some(TreeStyle::Sha1),
        BuildId::Sha256 => Some(TreeStyle::Sha256),
        BuildId::Fast => Some(TreeStyle::Fast),
        BuildId::Md5 | BuildId::Uuid | BuildId::Hex(_) => None,
    }
}

/// Everything the image phase reads: the front's tables, borrowed.
#[derive(Clone, Copy)]
pub struct Link<'a, 's> {
    pub arch: &'a dyn Arch,
    pub pool: &'a Pool,
    pub inputs: &'a Inputs<'s>,
    pub resolution: &'a Resolution,
    /// The assignment after `place` (the one `Layout::validate` checked).
    pub assignment: &'a Assignment<'s>,
    pub liveness: &'a Liveness,
    /// Phase 60's `Phase::split`.
    pub split: &'a Split,
    /// Phase 50's fragment order (`Front::order`).
    pub order: &'a Order,
    /// The scan's record index (`Scanned::order`).
    pub records: &'a RecIndex,
    /// The scan's demands (`Scanned::demands`).
    pub demands: &'a Demands,
    pub layout: &'a Layout,
    /// The `.eh_frame` plan and the source it was made from (`MergeEh`).
    pub eh: Option<(&'a EhPlan, &'a dyn EhSource)>,
    pub synth: SynthPlans<'a>,
    /// The scan's answers per key.
    pub facts: &'a dyn KeyFacts,
    /// The proof runs of input section `(file, index)` of `len` bytes
    /// (`spec/arch.md` §3.6 rule 1: the scan's `CodeProof`).
    pub proof: &'a (dyn Fn(FileId, SectionIndex, u64) -> Vec<ProofRun> + Sync),
    /// `Store::recheck` of the blob holding `file` (input.md §11.5):
    /// `Err(CopyError::Changed)` for a file that changed since it was
    /// read; `Ok` for one the store does not hold open.
    pub recheck: &'a (dyn Fn(FileId) -> Result<(), CopyError> + Sync),
    pub options: LinkOptions,
    /// Words written over fill's bytes, by address, sorted: the erratum
    /// fixes (`spec/a64.md` §11.2; the front computes them from the words
    /// fill writes at the final addresses).
    pub fixups: &'a [(u64, [u8; 4])],
}

impl std::fmt::Debug for Link<'_, '_> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Link").field("options", &self.options).finish_non_exhaustive()
    }
}

/// What a written link produced.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Written {
    /// The build id written into the note, when one was hashed.
    pub build_id: Option<Vec<u8>>,
    /// The plan's size: rows and jobs (for `--time-phases` and tests).
    pub rows: usize,
    pub jobs: usize,
}

fn internal(what: impl Into<String>) -> Diagnostic {
    Diagnostic::error(Kind::InternalError { what: what.into() })
}

fn failed(d: Diagnostic) -> Failed {
    Failed { diagnostics: vec![(Key::new(PHASE_IMAGE, 0, 0), d)] }
}

// ---- the tables ------------------------------------------------------------

/// One output section as its header gives it (L-86).
#[derive(Debug, Clone, Copy, Default)]
struct OutSection {
    addr: u64,
    offset: u64,
    size: u64,
}

/// The fill tables and the synthetic chunks' answers over one link.
pub struct LinkTables<'a, 's> {
    link: Link<'a, 's>,
    addrs: Addresses<'a, 's>,
    /// Per file: its symbol table, when it is an object with one.
    tables: Box<[Option<SymbolTable<'s>>]>,
    /// Per global symbol: its definition's address (model §4.5, without
    /// `redirect`: the record keys are already redirected, as the scan
    /// decoded them).
    global: Box<[Option<u64>]>,
    got: SlotIndex,
    tls_ie: SlotIndex,
    tls_gd: SlotIndex,
    tls_desc: SlotIndex,
    iplt: SlotIndex,
    got_addrs: GotAddrs,
    /// `.eh_frame`'s section index and address.
    eh_frame: Option<(u32, u64)>,
    /// Per section index (0 is the null section).
    sections: Box<[OutSection]>,
}

impl std::fmt::Debug for LinkTables<'_, '_> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("LinkTables").field("got", &self.got_addrs).finish_non_exhaustive()
    }
}

impl<'a, 's> LinkTables<'a, 's> {
    /// The tables of `link`.
    #[must_use]
    pub fn new(link: Link<'a, 's>) -> Self {
        let addrs = Addresses::new(link.resolution, link.assignment, link.split, link.layout);
        let tables: Box<[Option<SymbolTable<'s>>]> = link
            .inputs
            .files
            .iter()
            .map(|(_, f)| match &f.reader {
                Reader::Object(o) => o.symbols().ok().flatten(),
                _ => None,
            })
            .collect();
        let slots = &link.layout.slots;
        let pl = &link.layout.placement;
        let chunk_addr = |kind: SynthKind| {
            link.synth
                .chunks
                .iter()
                .find(|c| c.0 == kind)
                .and_then(|c| pl.chunks.get(c.1))
                .and_then(|c| c.addr)
        };
        let got_addrs = GotAddrs {
            got: chunk_addr(SynthKind::Got).unwrap_or(0),
            got_plt: chunk_addr(SynthKind::GotPlt).unwrap_or(0),
            plt: chunk_addr(SynthKind::Plt).unwrap_or(0),
        };
        let eh_frame = link
            .synth
            .chunks
            .iter()
            .find(|c| c.0 == SynthKind::EhFrame)
            .and_then(|c| pl.chunks.get(c.1))
            .and_then(|c| Some((c.shndx?, c.addr?)));
        let mut t = Self {
            link,
            addrs,
            tables,
            global: Box::new([]),
            got: SlotIndex::new(&slots.got),
            tls_ie: SlotIndex::new(&slots.tls_ie),
            tls_gd: SlotIndex::new(&slots.tls_gd),
            tls_desc: SlotIndex::new(&slots.tls_desc),
            iplt: SlotIndex::new(&slots.iplt),
            got_addrs,
            eh_frame,
            sections: out_sections(link.layout).into(),
        };
        let n = link.inputs.symbols.len() as usize;
        let global = link.pool.map(n, |i| t.def_addr(SymbolId(i as u32)));
        t.global = global.into();
        t
    }

    /// The addresses of the synthetic chunks `.got`, `.got.plt`, `.plt`.
    #[must_use]
    pub fn got_addrs(&self) -> GotAddrs {
        self.got_addrs
    }

    fn sym_of(&self, e: SymEntry) -> Option<(SymbolTable<'s>, mortar_elf::raw::Sym)> {
        let t = (*self.tables.get(e.file.0 as usize)?)?;
        let s = t.get(e.index).ok()?;
        Some((t, s))
    }

    /// The input section entry `e` is defined in, with its value; `Err`
    /// with the absolute value for `SHN_ABS`.
    fn entry_place(&self, e: SymEntry) -> Option<Result<(SectionId, mortar_elf::raw::Sym), u64>> {
        let (t, s) = self.sym_of(e)?;
        match t.section_of(e.index, &s).ok()? {
            SymbolSection::Absolute => Some(Err(s.st_value)),
            SymbolSection::Section(index) => {
                Some(Ok((self.link.resolution.section_id(e.file, index)?, s)))
            }
            _ => None,
        }
    }

    fn section_size(&self, s: SectionId) -> Option<u64> {
        let (file, index) = self.link.resolution.sh_index(s)?;
        let Reader::Object(o) = &self.link.inputs.files.get(file)?.reader else { return None };
        Some(o.section(index).ok()?.size())
    }

    fn section_flags(&self, s: SectionId) -> u64 {
        let Some((file, index)) = self.link.resolution.sh_index(s) else { return 0 };
        let Some(Reader::Object(o)) = self.link.inputs.files.get(file).map(|f| &f.reader) else {
            return 0;
        };
        o.section(index).map_or(0, |s| s.flags())
    }

    fn kind_of(&self, s: SectionId) -> Option<SectionKind> {
        self.link.resolution.sections.get(s).map(|r| r.kind)
    }

    /// The output address of a named symbol at `v` in section `s`
    /// (model §4.5; S-19/S-22 for a split `Merge` section; S-50 for an
    /// `.eh_frame`).
    fn value_in(&self, s: SectionId, v: u64) -> Option<u64> {
        match self.kind_of(s)? {
            SectionKind::Merge => {
                let size = self.section_size(s)?;
                let p = refs::named_symbol(self.link.split, s, size, v).ok()?;
                self.frag_value(p.frag)?.checked_add(p.within)
            }
            SectionKind::EhFrame => {
                let (plan, src) = self.link.eh?;
                let r = i64::try_from(v).ok()?;
                match plan.value(src, s, r, EhWho::Symbol { droppable: false }).ok()? {
                    EhValue::Offset(o) => self.eh_frame?.1.checked_add(o),
                    EhValue::Dropped => None,
                }
            }
            _ => {
                let (c, at) = self.link.layout.placement.place.get(s)?.get()?;
                Some(self.chunk_at(c, at.checked_add(v)?)?.1)
            }
        }
    }

    /// A live fragment's value (model §4.5): its address, or in a
    /// non-allocated merged chunk its offset from its section's start
    /// (`.debug_str`, the gABI's `sh_addr` 0).
    fn frag_value(&self, frag: mortar_core::FragId) -> Option<u64> {
        let row = self.link.split.frags.get(frag)?;
        let chunk = self.addrs.merged_chunk(self.link.assignment.out(row.section)?)?;
        let offset = self.link.layout.merge.offset(frag)?;
        Some(self.chunk_at(chunk, offset)?.1)
    }

    fn entry_addr(&self, e: SymEntry) -> Option<u64> {
        match self.entry_place(e)? {
            Err(abs) => Some(abs),
            Ok((s, sym)) => self.value_in(s, sym.st_value),
        }
    }

    /// The address of global `id`'s definition (model §4.5 without
    /// `redirect`): weak undefined is 0, strong undefined `None`.
    fn def_addr(&self, id: SymbolId) -> Option<u64> {
        let l = &self.link;
        let pl = &l.layout.placement;
        match l.assignment.def(l.resolution, id)? {
            Def::Undefined { weak: true } => Some(0),
            Def::Undefined { weak: false } => None,
            Def::Defined(e) => self.entry_addr(e),
            Def::Common(_) => self.alloc_addr(AllocKey::Common(id)),
            Def::Shared(_) => {
                let copies = &l.layout.slots.copy_of;
                let at = copies.binary_search_by_key(&id, |row| row.0).ok()?;
                self.alloc_addr(AllocKey::Copy(copies[at].1))
            }
            Def::Internal(_) => pl.defined_of(id),
        }
    }

    fn alloc_addr(&self, key: AllocKey) -> Option<u64> {
        let pl = &self.link.layout.placement;
        let row = pl.sym_place_of(key)?;
        pl.chunks.get(row.chunk)?.addr?.checked_add(row.offset)
    }

    /// The entry defining `key` in a section (a local's own, a global's
    /// regular definition).
    fn def_entry(&self, key: SymKey) -> Option<SymEntry> {
        match key.kind() {
            SymKind::Local(e) => Some(e),
            SymKind::Global(id) => match self.link.assignment.def(self.link.resolution, id)? {
                Def::Defined(e) => Some(e),
                _ => None,
            },
        }
    }

    /// The address a slot holds for `key` (S-66): its definition's, the
    /// resolver's for an IFUNC; `None` for an undefined key.
    fn address(&self, key: SymKey) -> Option<u64> {
        match key.kind() {
            SymKind::Local(e) => self.entry_addr(e),
            SymKind::Global(id) => {
                let l = &self.link;
                if matches!(l.assignment.def(l.resolution, id)?, Def::Undefined { .. }) {
                    return None;
                }
                *self.global.get(id.0 as usize)?
            }
        }
    }

    fn got_slot(&self, region: Region, ix: &SlotIndex, key: SymKey) -> Option<u64> {
        let i = ix.find(key)?;
        Some(self.got_addrs.got + self.link.synth.got.got_offset(region, i))
    }

    fn tls_block(&self) -> Option<TlsBlock> {
        self.link.layout.placement.tls.map(|t| TlsBlock {
            start: t.start,
            size: t.size,
            align: 1u64 << t.align,
        })
    }

    /// The section index and the address base of chunk `c`: its address,
    /// or for a non-alloc chunk its offset from its section's start.
    fn chunk_at(&self, c: ChunkId, offset: u64) -> Option<(u32, u64)> {
        let chunk = self.link.layout.placement.chunks.get(c)?;
        let shndx = match chunk.kind {
            ChunkKind::FileHeader | ChunkKind::ProgramHeaders => 1,
            _ => chunk.shndx?,
        };
        let base = if let Some(a) = chunk.addr {
            a
        } else {
            let sec = self.sections.get(shndx as usize)?;
            chunk.offset.checked_sub(sec.offset)?
        };
        Some((shndx, base.checked_add(offset)?))
    }

    /// The chunk holding input section `s`'s bytes (a `Merge` section's:
    /// its output section's merged chunk).
    fn chunk_of(&self, s: SectionId) -> Option<(ChunkId, u64)> {
        if self.kind_of(s)? == SectionKind::Merge {
            let out = self.link.assignment.out(s)?;
            return Some((self.addrs.merged_chunk(out)?, 0));
        }
        self.link.layout.placement.place.get(s)?.get()
    }
}

impl SymNames for LinkTables<'_, '_> {
    fn name(&self, key: SymKey) -> &[u8] {
        self.link.facts.name(key)
    }
}

impl FillTables for LinkTables<'_, '_> {
    fn sym(&self, key: SymKey) -> SymValue {
        let facts = self.link.facts.facts(key);
        let entry = self.def_entry(key);
        let size = entry.and_then(|e| self.sym_of(e)).map_or(0, |(_, s)| s.st_size);
        let (s, live) = match key.kind() {
            SymKind::Local(e) => {
                let live = !facts.dead;
                (if live { self.entry_addr(e) } else { None }, live)
            }
            SymKind::Global(id) => {
                let live = match entry.and_then(|e| self.entry_place(e)) {
                    Some(Ok((sec, _))) => self.link.liveness.is_live(sec),
                    _ => true,
                };
                (self.global.get(id.0 as usize).copied().flatten(), live)
            }
        };
        // S-71: every address of an IFUNC with an IPLT entry is the entry.
        let s = self.plt(key).or(s).unwrap_or(0);
        SymValue { s, kind: facts.kind, size, live, tls: facts.tls }
    }

    fn slot(&self, key: SymKey, kind: SlotKind) -> Option<u64> {
        match kind {
            SlotKind::Addr => self.got_slot(Region::Addr, &self.got, key),
            SlotKind::TpOff { .. } => self.got_slot(Region::TpOff, &self.tls_ie, key),
            SlotKind::TlsGd => self.got_slot(Region::TlsGd, &self.tls_gd, key),
            SlotKind::TlsDesc => self.got_slot(Region::TlsDesc, &self.tls_desc, key),
            SlotKind::TlsLd => self
                .link
                .layout
                .slots
                .tls_ld
                .then(|| self.got_addrs.got + self.link.synth.got.got_offset(Region::TlsLd, 0)),
            SlotKind::PltGot => {
                let n = self.iplt.find(key)?;
                Some(self.got_addrs.got_plt + self.link.synth.got.iplt_word(n))
            }
        }
    }

    fn plt(&self, key: SymKey) -> Option<u64> {
        self.link.synth.got.iplt_address(&self.got_addrs, key)
    }

    fn flags(&self, key: SymKey) -> DemandFlags {
        self.link.demands.of(key)
    }

    fn redirect(&self, section: SectionId, offset: u64) -> Option<u64> {
        let pl = &self.link.layout.placement;
        let patch = pl.redirect_of(section, offset)?;
        pl.chunks.get(patch.chunk)?.addr?.checked_add(patch.offset)
    }
}

impl Tables for LinkTables<'_, '_> {
    fn target_alloc(&self, key: SymKey) -> bool {
        match self.def_entry(key).and_then(|e| self.entry_place(e)) {
            Some(Ok((s, _))) => self.section_flags(s) & SHF_ALLOC != 0,
            Some(Err(_)) => false,
            // Commons, linker-defined symbols: in alloc chunks.
            None => true,
        }
    }

    fn split_target(&self, key: SymKey, addend: i64) -> Option<Result<u64, Diagnostic>> {
        let e = self.def_entry(key)?;
        let Ok((s, sym)) = self.entry_place(e)? else { return None };
        let section_sym = sym.sym_type() == STT_SECTION;
        let malformed = |what: String| Diagnostic::error(Kind::Malformed { what });
        match self.kind_of(s)? {
            SectionKind::Merge if section_sym => {
                let size = self.section_size(s)?;
                let r = (sym.st_value as i64).wrapping_add(addend);
                Some(match refs::section_symbol(self.link.split, s, size, r) {
                    Ok(p) => self
                        .frag_value(p.frag)
                        .and_then(|a| a.checked_add(p.within))
                        .ok_or_else(|| internal("fill: a live merge piece has no address")),
                    Err(w) => Err(malformed(w.message())),
                })
            }
            SectionKind::EhFrame => {
                let Some((plan, src)) = self.link.eh else {
                    return Some(Err(internal(
                        "fill: a reference into .eh_frame without its plan",
                    )));
                };
                let Some((_, eh)) = self.eh_frame else {
                    return Some(Err(internal(
                        "fill: a reference into .eh_frame without the chunk",
                    )));
                };
                let r = if section_sym {
                    (sym.st_value as i64).wrapping_add(addend)
                } else {
                    sym.st_value as i64
                };
                let tail = if section_sym { 0 } else { addend };
                Some(match plan.value(src, s, r, EhWho::Reloc) {
                    Ok(EhValue::Offset(o)) => Ok(eh.wrapping_add(o).wrapping_add(tail as u64)),
                    Ok(EhValue::Dropped) => Err(internal("fill: a relocation to a dropped symbol")),
                    Err(e) => Err(e.into_diagnostic()),
                })
            }
            _ => None,
        }
    }
}

impl RecordKeys for LinkTables<'_, '_> {
    /// As the scan decodes a record (`mortar_scan`'s `key_of`; model
    /// §3.3: `redirect` applies to undefined mentions).
    fn key(&self, file: FileId, r_sym: u32) -> Option<SymKey> {
        let local = |index| SymKey::local(SymEntry { file, index: SymbolIndex(index) });
        let Some(t) = self.tables.get(file.0 as usize).copied().flatten() else {
            return (r_sym == 0).then(|| local(0));
        };
        if r_sym >= t.len() {
            return None;
        }
        let first = t.first_global().0;
        if r_sym < first {
            return Some(local(r_sym));
        }
        let id = *self.link.inputs.sym_map_of(file).get((r_sym - first) as usize)?;
        let to = self.link.resolution.redirect.get(id).copied().unwrap_or(id);
        if to != id {
            let s = t.get(SymbolIndex(r_sym)).ok()?;
            if t.section_of(SymbolIndex(r_sym), &s).ok()? == SymbolSection::Undefined {
                return Some(SymKey::global(to));
            }
        }
        Some(SymKey::global(id))
    }
}

impl SlotValues for LinkTables<'_, '_> {
    fn address(&self, key: SymKey) -> Option<u64> {
        LinkTables::address(self, key)
    }

    fn tls(&self) -> Option<TlsBlock> {
        self.tls_block()
    }
}

impl SymAddrs for LinkTables<'_, '_> {
    fn section(&self, section: SectionId, value: u64) -> Option<(u32, u64)> {
        let (c, at) = self.chunk_of(section)?;
        let (shndx, base) = self.chunk_at(c, 0)?;
        let addr = match self.kind_of(section)? {
            SectionKind::Merge => self.value_in(section, value)?,
            _ if self.link.layout.placement.chunks.get(c)?.addr.is_some() => {
                self.addrs.addr_in(section, value)?
            }
            _ => base.checked_add(at)?.checked_add(value)?,
        };
        Some((shndx, addr))
    }

    fn eh_frame(&self) -> Option<(u32, u64)> {
        self.eh_frame
    }

    fn common(&self, id: SymbolId) -> Option<(u32, u64)> {
        let row = self.link.layout.placement.sym_place_of(AllocKey::Common(id))?;
        self.chunk_at(row.chunk, row.offset)
    }

    fn internal(&self, id: SymbolId) -> Option<InternalValue> {
        let pl = &self.link.layout.placement;
        let at = pl.defined.binary_search_by_key(&id, |row| row.symbol).ok()?;
        Some(match pl.defined[at].value {
            DefValue::Absolute(v) => InternalValue::Absolute(v),
            DefValue::At { chunk, offset } => {
                let (shndx, addr) = self.chunk_at(chunk, offset)?;
                InternalValue::At { shndx, addr }
            }
        })
    }

    fn patch(&self, index: usize) -> Option<(u32, u64)> {
        let p = self.link.layout.placement.patches.get(index)?;
        self.chunk_at(p.chunk, p.offset)
    }

    fn tls_start(&self) -> Option<u64> {
        self.link.layout.placement.tls.map(|t| t.start)
    }

    fn section_range(&self, shndx: u32) -> Option<(u64, u64)> {
        let s = self.sections.get(shndx as usize)?;
        (shndx > 0).then_some((s.addr, s.size))
    }
}

// ---- the structural units (I-40) -------------------------------------------

/// The output sections by index (L-86): the first chunk's address and
/// offset, the size to the end of the last chunk's `size`.
fn out_sections(layout: &Layout) -> Vec<OutSection> {
    let mut rows: Vec<Option<OutSection>> = Vec::new();
    let mut ends: Vec<u64> = Vec::new();
    for (_, c) in layout.placement.chunks.iter() {
        let Some(n) = c.shndx else { continue };
        let n = n as usize;
        if rows.len() <= n {
            rows.resize(n + 1, None);
            ends.resize(n + 1, 0);
        }
        let start = c.addr.unwrap_or(c.offset);
        let end = start + c.size;
        match &mut rows[n] {
            Some(s) => {
                let first = s.addr_or_offset(c.addr.is_some());
                if start < first {
                    s.addr = c.addr.unwrap_or(0);
                    s.offset = c.offset;
                }
                ends[n] = ends[n].max(end);
                let first = s.addr_or_offset(c.addr.is_some());
                s.size = ends[n] - first;
            }
            slot @ None => {
                *slot =
                    Some(OutSection { addr: c.addr.unwrap_or(0), offset: c.offset, size: c.size });
                ends[n] = end;
            }
        }
    }
    rows.into_iter().map(Option::unwrap_or_default).collect()
}

impl OutSection {
    fn addr_or_offset(&self, alloc: bool) -> u64 {
        if alloc { self.addr } else { self.offset }
    }
}

/// The bytes of the `Patches` chunks with file bytes (I-39), by chunk:
/// each thunk written by `Arch::write_thunk` at its offset, with its
/// target `T` as layout's fixed point computed it (`S + A` of the key, the
/// IPLT entry of an IFUNC; a `Plt` target's entry), and zero between
/// patches.
///
/// An erratum patch is left zero here: its words are in `Link::fixups`.
///
/// # Errors
///
/// The refusal of a thunk target (BTI); `internal-error` when a patch lies outside its chunk or
/// a thunk cannot be encoded.
fn patches(
    link: &Link<'_, '_>,
    tables: &LinkTables<'_, '_>,
) -> Result<std::collections::BTreeMap<ChunkId, Box<[u8]>>, Diagnostic> {
    use mortar_core::layout::{PatchKind, ThunkTarget};
    let pl = &link.layout.placement;
    let mut out: std::collections::BTreeMap<ChunkId, Box<[u8]>> = std::collections::BTreeMap::new();
    for (id, c) in pl.chunks.iter() {
        if matches!(c.kind, ChunkKind::Patches(_)) && c.in_file() && c.size > 0 {
            let size = usize::try_from(c.size)
                .map_err(|_| internal("image: a patch island larger than memory"))?;
            out.insert(id, vec![0u8; size].into_boxed_slice());
        }
    }
    for p in &pl.patches {
        let target = match p.kind {
            PatchKind::Thunk(ThunkTarget::Plt(key)) => tables.plt(key),
            PatchKind::Thunk(ThunkTarget::Sym { key, addend }) => {
                Some(tables.sym(key).s.wrapping_add_signed(addend))
            }
            // Written by `Link::fixups` (its words are fill's).
            PatchKind::Erratum { .. } => continue,
        };
        let Some(target) = target else {
            return Err(internal(format!("image: a thunk at chunk {} has no target", p.chunk.0)));
        };
        let Some(shape) = link.arch.thunk() else {
            return Err(internal("image: a thunk for an architecture without thunks"));
        };
        let Some(addr) = pl.chunks.get(p.chunk).and_then(|c| c.addr?.checked_add(p.offset)) else {
            return Err(internal(format!("image: a thunk in chunk {} has no address", p.chunk.0)));
        };
        let cx = mortar_arch::PatchCx {
            addr,
            target,
            site: 0,
            word: 0,
            bti: link.options.bti,
            target_word: None,
            target_abs: false,
        };
        link.arch.thunk_target_ok(&cx).map_err(|r| r.diagnostic())?;
        let bytes = out.get_mut(&p.chunk).ok_or_else(|| {
            internal(format!("image: a thunk in chunk {}, not a patch island", p.chunk.0))
        })?;
        let range = usize::try_from(p.offset)
            .ok()
            .and_then(|s| Some(s..s.checked_add(usize::try_from(shape.size).ok()?)?))
            .filter(|r| r.end <= bytes.len())
            .ok_or_else(|| internal(format!("image: a thunk past its island {}", p.chunk.0)))?;
        link.arch.write_thunk(&cx, &mut bytes[range]).map_err(|e| {
            internal(format!("image: the thunk at {addr:#x} to {target:#x}: {e:?}"))
        })?;
    }
    Ok(out)
}

/// `Link::fixups` at file offsets, sorted: each word's address in the
/// in-file alloc chunk that holds it.
///
/// # Errors
///
/// `internal-error` for a word in no such chunk.
fn file_fixups(link: &Link<'_, '_>) -> Result<Vec<(u64, [u8; 4])>, Diagnostic> {
    let pl = &link.layout.placement;
    let mut spans: Vec<(u64, u64, u64)> = pl
        .chunks
        .iter()
        .filter(|(_, c)| c.in_file() && c.flags & SHF_ALLOC != 0)
        .filter_map(|(_, c)| Some((c.addr?, c.size, c.offset)))
        .collect();
    spans.sort_unstable();
    let mut out = Vec::with_capacity(link.fixups.len());
    for &(addr, word) in link.fixups {
        let at = spans.partition_point(|s| s.0 <= addr);
        let off = at
            .checked_sub(1)
            .map(|i| spans[i])
            .filter(|&(a, size, _)| addr + 4 <= a + size)
            .map(|(a, _, off)| off + (addr - a))
            .ok_or_else(|| internal(format!("image: a fixed word at {addr:#x} is in no chunk")))?;
        out.push((off, word));
    }
    out.sort_unstable_by_key(|f| f.0);
    Ok(out)
}

/// The bytes of the three structural chunks (I-40): the ELF header, the
/// program header table and the section header table, by chunk.
///
/// # Errors
///
/// `internal-error` when the placement's header chunks do not have the
/// size of what they hold, or a section's header cannot be built.
pub fn structure(
    link: &Link<'_, '_>,
) -> Result<std::collections::BTreeMap<ChunkId, Box<[u8]>>, Diagnostic> {
    let facts = link.arch.facts();
    let class = facts.class;
    let pl = &link.layout.placement;
    let find =
        |kind: ChunkKind| pl.chunks.iter().find(|(_, c)| c.kind == kind).map(|(id, c)| (id, *c));
    let (eh_id, eh_chunk) = find(ChunkKind::FileHeader)
        .ok_or_else(|| internal("image: no file header chunk (I-40)"))?;
    let ph = find(ChunkKind::ProgramHeaders);
    let sh = find(ChunkKind::SectionHeaders);
    let phdrs: Vec<Phdr> = pl
        .segments
        .iter()
        .map(|(_, s)| Phdr {
            p_type: s.p_type,
            p_flags: s.flags,
            p_offset: s.offset,
            p_vaddr: s.addr,
            p_paddr: s.addr,
            p_filesz: s.filesz,
            p_memsz: s.memsz,
            p_align: 1u64 << s.align,
        })
        .collect();
    let shdrs = section_headers(link)?;
    let shnum = shdrs.len() as u32;
    let shstrndx = synth_shndx(link, SynthKind::Shstrtab).unwrap_or(0);
    let e_type = match link.options.output {
        OutputKind::Static | OutputKind::Exec => ET_EXEC,
        OutputKind::Pie | OutputKind::Shared => ET_DYN,
        OutputKind::Relocatable => ET_REL,
    };
    let mut shdrs = shdrs;
    // gABI extended numbering: the count and the string table's index in
    // section header 0 when they do not fit `e_shnum`/`e_shstrndx`.
    let (e_shnum, e_shstrndx) = {
        let num = if shnum >= SHN_LORESERVE {
            if let Some(h0) = shdrs.first_mut() {
                h0.sh_size = u64::from(shnum);
            }
            0
        } else {
            shnum as u16
        };
        let str_ndx = if shstrndx >= SHN_LORESERVE {
            if let Some(h0) = shdrs.first_mut() {
                h0.sh_link = shstrndx;
            }
            SHN_XINDEX
        } else {
            shstrndx as u16
        };
        (num, str_ndx)
    };
    let phnum = u16::try_from(phdrs.len())
        .map_err(|_| internal(format!("image: {} program headers", phdrs.len())))?;
    let e_flags = link.arch.e_flags(&mut live_flags(link)).map_err(|r| r.diagnostic())?;
    // `EI_OSABI` (a64.md Q11, ruled by WP-027c): `ELFOSABI_GNU` when the
    // output uses a GNU extension of the gABI: an `STT_GNU_IFUNC` or
    // `STB_GNU_UNIQUE` symbol, or an `IRELATIVE` record (GNU ld's rule;
    // eu-elflint rejects the IFUNC symbol under 0); else the target's.
    let gnu = link.synth.symtab.is_some_and(|(p, _)| p.gnu_symbols())
        || !link.layout.slots.iplt.is_empty();
    let ehdr = Ehdr {
        osabi: if gnu { ELFOSABI_GNU } else { facts.osabi },
        e_type,
        e_machine: facts.e_machine,
        e_entry: pl.entry.unwrap_or(0),
        e_phoff: ph.map_or(0, |p| p.1.offset),
        e_shoff: sh.map_or(0, |s| s.1.offset),
        e_flags,
        e_phnum: phnum,
        e_shnum,
        e_shstrndx,
    };
    let mut out = std::collections::BTreeMap::new();
    let mut put = |id: ChunkId, size: u64, bytes: Vec<u8>, what: &str| {
        if bytes.len() as u64 != size {
            return Err(internal(format!(
                "image: the {what} chunk has {size} bytes for {} (I-40)",
                bytes.len()
            )));
        }
        out.insert(id, bytes.into_boxed_slice());
        Ok(())
    };
    put(eh_id, eh_chunk.size, headers::ehdr(class, &ehdr), "file header")?;
    if let Some((id, c)) = ph {
        put(id, c.size, headers::phdrs(class, &phdrs), "program header")?;
    }
    if let Some((id, c)) = sh {
        put(id, c.size, headers::shdrs(class, &shdrs), "section header")?;
    }
    Ok(out)
}

/// The live objects' `e_flags` (`spec/arch.md` §3.1 `e_flags`).
fn live_flags<'l>(link: &'l Link<'_, '_>) -> impl Iterator<Item = u32> + 'l {
    link.inputs.files.iter().filter_map(|(file, f)| {
        let Reader::Object(o) = &f.reader else { return None };
        link.resolution.file_live.get(file).is_some_and(|l| l.is_live()).then(|| o.header().e_flags)
    })
}

fn synth_shndx(link: &Link<'_, '_>, kind: SynthKind) -> Option<u32> {
    let id = link.synth.chunks.iter().find(|c| c.0 == kind)?.1;
    link.layout.placement.chunks.get(id)?.shndx
}

/// The section header table (I-40, L-86): the null header, then one per
/// output section in index order.
fn section_headers(link: &Link<'_, '_>) -> Result<Vec<Shdr>, Diagnostic> {
    let pl = &link.layout.placement;
    let facts = link.arch.facts();
    let class = facts.class;
    let word = u64::from(facts.word);
    let rel = matches!(facts.rel_form, mortar_arch::RelForm::Rel);
    let secs = out_sections(link.layout);
    let mut rows: Vec<Option<Shdr>> = vec![None; secs.len().max(1)];
    rows[0] = Some(Shdr::default());
    let kind_of = |id: ChunkId| link.synth.chunks.iter().find(|c| c.1 == id).map(|c| c.0);
    for (id, c) in pl.chunks.iter() {
        let Some(n) = c.shndx else { continue };
        let n = n as usize;
        let sec = secs[n];
        let row = rows[n].get_or_insert(Shdr {
            sh_name: link.synth.shstrtab.sh_name(n as u32).ok_or_else(|| {
                internal(format!("image: section {n} has no name in .shstrtab (S-90)"))
            })?,
            sh_type: c.sh_type,
            sh_flags: c.flags,
            sh_addr: sec.addr,
            sh_offset: sec.offset,
            sh_size: sec.size,
            sh_link: 0,
            sh_info: 0,
            sh_addralign: 1,
            sh_entsize: 0,
        });
        row.sh_addralign = row.sh_addralign.max(1u64 << c.align);
        match c.kind {
            // Layout's own chunk (L-74's `.relro_padding`): NOBITS, no plan.
            ChunkKind::Synthetic(s) if s == mortar_core::layout::Synth::RELRO_PADDING => {}
            ChunkKind::Synthetic(_) => {
                let Some(kind) = kind_of(id) else {
                    return Err(internal(format!("image: synthetic chunk {} has no plan", id.0)));
                };
                if let Some(h) = kind.header(class, rel) {
                    row.sh_entsize = h.entsize;
                }
                match kind {
                    SynthKind::Symtab => {
                        row.sh_link = synth_shndx(link, SynthKind::Strtab).unwrap_or(0);
                        row.sh_info = link.synth.symtab.map_or(0, |s| s.0.sh_info());
                    }
                    SynthKind::SymtabShndx => {
                        row.sh_link = synth_shndx(link, SynthKind::Symtab).unwrap_or(0);
                    }
                    // S-69: `sh_link` 0 in a static output, `sh_info` `.got.plt`.
                    SynthKind::RelaPlt => {
                        row.sh_info = synth_shndx(link, SynthKind::GotPlt).unwrap_or(0);
                    }
                    _ => {}
                }
            }
            kind => {
                // L-50: the merge class's entsize when `SHF_MERGE` is kept;
                // the gABI's pointer arrays have entries of a word.
                if row.sh_flags & SHF_MERGE != 0 {
                    let class = kind.out().and_then(|o| link.assignment.outsecs.get(o)?.merge);
                    row.sh_entsize = class.map_or(0, |m| m.entsize);
                } else if matches!(row.sh_type, SHT_INIT_ARRAY | SHT_FINI_ARRAY | SHT_PREINIT_ARRAY)
                {
                    row.sh_entsize = word;
                }
            }
        }
    }
    rows.into_iter()
        .enumerate()
        .map(|(n, r)| {
            r.ok_or_else(|| internal(format!("image: section index {n} has no chunk (L-86)")))
        })
        .collect()
}

// ---- the phase -------------------------------------------------------------

/// The synthetic chunks of a link, as fill drives them (§13).
struct Chunks<'a> {
    eh: Option<EhChunk<'a>>,
    hdr: Option<HdrChunk<'a>>,
    got: [GotChunk<'a>; 4],
    symtab: Option<[SymtabChunk<'a>; 3]>,
}

impl<'a> Chunks<'a> {
    fn new(link: &Link<'a, '_>) -> Self {
        let got = |part| GotChunk { plan: link.synth.got, part };
        Self {
            eh: link.eh.map(|(plan, src)| EhChunk { plan, src }),
            hdr: match (link.synth.hdr, link.eh) {
                (Some(plan), Some((eh, src))) => Some(HdrChunk { plan, eh, src }),
                _ => None,
            },
            got: [
                got(GotPart::Got),
                got(GotPart::GotPlt),
                got(GotPart::Plt),
                got(GotPart::RelaPlt),
            ],
            symtab: link.synth.symtab.map(|(plan, cx)| {
                let t = |table| SymtabChunk { plan, cx, table };
                [t(SymTable::Symtab), t(SymTable::Strtab), t(SymTable::SymtabShndx)]
            }),
        }
    }

    fn get(&self, link: &Link<'a, '_>, kind: SynthKind) -> Option<&(dyn SynthChunk + Sync + 'a)> {
        Some(match kind {
            SynthKind::EhFrame => self.eh.as_ref()?,
            SynthKind::EhFrameHdr => self.hdr.as_ref()?,
            SynthKind::Got => &self.got[0],
            SynthKind::GotPlt => &self.got[1],
            SynthKind::Plt => &self.got[2],
            SynthKind::RelaPlt => &self.got[3],
            SynthKind::Symtab => &self.symtab.as_ref()?[0],
            SynthKind::Strtab => &self.symtab.as_ref()?[1],
            SynthKind::SymtabShndx => &self.symtab.as_ref()?[2],
            SynthKind::Shstrtab => link.synth.shstrtab,
            SynthKind::NoteGnuBuildId => link.synth.build_id?,
            SynthKind::NoteGnuProperty => link.synth.property,
            _ => return None,
        })
    }
}

/// Writes the output file of `link` at `path` (§5, §7): the structural
/// units, every placed input section, merged chunk and synthetic chunk,
/// the gaps, the build id; then `input-changed` for an input that changed
/// since it was read (input.md §11.5); then the rename. `on_phase` is
/// called as phase 100 (`image`) and phase 105 (`output`) end, for
/// `--time-phases`.
///
/// A failed link leaves no output file (I-65): the temporary is removed
/// and an existing output is untouched.
///
/// # Errors
///
/// Every diagnostic of the phase, keyed (I-43, I-64, I-68).
#[allow(clippy::too_many_lines)]
pub fn write(
    link: &Link<'_, '_>,
    path: &Path,
    opts: &OutputOptions,
    on_phase: &mut dyn FnMut(u16, &str),
) -> Result<Written, Failed> {
    let tables = LinkTables::new(*link);
    let mut fixed = structure(link).map_err(failed)?;
    fixed.append(&mut patches(link, &tables).map_err(failed)?);
    let chunks = Chunks::new(link);
    let pl = &link.layout.placement;
    let mut list: Vec<&(dyn SynthChunk + Sync)> = Vec::new();
    let mut synth = std::collections::BTreeMap::new();
    let mut sealed = None;
    let mut eh_addr = None;
    let mut hdr_addr = 0;
    for &(kind, id) in link.synth.chunks {
        let Some(chunk) = chunks.get(link, kind) else {
            return Err(failed(internal(format!(
                "image: the chunk {} has no plan (§13)",
                String::from_utf8_lossy(kind.name(false))
            ))));
        };
        let Some(c) = pl.chunks.get(id) else {
            return Err(failed(internal(format!("image: no chunk {} (§13)", id.0))));
        };
        let whole = matches!(chunk.division(), Division::Whole { .. });
        let eh = kind == SynthKind::EhFrame;
        if eh {
            eh_addr = c.addr;
        }
        if kind == SynthKind::EhFrameHdr {
            hdr_addr = c.addr.unwrap_or(0);
        }
        if let Some(s) = chunk.sealed() {
            sealed = Some(c.offset + s.start..c.offset + s.end);
        }
        synth.insert(id, SynthRow { index: list.len() as u32, whole, eh });
        list.push(chunk);
    }
    let extras = Extras {
        fixed,
        synth,
        proof: link.proof,
        separate_code: link.options.separate_code,
        sealed: sealed.clone(),
    };
    let model = Model {
        inputs: link.inputs,
        resolution: link.resolution,
        liveness: link.liveness,
        split: link.split,
        layout: link.layout,
        assignment: link.assignment,
        order: Some(link.order),
        eh: link.eh.map(|e| e.1),
    };
    let target = Target::from_facts(link.arch.facts());
    let plan = layout_plan::from_layout(&model, target, &extras, IMAGE_BLOCK, link.pool)
        .map_err(failed)?;
    let output_ref = FileRef::new(path.display().to_string());
    let code_addr = |c: CodeRef| -> Option<u64> {
        let off = u64::try_from(c.offset).ok()?;
        tables.addrs.addr_in(c.section, off)
    };
    let got_base = match link.arch.facts().got_base {
        mortar_arch::GotBase::GotStart => {
            (tables.got_addrs.got != 0).then_some(tables.got_addrs.got)
        }
        mortar_arch::GotBase::GotPltStart => {
            (tables.got_addrs.got_plt != 0).then_some(tables.got_addrs.got_plt)
        }
    };
    let build_id = if sealed.is_some() { link.options.build_id } else { None };
    let fixups = file_fixups(link).map_err(failed)?;
    let img = Image {
        plan: &plan,
        source: link.inputs,
        arch: link.arch,
        tables: &tables,
        records: link.records,
        keys: &tables,
        output: link.options.output,
        got_base,
        tls: tables.tls_block(),
        chunks: &list,
        synth: mortar_synth::FillCx {
            arch: link.arch,
            pool: link.pool,
            output: &output_ref,
            slots: &tables,
            syms: &tables,
            got: tables.got_addrs,
            hdr: HdrAddrs { hdr: hdr_addr, eh: eh_addr.unwrap_or(0) },
            code_addr: &code_addr,
        },
        merged: Some(MergedFrags { order: link.order, split: link.split }),
        eh: match (link.eh, eh_addr) {
            (Some((plan, src)), Some(addr)) => Some(EhFill { plan, src, addr }),
            _ => None,
        },
        build_id,
        fixups: &fixups,
    };
    let out = Output::create(path, opts).map_err(|kd| Failed { diagnostics: vec![kd] })?;
    let filled: Filled = match fill::write(&img, &out, out.shown(), link.pool) {
        Ok(f) => f,
        Err(e) => {
            out.abort();
            return Err(e);
        }
    };
    // input.md §11.5: the files the store still holds, once more.
    let mut changed = Vec::new();
    for (file, _) in link.inputs.files.iter() {
        if let Err(e) = (link.recheck)(file) {
            let path = layout_plan::file_ref(link.inputs, file);
            changed.push((Key::new(PHASE_IMAGE, file.0, 0), crate::source::copy_error(e, &path)));
        }
    }
    if !changed.is_empty() {
        out.abort();
        return Err(Failed { diagnostics: changed });
    }
    on_phase(PHASE_IMAGE, "image");
    out.commit().map_err(|kd| Failed { diagnostics: vec![kd] })?;
    on_phase(PHASE_OUTPUT, "output file");
    Ok(Written { build_id: filled.build_id, rows: plan.rows.len(), jobs: plan.jobs.len() })
}

// ---- the map (§9.2) --------------------------------------------------------

/// The map of a link (I-77, I-78; `spec/plumb.md` §3.2), from the tables
/// alone (I-79: independent of fill). Not yet listed: folds (M2).
#[must_use]
pub fn map_of(link: &Link<'_, '_>) -> crate::map::Map {
    LinkTables::new(*link).map()
}

impl LinkTables<'_, '_> {
    #[allow(clippy::too_many_lines)]
    fn map(&self) -> crate::map::Map {
        use crate::map::{self, DiscardWhy, MapChunk, MapFile, NonAlloc, Placed};
        use mortar_core::resolution::{DropReason, SectionState};
        let l = &self.link;
        let pl = &l.layout.placement;
        let mut m = map::Map::default();
        let mut ids: Vec<Option<usize>> = vec![None; l.inputs.files.len() as usize];
        for (file, row) in l.inputs.files.iter() {
            if file == FileId::INTERNAL
                || !l.resolution.file_live.get(file).is_some_and(|x| x.is_live())
            {
                continue;
            }
            let Reader::Object(_) = row.reader else { continue };
            let id = m.files.len();
            ids[file.0 as usize] = Some(id);
            m.files.push(MapFile {
                path: row.path.to_vec(),
                member: row.member.as_ref().map(|mm| (mm.name.to_vec(), mm.offset)),
            });
            let mut placed = Vec::new();
            let mut nonalloc = Vec::new();
            let mut discards = Vec::new();
            let range = l.resolution.section_base[file];
            for i in 1..range.len() {
                let sid = SectionId(range.start.0 + i);
                let Some(r) = l.resolution.sections.get(sid) else { continue };
                let size = self.section_size(sid).unwrap_or(0);
                let bytes = size > 0 && r.kind != SectionKind::Nobits;
                let why = match r.state {
                    SectionState::Dropped(DropReason::Consumed) => continue,
                    SectionState::Dropped(DropReason::Comdat) => Some(DiscardWhy::Comdat),
                    SectionState::Dropped(DropReason::Script) => Some(DiscardWhy::Script),
                    SectionState::Dropped(DropReason::Excluded | DropReason::Stripped) => {
                        Some(DiscardWhy::Unspecified)
                    }
                    SectionState::Kept if !l.liveness.is_live(sid) => Some(DiscardWhy::Gc),
                    SectionState::Kept => None,
                };
                if let Some(why) = why {
                    if bytes {
                        discards.push((i, why));
                    }
                    continue;
                }
                // Consumed by the link (layout §8.1): no output section.
                let Some(o) = l.assignment.out(sid) else { continue };
                let Some(out) = l.assignment.outsecs.get(o) else { continue };
                let alloc = out.flags & SHF_ALLOC != 0;
                match (r.kind, alloc) {
                    // Its pieces come from the plan's records, below.
                    (SectionKind::EhFrame, _) => {}
                    (SectionKind::Merge, true) => {
                        let Some(span) = l.split.merge_pieces(sid) else { continue };
                        let offs = &l.split.piece_off[span.slots()];
                        for (k, &off) in offs.iter().enumerate() {
                            let end = offs.get(k + 1).map_or(size, |&e| u64::from(e));
                            let frag = l.split.piece_frag[span.start as usize + k];
                            let Some(addr) = self.frag_value(frag) else { continue };
                            placed.push(Placed::Piece {
                                section: i,
                                input_offset: u64::from(off),
                                addr,
                                size: end - u64::from(off),
                            });
                        }
                    }
                    (SectionKind::Merge, false) => {
                        nonalloc.push(NonAlloc::Merged { section: i, out: out.name.to_vec() });
                    }
                    (_, true) => {
                        if let Some(addr) = pl.section_addr(sid, 0) {
                            placed.push(Placed::Place { section: i, addr, size });
                        }
                    }
                    (_, false) => {
                        let Some((c, at)) = pl.place.get(sid).and_then(|p| p.get()) else {
                            continue;
                        };
                        if let Some((_, offset)) = self.chunk_at(c, at) {
                            nonalloc.push(NonAlloc::Place {
                                section: i,
                                out: out.name.to_vec(),
                                offset,
                                size,
                            });
                        }
                    }
                }
            }
            m.placed.push(placed);
            m.nonalloc.push(nonalloc);
            m.discards.push(discards);
        }
        // `.eh_frame`'s kept records, as pieces of their input sections.
        if let (Some((plan, src)), Some((_, base))) = (l.eh, self.eh_frame) {
            let frames = src.frames();
            for n in 0..plan.records().len() {
                let (srow, g) = plan.source(n);
                let Some(sec) = frames.sections.get(srow) else { continue };
                let Some(rec) = frames.records.get(g as usize) else { continue };
                let Some((file, index)) = l.resolution.sh_index(sec.section) else { continue };
                let Some(id) = ids.get(file.0 as usize).copied().flatten() else { continue };
                let off = plan.out_offset(n);
                m.placed[id].push(Placed::Piece {
                    section: index.0,
                    input_offset: u64::from(rec.offset),
                    addr: base + off,
                    size: plan.out_offset(n + 1) - off,
                });
            }
        }
        // Each file's records in output order (I-78).
        for p in &mut m.placed {
            p.sort_by_key(|r| match *r {
                Placed::Place { addr, section, .. } => (addr, section, 0),
                Placed::Piece { addr, section, input_offset, .. } => (addr, section, input_offset),
            });
        }
        for (_, c) in pl.chunks.iter() {
            if let ChunkKind::Synthetic(s) = c.kind
                && c.size > 0
                && c.sh_type != SHT_NOBITS
            {
                m.chunks.push(MapChunk {
                    kind: map::ChunkKind::Synthetic,
                    name: s.name.to_vec(),
                    addr: c.addr,
                    size: c.size,
                    target: None,
                });
            }
        }
        // Range extension's thunks and erratum patches (I-78), in patch
        // order, named as `.symtab` names them (S-80).
        let thunk_size = l.arch.thunk().map_or(0, |t| u64::from(t.size));
        for p in &pl.patches {
            use mortar_core::layout::{PatchKind, ThunkTarget};
            let addr = pl.chunks.get(p.chunk).and_then(|c| c.addr?.checked_add(p.offset));
            let chunk = match p.kind {
                PatchKind::Thunk(t) => {
                    let (key, addend, plt) = match t {
                        ThunkTarget::Sym { key, addend } => (key, addend, false),
                        ThunkTarget::Plt(key) => (key, 0, true),
                    };
                    let mut target = l.facts.name(key).to_vec();
                    if plt {
                        target.extend_from_slice(b"@plt");
                    }
                    let mut name = b"__".to_vec();
                    name.extend_from_slice(&target);
                    if addend > 0 {
                        name.extend_from_slice(format!("+{addend:#x}").as_bytes());
                    } else if addend < 0 {
                        name.extend_from_slice(format!("-{:#x}", addend.unsigned_abs()).as_bytes());
                    }
                    name.extend_from_slice(b"_veneer");
                    MapChunk {
                        kind: map::ChunkKind::Thunk,
                        name,
                        addr,
                        size: thunk_size,
                        target: Some((target, addend)),
                    }
                }
                PatchKind::Erratum { erratum, .. } => MapChunk {
                    kind: map::ChunkKind::Patch,
                    name: mortar_synth::symtab::ERRATUM_843419.to_vec(),
                    addr,
                    size: l.arch.patch(erratum).map_or(0, |s| u64::from(s.size)),
                    target: None,
                },
            };
            m.chunks.push(chunk);
        }
        for d in &pl.defined {
            if let (Some(k), Some(v)) = (l.inputs.symbols.get(d.symbol), pl.defined_of(d.symbol)) {
                m.defines.push((k.name.to_vec(), v));
            }
        }
        m
    }
}
