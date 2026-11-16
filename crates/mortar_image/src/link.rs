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
    TargetKind, TlsBlock,
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
use mortar_synth::dynamic::{ArraySection, DynKind, DynamicPlan, RunpathTag};
use mortar_synth::dynstr::DynstrChunk;
use mortar_synth::dynsym::{DynsymChunk, DynsymInputs};
use mortar_synth::eh::CodeRef;
use mortar_synth::gnuhash::GnuHashChunk;
use mortar_synth::got::{GotAddrs, GotPlan, Region, SlotValues};
use mortar_synth::hdr::{HdrAddrs, HdrPlan};
use mortar_synth::image::{EhChunk, GotChunk, GotPart, HdrChunk, SymTable, SymtabChunk};
use mortar_synth::symtab::{InternalValue, SymAddrs, SymCtx, SymtabPlan};
use mortar_synth::version::{VerneedChunk, VersymChunk};
use mortar_synth::{
    BuildIdPlan, Division, DynAddrs, DynRecord, DynSymSource, DynsymPlan, EhPlan, EhSource,
    EhValue, EhWho, FillCx, InterpPlan, PropertyPlan, RelUnit, RelaPlan, ShstrtabPlan, SizeCx,
    SynthChunk, SynthError, Window,
};

use crate::fill::{self, EhFill, Failed, Filled, Image, MergedFrags, SplitPlace, Tables};
use crate::headers::{self, Ehdr, Phdr, Shdr};
use crate::layout_plan::{self, Extras, Model, SynthRow};
use crate::output::{Output, OutputOptions};
use crate::source::RecordKeys;
use crate::target::Target;
use crate::{IMAGE_BLOCK, PHASE_IMAGE, PHASE_OUTPUT};

const SHF_ALLOC: u64 = 0x2;
const SHF_MERGE: u64 = 0x10;
const SHF_EXECINSTR: u64 = 0x4;
const SHT_NOBITS: u32 = 8;
const SHT_RELA: u32 = 4;
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

    /// The facts of `key` for a record whose symbol is `mention`, as
    /// `arch.scan` sees them (an undefined key takes the mention's type):
    /// fill's checks of the sections the scan left to it
    /// (`RecIndex::deferred`, `spec/image.md` I-14). By default the
    /// definition's, [`KeyFacts::facts`].
    fn facts_at(&self, key: SymKey, mention: Option<SymEntry>) -> SymFacts {
        let _ = mention;
        self.facts(key)
    }

    /// The diagnostics of deferred `sections` (I-14), as the scan's walk
    /// of them makes them, keyed at phase 100: fill found a fault in them,
    /// or could not check them. By default one `internal-error`: these
    /// tables cannot report them.
    fn deferred_report(&self, sections: &[SectionId], pool: &Pool) -> Vec<(Key, Diagnostic)> {
        let _ = pool;
        let what = format!(
            "image: {} section(s) left to fill's checks (I-14) and no report of them",
            sections.len()
        );
        vec![(Key::new(PHASE_IMAGE, 0, 0), internal(what))]
    }

    /// Walks the relocation records of section `sid` (`index` in `file`,
    /// its relocation section `rel`) as the scan's walk sees them, with
    /// the `ScanCx` the scan called `arch.scan` with: `.rela.dyn`'s
    /// records (`spec/dyn.md` D-40, `crate::dynrel::roles`). By default
    /// an `internal-error`: these tables cannot walk.
    #[allow(clippy::too_many_arguments, reason = "the scan's walk of one section")]
    fn walk_records(
        &self,
        file: FileId,
        sid: SectionId,
        index: u32,
        rel: u32,
        leads: bool,
        visit: &mut dyn FnMut(&mortar_arch::ScanCx<'_>, &mortar_arch::RelocAt<'_>),
    ) -> Result<(), Diagnostic> {
        let _ = (file, sid, index, rel, leads, visit);
        Err(internal("image: no walk of a section's records for .rela.dyn"))
    }
}

/// The plans of an output with `.dynamic` (`spec/dyn.md`; phase 80), and
/// what the map records of them.
#[derive(Clone, Copy)]
pub struct DynPlans<'a> {
    pub kind: DynKind,
    pub dynsym: &'a DynsymPlan,
    pub src: &'a dyn DynSymSource,
    pub rela: &'a RelaPlan,
    pub dynamic: &'a DynamicPlan,
    /// `.interp` (D-31): `None` in a `StaticPie` (D-1).
    pub interp: Option<&'a InterpPlan>,
    pub runpath: Option<(RunpathTag, &'a [u8])>,
    /// Per copy allocation (as `Slots.copies`): its size (D-42).
    pub copy_size: &'a [u64],
    pub bind_now: bool,
    pub relro: bool,
    /// The context `.dynsym`'s exported values are read with, as
    /// `.symtab`'s (D-23 "exactly as `.symtab`").
    pub syms: SymCtx<'a>,
    /// The command line's words (the map's `arg` records, P-D3).
    pub args: &'a [Vec<u8>],
}

impl std::fmt::Debug for DynPlans<'_> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("DynPlans").field("kind", &self.kind).finish_non_exhaustive()
    }
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
    /// An output with `.dynamic`'s plans; `None` in a static link.
    pub dynamic: Option<DynPlans<'a>>,
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
    /// Keep the build-id leaves (I-76; `spec/incr.md` IN-21): an
    /// incremental-mode link stores them in its state, `L128` leaves when
    /// no id is hashed. `false` in a plain link.
    pub keep_leaves: bool,
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
    /// With `LinkOptions::keep_leaves`: the leaves, in window order, and
    /// their function.
    pub leaves: Option<(mortar_digest::TreeFn, Vec<mortar_digest::Leaf>)>,
    /// The build id's descriptor in the file (hashed as zeros, I-73).
    pub sealed: Option<std::ops::Range<u64>>,
    /// The output file's size.
    pub file_size: u64,
    /// With `LinkOptions::keep_leaves`: the device and inode of the file
    /// written (`spec/incr.md` IN-21), from its descriptor.
    pub file: Option<(u64, u64)>,
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
    /// `_DYNAMIC`, `.dynamic`'s address (`spec/dyn.md` D-32); `None` in a
    /// static link.
    dynamic: Option<u64>,
    /// Per section index (0 is the null section).
    sections: Box<[OutSection]>,
    /// References into `Merge` sections by offset (I-119).
    pieces: refs::PieceIndex,
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
            dynamic: link.synth.dynamic.and_then(|_| chunk_addr(SynthKind::Dynamic)),
            sections: out_sections(link.layout).into(),
            pieces: refs::PieceIndex::default(),
        };
        t.pieces = refs::PieceIndex::new(link.split, |s| t.section_size(s));
        let n = link.inputs.symbols.len() as usize;
        let global = link.pool.unit_cost(16).map(n, |i| t.def_addr(SymbolId(i as u32)));
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
                let p = if let Some(p) = self.pieces.named_symbol(self.link.split, s, v) {
                    p.ok()?
                } else {
                    let size = self.section_size(s)?;
                    refs::named_symbol(self.link.split, s, size, v).ok()?
                };
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
    /// Whether `frag` survived the mark (`frag_live`; every fragment
    /// without `--gc-sections`).
    fn frag_live(&self, frag: mortar_core::FragId) -> bool {
        self.link.liveness.frag_live.get(frag).copied().unwrap_or(true)
    }

    /// Whether a named symbol at `v` in `Merge` section `s` names a
    /// collected piece (I-7): its references are then to a dead target.
    fn named_piece_dead(&self, s: SectionId, v: u64) -> bool {
        let found = if let Some(p) = self.pieces.named_symbol(self.link.split, s, v) {
            p
        } else {
            let Some(size) = self.section_size(s) else { return false };
            refs::named_symbol(self.link.split, s, size, v)
        };
        found.is_ok_and(|p| !self.frag_live(p.frag))
    }

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

impl LinkTables<'_, '_> {
    /// `sym(key)`, reading the key's symbol once; with `split` (the
    /// caller asks `split_target`), `s` is left 0 when the target is in a
    /// split section, and the flag says so (`Tables::record_sym`).
    fn sym_value(&self, key: SymKey, split: bool) -> (SymValue, bool) {
        self.sym_value_of(key, split, self.link.facts.facts(key))
    }

    /// [`Self::sym_value`] with the key's facts (a definition's) given.
    fn sym_value_of(&self, key: SymKey, split: bool, facts: SymFacts) -> (SymValue, bool) {
        let entry = self.def_entry(key);
        let read = entry.and_then(|e| self.sym_of(e).map(|(t, s)| (e, t, s)));
        let size = read.map_or(0, |r| r.2.st_size);
        let place = read.and_then(|(e, t, sym)| match t.section_of(e.index, &sym).ok()? {
            SymbolSection::Absolute => Some(Err(sym.st_value)),
            SymbolSection::Section(index) => {
                Some(Ok((self.link.resolution.section_id(e.file, index)?, sym)))
            }
            _ => None,
        });
        // `split_target`'s cases: a section symbol of a `Merge` section,
        // any key defined in an `.eh_frame`.
        let in_split = |s: SectionId, sym: &mortar_elf::raw::Sym| match self.kind_of(s) {
            Some(SectionKind::Merge) => sym.sym_type() == STT_SECTION,
            Some(SectionKind::EhFrame) => true,
            _ => false,
        };
        let is_split = split && matches!(&place, Some(Ok((s, sym))) if in_split(*s, sym));
        // A named symbol in a collected merge piece (I-7) is dead, as one
        // in a collected section is.
        let piece_dead = |place: &Option<Result<(SectionId, mortar_elf::raw::Sym), u64>>| {
            matches!(place, Some(Ok((s, sym)))
                if sym.sym_type() != STT_SECTION
                    && self.kind_of(*s) == Some(SectionKind::Merge)
                    && self.named_piece_dead(*s, sym.st_value))
        };
        let (s, live) = match key.kind() {
            SymKind::Local(_) => {
                let live = !facts.dead && !piece_dead(&place);
                let s = match place {
                    _ if !live || is_split => None,
                    Some(Err(abs)) => Some(abs),
                    Some(Ok((s, sym))) => self.value_in(s, sym.st_value),
                    None => None,
                };
                (s, live)
            }
            SymKind::Global(id) => {
                let live = match place {
                    Some(Ok((sec, _))) => self.link.liveness.is_live(sec) && !piece_dead(&place),
                    _ => true,
                };
                (self.global.get(id.0 as usize).copied().flatten(), live)
            }
        };
        // S-71: every address of an IFUNC with an IPLT entry is the entry.
        let s = self.plt(key).or(s).unwrap_or(0);
        (SymValue { s, kind: facts.kind, size, live, tls: facts.tls }, is_split)
    }
}

impl FillTables for LinkTables<'_, '_> {
    fn sym(&self, key: SymKey) -> SymValue {
        self.sym_value(key, false).0
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
        // `spec/dyn.md` D-44, D-49: a PLT entry (dynamic) or an IPLT one.
        self.link.synth.got.plt_address(&self.got_addrs, key)
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
    fn record_sym(&self, key: SymKey) -> (SymValue, bool) {
        self.sym_value(key, true)
    }

    fn scan_facts(&self, key: SymKey, mention: Option<SymEntry>) -> Option<SymFacts> {
        Some(self.link.facts.facts_at(key, mention))
    }

    /// One read of the scan's facts where the mention cannot change them:
    /// a mention gives only an undefined key its type, and an undefined
    /// key's kind is `Weak0`, `WeakDyn` or `Undefined` (I-19); otherwise
    /// the definition's facts are those at the mention.
    fn record_sym_at(
        &self,
        key: SymKey,
        mention: Option<SymEntry>,
    ) -> (SymValue, bool, Option<SymFacts>) {
        let at = self.link.facts.facts_at(key, mention);
        let undefined =
            matches!(at.kind, TargetKind::Weak0 | TargetKind::WeakDyn | TargetKind::Undefined);
        let def = if mention.is_some() && undefined && matches!(key.kind(), SymKind::Global(_)) {
            self.link.facts.facts(key)
        } else {
            debug_assert_eq!(at, self.link.facts.facts(key), "facts at a mention");
            at
        };
        let (value, split) = self.sym_value_of(key, true, def);
        (value, split, Some(at))
    }

    /// D-25, D-46: the OUTPUT section's `SHF_WRITE` (the loader maps the
    /// output section; RELRO sections have it), as the scan's place.
    fn section_writable(&self, section: SectionId) -> Option<bool> {
        let a = self.link.assignment;
        let flags = match a.out(section).and_then(|o| a.outsecs.get(o)) {
            Some(out) => out.flags,
            None => self.section_flags(section),
        };
        Some(flags & mortar_elf::consts::SHF_WRITE != 0)
    }

    fn target_alloc(&self, key: SymKey) -> bool {
        match self.def_entry(key).and_then(|e| self.entry_place(e)) {
            Some(Ok((s, _))) => self.section_flags(s) & SHF_ALLOC != 0,
            Some(Err(_)) => false,
            // Commons, linker-defined symbols: in alloc chunks.
            None => true,
        }
    }

    fn split_target(&self, key: SymKey, addend: i64) -> Option<Result<Option<u64>, Diagnostic>> {
        let place = self.split_place(key)?;
        self.split_at(&place, key, addend)
    }

    fn split_place(&self, key: SymKey) -> Option<SplitPlace> {
        let e = self.def_entry(key)?;
        let Ok((s, sym)) = self.entry_place(e)? else { return None };
        let section_sym = sym.sym_type() == STT_SECTION;
        let merge = match self.kind_of(s)? {
            SectionKind::Merge if section_sym => true,
            SectionKind::EhFrame => false,
            _ => return None,
        };
        let row = if merge { self.pieces.row_of(s).unwrap_or(u32::MAX) } else { u32::MAX };
        let chunk_base = merge
            .then(|| {
                let chunk = self.addrs.merged_chunk(self.link.assignment.out(s)?)?;
                Some(self.chunk_at(chunk, 0)?.1)
            })
            .flatten();
        Some(SplitPlace { section: s, merge, value: sym.st_value, section_sym, row, chunk_base })
    }

    fn split_at(
        &self,
        place: &SplitPlace,
        _key: SymKey,
        addend: i64,
    ) -> Option<Result<Option<u64>, Diagnostic>> {
        let &SplitPlace { section: s, merge, value, section_sym, row, chunk_base } = place;
        let malformed = |what: String| Diagnostic::error(Kind::Malformed { what });
        if merge {
            let r = (value as i64).wrapping_add(addend);
            let found = if let Some(found) = self.pieces.section_symbol_in(self.link.split, row, r)
            {
                found
            } else {
                let size = self.section_size(s)?;
                refs::section_symbol(self.link.split, s, size, r)
            };
            // S-13: the piece's fragment is in the section's output
            // section's merged chunk.
            let frag_value = |frag| match chunk_base {
                Some(base) => {
                    let v = self.link.layout.merge.offset(frag).and_then(|o| base.checked_add(o));
                    debug_assert_eq!(v, self.frag_value(frag), "S-13");
                    v
                }
                None => self.frag_value(frag),
            };
            return Some(match found {
                Ok(p) if !self.frag_live(p.frag) => Ok(None),
                Ok(p) => frag_value(p.frag)
                    .and_then(|a| a.checked_add(p.within))
                    .map(Some)
                    .ok_or_else(|| internal("fill: a live merge piece has no address")),
                Err(w) => Err(malformed(w.message())),
            });
        }
        let Some((plan, src)) = self.link.eh else {
            return Some(Err(internal("fill: a reference into .eh_frame without its plan")));
        };
        let Some((_, eh)) = self.eh_frame else {
            return Some(Err(internal("fill: a reference into .eh_frame without the chunk")));
        };
        let r = if section_sym { (value as i64).wrapping_add(addend) } else { value as i64 };
        let tail = if section_sym { 0 } else { addend };
        Some(match plan.value(src, s, r, EhWho::Reloc) {
            Ok(EhValue::Offset(o)) => Ok(Some(eh.wrapping_add(o).wrapping_add(tail as u64))),
            Ok(EhValue::Dropped) => Err(internal("fill: a relocation to a dropped symbol")),
            Err(e) => Err(e.into_diagnostic()),
        })
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

    fn dynamic(&self) -> Option<u64> {
        self.dynamic
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
        let (target_word, target_abs) = if link.options.bti {
            match p.kind {
                PatchKind::Thunk(ThunkTarget::Sym { key, addend }) => {
                    thunk_target_word(link, tables, key, addend)?
                }
                PatchKind::Thunk(ThunkTarget::Plt(_)) => (plt_first_word(link, target), false),
                PatchKind::Erratum { .. } => (None, false),
            }
        } else {
            (None, false)
        };
        let cx = mortar_arch::PatchCx {
            addr,
            target,
            site: 0,
            word: 0,
            bti: link.options.bti,
            target_word,
            target_abs,
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

/// a64.md §9 (BTI): the input word at a thunk's direct target `key` +
/// `addend` and whether the target is `SHN_ABS`. The word is `Some` only
/// when it lies in an executable input section, inside a `$x` run of its
/// proof (`Proof::Insn`), and no relocation record's field can overlap it
/// (fill writes the input bytes there unchanged): anything else is `None`,
/// which `thunk_target_ok` refuses by name.
///
/// # Errors
///
/// `input-changed` when the input changed since it was read.
fn thunk_target_word(
    link: &Link<'_, '_>,
    tables: &LinkTables<'_, '_>,
    key: SymKey,
    addend: i64,
) -> Result<(Option<u32>, bool), Diagnostic> {
    let place = tables.def_entry(key).and_then(|e| tables.entry_place(e));
    let (sec, sym) = match place {
        None => return Ok((None, false)),
        Some(Err(_)) => return Ok((None, true)),
        Some(Ok(p)) => p,
    };
    let Some(off) = sym.st_value.checked_add_signed(addend) else { return Ok((None, false)) };
    let Some((file, index)) = link.resolution.sh_index(sec) else { return Ok((None, false)) };
    let Some(Reader::Object(obj)) = link.inputs.files.get(file).map(|f| &f.reader) else {
        return Ok((None, false));
    };
    let Ok(s) = obj.section(index) else { return Ok((None, false)) };
    let size = s.size();
    if s.flags() & SHF_EXECINSTR == 0
        || s.sh_type() == SHT_NOBITS
        || off.checked_add(4).is_none_or(|end| end > size)
    {
        return Ok((None, false));
    }
    let insn = (link.proof)(file, index, size)
        .iter()
        .any(|r| matches!(r.proof, mortar_arch::Proof::Insn) && r.start <= off && off + 4 <= r.end);
    if !insn || relocated_near(link, obj, file, sec, off)? {
        return Ok((None, false));
    }
    let Ok(Some(range)) = s.file_range() else { return Ok((None, false)) };
    let at = range.start as u64 + off;
    let mut w = [0u8; 4];
    link.inputs
        .copy_to(file, at..at + 4, &mut w)
        .map_err(|e| crate::source::copy_error(e, &layout_plan::file_ref(link.inputs, file)))?;
    Ok((Some(u32::from_le_bytes(w)), false))
}

/// Whether a relocation record of input section `sec` (in `obj`) has an
/// `r_offset` in `off - 7 ..= off + 3`, so that a field of up to 8 bytes
/// could overlap the word at `off`. Found by binary search over the
/// records in `r_offset` order (I-29, `Link::records`); a section whose
/// records cannot be read that way (not `SHT_RELA`, ELF32) answers `true`.
fn relocated_near(
    link: &Link<'_, '_>,
    obj: &mortar_elf::Object<'_>,
    file: FileId,
    sec: SectionId,
    off: u64,
) -> Result<bool, Diagnostic> {
    let Some(rel) = link.resolution.sections.get(sec).map(|r| r.reloc) else { return Ok(true) };
    if rel.0 == 0 {
        return Ok(false);
    }
    let Ok(rs) = obj.section(rel) else { return Ok(true) };
    if rs.sh_type() != SHT_RELA || !matches!(obj.class(), mortar_elf::Class::Elf64) {
        return Ok(true);
    }
    let Ok(Some(range)) = rs.file_range() else { return Ok(true) };
    let base = range.start as u64;
    let count = (range.end - range.start) as u64 / 24;
    let order = link.records.get(sec);
    let path = || layout_plan::file_ref(link.inputs, file);
    let r_offset = |k: u64| -> Result<u64, Diagnostic> {
        let i = match order {
            mortar_core::records::Order::Sorted => k,
            mortar_core::records::Order::Permuted(p) => {
                u64::from(*p.get(k as usize).ok_or_else(|| internal("image: a short permutation"))?)
            }
        };
        let at = base + i * 24;
        let mut b = [0u8; 8];
        link.inputs
            .copy_to(file, at..at + 8, &mut b)
            .map_err(|e| crate::source::copy_error(e, &path()))?;
        Ok(u64::from_le_bytes(b))
    };
    let low = off.saturating_sub(7);
    let (mut lo, mut hi) = (0u64, count);
    while lo < hi {
        let mid = lo + (hi - lo) / 2;
        if r_offset(mid)? < low {
            lo = mid + 1;
        } else {
            hi = mid;
        }
    }
    Ok(lo < count && r_offset(lo)? < off + 4)
}

/// a64.md §9, §10.4: the first word of the IPLT entry at `target`, as
/// `write_plt` writes it in this output (`bti c` in a BTI output); `None`
/// when the architecture writes none.
fn plt_first_word(link: &Link<'_, '_>, target: u64) -> Option<u32> {
    let variant = link.synth.got.variant();
    let size = link.arch.plt(variant).iplt_entry as usize;
    let mut buf = vec![0u8; size.max(4)];
    let cx =
        mortar_arch::SynthCx { addr: target, header: 0, got_plt: target, got_base: 0, index: 0 };
    link.arch.write_plt(variant, mortar_arch::PltPart::Iplt(0), &cx, &mut buf[..size]).ok()?;
    Some(u32::from_le_bytes([buf[0], buf[1], buf[2], buf[3]]))
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
        || !link.layout.slots.iplt.is_empty()
        || link.synth.dynamic.is_some_and(|d| {
            let inputs = DynsymInputs {
                resolution: link.resolution,
                demands: link.demands,
                slots: &link.layout.slots,
                src: d.src,
            };
            mortar_synth::dynsym::gnu_symbols(d.dynsym, &inputs)
        });
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
            // Layout's own chunk (L-64's `.pad.relro`): NOBITS, no plan.
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
                    // S-69: `sh_link` 0 in a static output (`.dynsym` in a
                    // dynamic one), `sh_info` `.got.plt`.
                    SynthKind::RelaPlt => {
                        row.sh_info = synth_shndx(link, SynthKind::GotPlt).unwrap_or(0);
                        if link.synth.dynamic.is_some() {
                            row.sh_link = synth_shndx(link, SynthKind::Dynsym).unwrap_or(0);
                        }
                    }
                    // gABI: a symbol table's and `.dynamic`'s strings, a
                    // hash table's, versym's and relocations' symbols.
                    SynthKind::Dynsym => {
                        row.sh_link = synth_shndx(link, SynthKind::Dynstr).unwrap_or(0);
                        row.sh_info = 1;
                    }
                    SynthKind::Dynamic => {
                        row.sh_link = synth_shndx(link, SynthKind::Dynstr).unwrap_or(0);
                    }
                    SynthKind::GnuHash | SynthKind::GnuVersion | SynthKind::RelaDyn => {
                        row.sh_link = synth_shndx(link, SynthKind::Dynsym).unwrap_or(0);
                    }
                    SynthKind::GnuVersionR => {
                        row.sh_link = synth_shndx(link, SynthKind::Dynstr).unwrap_or(0);
                        row.sh_info = link.layout.slots.verneed.len() as u32;
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

// ---- the dynamic sections (spec/dyn.md) ----------------------------------

/// The image's answers to `.dynsym` (`DynAddrs`) and `.dynamic`
/// (`DynamicAddrs`) over the layout.
struct DynView<'t, 'a, 's> {
    t: &'t LinkTables<'a, 's>,
    d: DynPlans<'a>,
}

impl DynView<'_, '_, '_> {
    /// A synthetic chunk's id.
    fn chunk_id(&self, kind: SynthKind) -> Option<ChunkId> {
        Some(self.t.link.synth.chunks.iter().find(|c| c.0 == kind)?.1)
    }

    /// The `.dynsym` inputs.
    fn inputs(&self) -> DynsymInputs<'_> {
        let l = &self.t.link;
        DynsymInputs {
            resolution: l.resolution,
            demands: l.demands,
            slots: &l.layout.slots,
            src: self.d.src,
        }
    }
}

impl DynAddrs for DynView<'_, '_, '_> {
    fn defined(&self, key: SymbolId) -> Option<(u32, u64)> {
        mortar_synth::symtab::global_value(&self.d.syms, self.t, key).ok().flatten()
    }

    fn plt(&self, key: SymbolId) -> Option<u64> {
        self.t.link.synth.got.plt_address(&self.t.got_addrs, SymKey::global(key))
    }

    fn iplt(&self, key: SymbolId) -> Option<(u32, u64)> {
        let shndx = synth_shndx(&self.t.link, SynthKind::Plt)?;
        let at = self.t.link.synth.got.iplt_address(&self.t.got_addrs, SymKey::global(key))?;
        Some((shndx, at))
    }

    fn copy(&self, alloc: u32) -> Option<(u32, u64)> {
        let row = self.t.link.layout.placement.sym_place_of(AllocKey::Copy(alloc))?;
        self.t.chunk_at(row.chunk, row.offset)
    }
}

impl crate::dynamic::DynamicAddrs for DynView<'_, '_, '_> {
    fn chunk(&self, kind: SynthKind) -> Option<(u64, u64)> {
        let c = self.t.link.layout.placement.chunks.get(self.chunk_id(kind)?)?;
        Some((c.addr?, c.size))
    }

    fn section(&self, section: ArraySection) -> Option<(u64, u64)> {
        let l = &self.t.link;
        let shndx = l.layout.placement.chunks.iter().find_map(|(_, c)| {
            let o = c.kind.out()?;
            (l.assignment.outsecs.get(o)?.name == section.name()).then_some(c.shndx?)
        })?;
        let s = self.t.sections.get(shndx as usize)?;
        Some((s.addr, s.size))
    }

    fn symbol(&self, key: SymbolId) -> Option<u64> {
        self.t.global.get(key.0 as usize).copied().flatten()
    }
}

/// `.rela.dyn` as a chunk: its bytes made before fill (D-40).
struct RelaDynChunk(crate::dynrel::RelaDyn);

impl SynthChunk for RelaDynChunk {
    fn size(&self, _: &SizeCx<'_>) -> u64 {
        self.0.size()
    }

    fn division(&self) -> Division {
        Division::Divisible
    }

    fn write(
        &self,
        _: &FillCx<'_>,
        range: std::ops::Range<u64>,
        out: &mut [u8],
    ) -> Result<(), SynthError> {
        if range.end > self.0.size() || out.len() as u64 != range.end - range.start {
            return Err(mortar_synth::error::internal(".rela.dyn written out of its range"));
        }
        self.0.write(&mut Window::new(range.start, out));
        Ok(())
    }
}

/// A NOBITS chunk (`.dynbss`, `.bss.rel.ro`, D-42): no file bytes.
struct Nobits(u64);

impl SynthChunk for Nobits {
    fn size(&self, _: &SizeCx<'_>) -> u64 {
        self.0
    }

    fn division(&self) -> Division {
        Division::Whole { max_size: 0 }
    }

    fn write(
        &self,
        _: &FillCx<'_>,
        _: std::ops::Range<u64>,
        out: &mut [u8],
    ) -> Result<(), SynthError> {
        out.fill(0);
        Ok(())
    }
}

/// The dynamic sections' chunks (D-20 to D-49), over a [`DynView`].
struct DynChunks<'v> {
    interp: Option<&'v InterpPlan>,
    gnu_hash: GnuHashChunk<'v>,
    dynsym: DynsymChunk<'v>,
    dynstr: DynstrChunk<'v>,
    versym: VersymChunk<'v>,
    verneed: VerneedChunk<'v>,
    dynamic: crate::dynamic::DynamicChunk<'v>,
    rela: RelaDynChunk,
    dynbss: Nobits,
    relro: Nobits,
}

impl<'v> DynChunks<'v> {
    fn new(view: &'v DynView<'_, '_, '_>, rela: crate::dynrel::RelaDyn) -> Self {
        let l = &view.t.link;
        let d = view.d;
        let slots = &l.layout.slots;
        let facts = l.arch.facts();
        let class = facts.class;
        let inputs = view.inputs();
        let nobits = |kind: SynthKind| {
            Nobits(
                view.chunk_id(kind)
                    .and_then(|id| l.layout.placement.chunks.get(id))
                    .map_or(0, |c| c.size),
            )
        };
        Self {
            interp: d.interp,
            gnu_hash: GnuHashChunk { plan: d.dynsym, src: d.src, class },
            dynsym: DynsymChunk {
                plan: d.dynsym,
                inputs,
                dynstr: &slots.dynstr,
                addrs: view,
                machine: facts.machine,
            },
            dynstr: DynstrChunk {
                plan: d.dynsym,
                needed: &slots.needed,
                verneed: &slots.verneed,
                rpath: d.runpath.map(|r| r.1),
                dynstr: &slots.dynstr,
                names: d.src,
            },
            versym: VersymChunk { plan: d.dynsym, inputs, verneed: &slots.verneed, class },
            verneed: VerneedChunk {
                needed: &slots.needed,
                verneed: &slots.verneed,
                dynstr: &slots.dynstr,
                names: d.src,
                class,
            },
            dynamic: crate::dynamic::DynamicChunk {
                plan: d.dynamic,
                dynstr: &slots.dynstr,
                addrs: view,
            },
            rela: RelaDynChunk(rela),
            dynbss: nobits(SynthKind::DynBss),
            relro: nobits(SynthKind::BssRelRo),
        }
    }

    fn get(&self, kind: SynthKind) -> Option<&(dyn SynthChunk + Sync + 'v)> {
        Some(match kind {
            SynthKind::Interp => self.interp?,
            SynthKind::GnuHash => &self.gnu_hash,
            SynthKind::Dynsym => &self.dynsym,
            SynthKind::Dynstr => &self.dynstr,
            SynthKind::GnuVersion => &self.versym,
            SynthKind::GnuVersionR => &self.verneed,
            SynthKind::Dynamic => &self.dynamic,
            SynthKind::RelaDyn => &self.rela,
            SynthKind::DynBss => &self.dynbss,
            SynthKind::BssRelRo => &self.relro,
            _ => return None,
        })
    }
}

/// `.rela.dyn`'s records (D-40, D-42, D-46, D-48): each relocated
/// section's, walked as the scan walked it (`KeyFacts::walk_records`) on
/// the pool; the GOT's (`GotPlan::got_records`); one `COPY` per copy
/// allocation.
///
/// # Errors
///
/// `internal-error` when a record breaks an invariant or the counts
/// differ from the plan's.
#[allow(clippy::too_many_lines)]
fn rela_dyn(view: &DynView<'_, '_, '_>) -> Result<crate::dynrel::RelaDyn, Diagnostic> {
    use crate::dynrel::{SectionCx, SectionRecords};
    use mortar_arch::DynRole;
    let t = view.t;
    let l = &t.link;
    let d = view.d;
    let arch = l.arch;
    let facts = arch.facts();
    let (class, form) = (facts.class, facts.rel_form);
    let pl = &l.layout.placement;
    let got = l.synth.got;
    let ga = t.got_addrs;
    let sections: Vec<SectionId> = d.rela.sections().collect();
    // `S + A` at link time (a `RELATIVE` record's addend): a reference
    // into a split section through its piece, else the key's value.
    let value = |key: SymKey, addend: i64| -> Option<u64> {
        match t.split_target(key, addend) {
            Some(Ok(Some(v))) => Some(v),
            Some(_) => None,
            None => Some(t.sym(key).s.wrapping_add_signed(addend)),
        }
    };
    let dynsym = |id: SymbolId| d.dynsym.index(id);
    let per: Vec<Result<(u64, SectionRecords), Diagnostic>> = l.pool.map(sections.len(), |i| {
        let sid = sections[i];
        let what = |m: &str| internal(format!(".rela.dyn: section {}: {m}", sid.0));
        let (file, index) = l.resolution.sh_index(sid).ok_or_else(|| what("no input section"))?;
        let rel = l.resolution.sections.get(sid).ok_or_else(|| what("no row"))?.reloc.0;
        let addr = pl.section_addr(sid, 0).ok_or_else(|| what("no address"))?;
        let leads = l.records.leads(sid);
        let mut out = SectionRecords::default();
        let mut err = None;
        l.facts.walk_records(file, sid, index.0, rel, leads, &mut |cx, at| {
            if err.is_some() {
                return;
            }
            let scx = SectionCx { arch, scan: *cx, addr, value: &value, dynsym: &dynsym };
            if let Err(e) = crate::dynrel::push_records(&scx, at, &mut out) {
                err = Some(e);
            }
        })?;
        if let Some(e) = err {
            return Err(what(&e));
        }
        out.relative.sort_unstable_by_key(|r| r.r_offset);
        let unit = RelUnit::Section(sid);
        if out.relative.len() != d.rela.relative_of(unit) as usize
            || out.other.len() != d.rela.other_of(unit) as usize
        {
            return Err(what(&format!(
                "{} RELATIVE and {} other records, the scan counted {} and {}",
                out.relative.len(),
                out.other.len(),
                d.rela.relative_of(unit),
                d.rela.other_of(unit)
            )));
        }
        Ok((addr, out))
    });
    let mut recs = Vec::with_capacity(per.len());
    for r in per {
        recs.push(r?);
    }
    let mut got_relative = Vec::new();
    let mut other = Vec::new();
    for r in got.got_records() {
        let r_offset = ga.got + r.offset;
        if r.role == DynRole::Relative {
            let v = r
                .key
                .and_then(|k| got.iplt_address(&ga, k).or_else(|| t.address(k)))
                .ok_or_else(|| internal(".rela.dyn: a GOT RELATIVE record without a value"))?;
            got_relative.push(DynRecord { r_offset, r_sym: 0, r_type: r.r_type, addend: v as i64 });
        } else {
            other.push(DynRecord { r_offset, r_sym: r.r_sym, r_type: r.r_type, addend: 0 });
        }
    }
    got_relative.sort_unstable_by_key(|r| r.r_offset);
    if !l.layout.slots.copies.is_empty() {
        let r_type = arch
            .dyn_type(DynRole::Copy)
            .ok_or_else(|| internal(".rela.dyn: no COPY type (D-42)"))?;
        for (a, c) in l.layout.slots.copies.iter().enumerate() {
            let (_, r_offset) = view
                .copy(a as u32)
                .ok_or_else(|| internal(format!(".rela.dyn: copy {a} has no address")))?;
            let r_sym = d
                .dynsym
                .index(c.alias)
                .ok_or_else(|| internal(format!(".rela.dyn: copy {a}'s key not in .dynsym")))?;
            other.push(DynRecord { r_offset, r_sym, r_type, addend: 0 });
        }
    }
    // D-40 step 1: the units with RELATIVE records, in address order.
    let mut units: Vec<(u64, Option<usize>)> = recs
        .iter()
        .enumerate()
        .filter(|(_, r)| !r.1.relative.is_empty())
        .map(|(i, r)| (r.0, Some(i)))
        .collect();
    if !got_relative.is_empty() {
        units.push((ga.got, None));
    }
    units.sort_unstable_by_key(|u| u.0);
    let counts: Vec<u32> = units
        .iter()
        .map(|u| match u.1 {
            Some(i) => recs[i].1.relative.len() as u32,
            None => got_relative.len() as u32,
        })
        .collect();
    let relative = crate::dynrel::fill_relative(l.pool, class, form, &counts, |u| {
        Ok(match units[u].1 {
            Some(i) => recs[i].1.relative.clone(),
            None => got_relative.clone(),
        })
    })
    .map_err(|m| internal(format!(".rela.dyn: {m}")))?;
    for (_, r) in recs {
        other.extend(r.other);
    }
    // S-70: a `StaticPie`'s IPLT `IRELATIVE`s, last.
    let irelative = got.irelative_records(&ga, t).map_err(SynthError::into_diagnostic)?;
    crate::dynrel::RelaDyn::new(d.rela, class, form, relative, other, irelative)
        .map_err(SynthError::into_diagnostic)
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
            SynthKind::Comment => &mortar_synth::Comment,
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
/// The sections whose checks the scan left to fill (`RecIndex::deferred`,
/// `spec/image.md` I-14) are checked by fill; a fault, a section fill
/// does not reach, or a link that fails before fill has checked them all
/// adds the scan's diagnostics of them ([`KeyFacts::deferred_report`]).
///
/// # Errors
///
/// Every diagnostic of the phase, keyed (I-43, I-64, I-68).
pub fn write(
    link: &Link<'_, '_>,
    path: &Path,
    opts: &OutputOptions,
    on_phase: &mut dyn FnMut(u16, &str),
) -> Result<Written, Failed> {
    let mut checked = false;
    match write_link(link, path, opts, on_phase, &mut checked) {
        Err(mut f) if !checked && !link.records.deferred().is_empty() => {
            // I-14: the link failed before fill checked the sections.
            let all = link.records.deferred();
            f.diagnostics.extend(link.facts.deferred_report(all, link.pool));
            Err(f)
        }
        r => r,
    }
}

/// The order of the deferred sections (I-14) whose table is large and
/// whose row is cut into several jobs' clips (`Plan::deferred_order`):
/// found once here, in parallel, where each clip would read the table
/// whole.
fn deferred_orders(
    link: &Link<'_, '_>,
    plan: &crate::plan::Plan,
) -> Result<crate::plan::DeferredOrders, Failed> {
    use crate::plan::{RecRows, Unit};
    if link.records.deferred().is_empty() {
        return Ok(Box::new([]));
    }
    let job_of = |off: u64| plan.jobs.partition_point(|j| j.end <= off);
    let mut todo = Vec::new();
    for row in &plan.rows {
        let Unit::Input(i) = row.unit else { continue };
        let u = &plan.inputs[i as usize];
        let Some(r) = u.relocs else { continue };
        let RecRows::Rela(t) = &plan.relocs[r as usize].rows else { continue };
        if row.size == 0
            || !link.records.is_deferred(u.section)
            || u64::from(t.count) * t.entsize() <= crate::reloc::WHOLE_TABLE
            || job_of(row.offset) == job_of(row.end() - 1)
        {
            continue;
        }
        todo.push((u.section, *t));
    }
    let orders = link.pool.map(todo.len(), |k| {
        crate::reloc::table_order(link.inputs, plan, &todo[k].1).map(|o| (todo[k].0, o))
    });
    let mut v = Vec::with_capacity(orders.len());
    for o in orders {
        v.push(o.map_err(failed)?);
    }
    v.sort_by_key(|o| o.0);
    v.dedup_by_key(|o| o.0);
    Ok(v.into_boxed_slice())
}

/// The deferred sections (I-14) fill does not reach: no input row with
/// relocations in the plan (an empty section, one not in the file).
fn unreached(link: &Link<'_, '_>, plan: &crate::plan::Plan) -> Vec<SectionId> {
    let deferred = link.records.deferred();
    if deferred.is_empty() {
        return Vec::new();
    }
    let mut reached: Vec<SectionId> =
        plan.inputs.iter().filter(|u| u.relocs.is_some()).map(|u| u.section).collect();
    reached.sort_unstable();
    deferred.iter().copied().filter(|s| reached.binary_search(s).is_err()).collect()
}

/// [`write`]; `checked` is set once fill has run (I-14).
#[allow(clippy::too_many_lines)]
fn write_link(
    link: &Link<'_, '_>,
    path: &Path,
    opts: &OutputOptions,
    on_phase: &mut dyn FnMut(u16, &str),
    checked: &mut bool,
) -> Result<Written, Failed> {
    let mut steps = mortar_core::wp032b_steps::Steps::new("image");
    let tables = LinkTables::new(*link);
    steps.step("tables", u64::from(link.inputs.symbols.len()), "symbol");
    let mut fixed = structure(link).map_err(failed)?;
    fixed.append(&mut patches(link, &tables).map_err(failed)?);
    steps.step("structure", 0, "");
    let chunks = Chunks::new(link);
    let view = link.synth.dynamic.map(|d| DynView { t: &tables, d });
    let dyn_chunks = match &view {
        Some(v) => Some(DynChunks::new(v, rela_dyn(v).map_err(failed)?)),
        None => None,
    };
    let pl = &link.layout.placement;
    let mut list: Vec<&(dyn SynthChunk + Sync)> = Vec::new();
    let mut synth = std::collections::BTreeMap::new();
    let mut sealed = None;
    let mut eh_addr = None;
    let mut hdr_addr = 0;
    for &(kind, id) in link.synth.chunks {
        let chunk: Option<&(dyn SynthChunk + Sync)> = match chunks.get(link, kind) {
            Some(c) => Some(c),
            None => dyn_chunks.as_ref().and_then(|c| c.get(kind)),
        };
        let Some(chunk) = chunk else {
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
        // D-42, D-49: `.dynbss` and `.bss.rel.ro` are NOBITS; they take
        // no file bytes, so no row of the fill plan.
        if matches!(kind, SynthKind::DynBss | SynthKind::BssRelRo) {
            continue;
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
    let mut plan = layout_plan::from_layout(&model, target, &extras, IMAGE_BLOCK, link.pool)
        .map_err(failed)?;
    plan.deferred_order = deferred_orders(link, &plan)?;
    steps.step("plan", plan.rows.len() as u64, "row");
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
    let leaf_fn = link
        .options
        .keep_leaves
        .then(|| build_id.map_or(mortar_digest::TreeFn::L128, TreeStyle::tree_fn));
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
        synth: FillCx {
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
        leaf_fn,
        fixups: &fixups,
    };
    let out = Output::create(path, opts).map_err(|kd| Failed { diagnostics: vec![kd] })?;
    steps.step("create", 0, "");
    let (filled, checks) = fill::write_checked(&img, &out, out.shown(), link.pool);
    *checked = true;
    // I-14: the scan's diagnostics of the deferred sections fill found a
    // fault in, or did not reach; of all of them when a job stopped early.
    let mut report_on =
        if checks.complete { checks.flagged.clone() } else { link.records.deferred().to_vec() };
    report_on.extend(unreached(link, &plan));
    report_on.sort_unstable();
    report_on.dedup();
    let mut report = if report_on.is_empty() {
        Vec::new()
    } else {
        link.facts.deferred_report(&report_on, link.pool)
    };
    if checks.complete && !checks.flagged.is_empty() && report.is_empty() {
        let what = format!(
            "image: fill found a fault in section(s) {:?} that the scan's walk of them does not report (I-14)",
            checks.flagged.iter().map(|s| s.0).collect::<Vec<_>>()
        );
        report.push((Key::new(PHASE_IMAGE, 0, 0), internal(what)));
    }
    let filled: Filled = match filled {
        Ok(f) if report.is_empty() => f,
        Ok(_) => {
            out.abort();
            return Err(Failed { diagnostics: report });
        }
        Err(mut e) => {
            out.abort();
            e.diagnostics.append(&mut report);
            return Err(e);
        }
    };
    steps.step("fill", plan.file_size, "byte");
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
    steps.step("recheck", 0, "");
    on_phase(PHASE_IMAGE, "image");
    let written = if link.options.keep_leaves {
        out.commit_identified()
    } else {
        out.commit().map(|()| None)
    }
    .map_err(|kd| Failed { diagnostics: vec![kd] })?;
    on_phase(PHASE_OUTPUT, "output file");
    Ok(Written {
        build_id: filled.build_id,
        leaves: leaf_fn.zip(filled.leaves),
        sealed,
        file_size: plan.file_size,
        file: written,
        rows: plan.rows.len(),
        jobs: plan.jobs.len(),
    })
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
                    size: plan.record_span(n),
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
        m.incremental = self.incremental_layout();
        m.dead = self.dead_lines();
        if let Some(d) = l.synth.dynamic {
            m.dynamic = Some(self.dyn_records(d));
        }
        m
    }

    /// The `hole` and `dead` records (`spec/plumb.md` §3.2 and §6.8,
    /// `spec/incr.md` IN-60): every hole of the layout, at its offset from
    /// its output section's start (by address in an allocated section,
    /// by file offset in another), in section header order. An allocated
    /// range holding its section's fill, or a padding CIE, is a `hole`;
    /// a debug section's neutral form is `dead … neutral`; a 4-, 8- or
    /// 12-byte `.eh_frame` pad is no line (the record before it absorbs
    /// it, and its `piece` covers it).
    /// Whether the layout is an incremental-mode one (`spec/incr.md`
    /// IN-30): linked in incremental mode (`keep_leaves` is set exactly
    /// then), or replayed from an incremental state, whose layout has
    /// holes (a plain layout never has one).
    fn incremental_layout(&self) -> bool {
        self.link.options.keep_leaves || !self.link.layout.placement.holes.is_empty()
    }

    fn dead_lines(&self) -> Vec<crate::map::DeadLine> {
        use crate::map::{DeadKind, DeadLine};
        use mortar_core::layout::HoleKind;
        let l = &self.link;
        let pl = &l.layout.placement;
        if !self.incremental_layout() {
            return Vec::new();
        }
        // Per section index, the start of its first chunk.
        let mut starts: std::collections::BTreeMap<u32, u64> = std::collections::BTreeMap::new();
        let pos = |c: &mortar_core::layout::Chunk| c.addr.unwrap_or(c.offset);
        for (_, c) in pl.chunks.iter() {
            if let Some(i) = c.shndx {
                let e = starts.entry(i).or_insert(u64::MAX);
                *e = (*e).min(pos(c));
            }
        }
        let mut out: Vec<(u32, DeadLine)> = pl
            .holes
            .iter()
            .filter_map(|h| {
                let c = &pl.chunks[h.chunk];
                let i = c.shndx?;
                let name = match c.kind {
                    ChunkKind::Synthetic(s) => s.name,
                    _ => c.name(l.assignment)?,
                };
                let kind = match h.kind {
                    HoleKind::Fill => None,
                    HoleKind::EhPad if h.size >= 16 => None,
                    HoleKind::EhPad => return None,
                    HoleKind::Debug | HoleKind::AbbrevTable | HoleKind::DeadDebug => {
                        Some(DeadKind::Neutral)
                    }
                    // Dead string fragments; a dead constant fragment
                    // takes the fill and is a `Fill` hole (its writer's).
                    HoleKind::DeadFragment => Some(DeadKind::Strings),
                    HoleKind::DeadCie => Some(DeadKind::Cie),
                };
                let offset = pos(c) + h.offset - starts.get(&i).copied().unwrap_or(0);
                Some((i, DeadLine { out: name.to_vec(), offset, size: h.size, kind }))
            })
            .collect();
        {
            // layout.md L-118: the 843419 start padding before a synthetic
            // code chunk is fill no item owns and no alignment explains:
            // an incremental-mode map lists it as a hole (plumb.md §6.8,
            // P-I1). From the end of the chunk before it in the section.
            let mut alloc: Vec<&mortar_core::layout::Chunk> = pl
                .chunks
                .iter()
                .map(|(_, c)| c)
                .filter(|c| c.is_alloc() && c.in_file() && c.addr.is_some() && c.shndx.is_some())
                .collect();
            alloc.sort_by_key(|c| (c.addr, c.shndx));
            for w in alloc.windows(2) {
                let (prev, c) = (w[0], w[1]);
                let code = matches!(c.kind, ChunkKind::Patches(_) | ChunkKind::Synthetic(_))
                    && c.flags & SHF_EXECINSTR != 0;
                let (Some(i), Some(a), Some(p)) = (c.shndx, c.addr, prev.addr) else { continue };
                let end = p + prev.capacity;
                let align = 1u64 << c.align.min(63);
                if !code || prev.shndx != Some(i) || a <= end.next_multiple_of(align) {
                    continue;
                }
                let Some(name) = (match c.kind {
                    ChunkKind::Synthetic(s) => Some(s.name),
                    _ => c.name(l.assignment).or_else(|| prev.name(l.assignment)),
                }) else {
                    continue;
                };
                let offset = end - starts.get(&i).copied().unwrap_or(0);
                out.push((i, DeadLine { out: name.to_vec(), offset, size: a - end, kind: None }));
            }
        }
        out.sort_by_key(|(i, d)| (*i, d.offset));
        out.into_iter().map(|(_, d)| d).collect()
    }

    /// The map's dynamic records (`spec/plumb.md` §3.2, P-D3).
    fn dyn_records(&self, d: DynPlans<'_>) -> crate::map::DynRecords {
        use crate::map::CopyHint;
        let l = &self.link;
        let slots = &l.layout.slots;
        let view = DynView { t: self, d };
        let mut r = crate::map::DynRecords::default();
        crate::dynamic::fill_records(&mut r, d.kind, d.interp, d.runpath, d.bind_now, d.relro);
        r.args = d.args.to_vec();
        r.needed = slots.needed.iter().map(|&f| d.src.needed(f).to_vec()).collect();
        r.copies = slots
            .copies
            .iter()
            .enumerate()
            .filter_map(|(a, c)| {
                Some(CopyHint {
                    name: d.src.symbol(c.alias).to_vec(),
                    addr: view.copy(a as u32)?.1,
                    size: *d.copy_size.get(a)?,
                })
            })
            .collect();
        r.exports = d
            .dynsym
            .order()
            .iter()
            .filter(|&&k| {
                matches!(
                    l.resolution.def.get(k),
                    Some(Def::Defined(_) | Def::Common(_) | Def::Internal(_))
                )
            })
            .map(|&k| d.src.symbol(k).to_vec())
            .collect();
        let key_name = |k: SymKey| {
            let mut n = l.facts.name(k).to_vec();
            if let Some(id) = k.as_global()
                && let Some(Def::Shared(e)) = l.resolution.def.get(id).copied()
                && let Some(v) = d.src.bound_version(e).filter(|v| v.0 >= 2)
            {
                n.push(b'@');
                n.extend_from_slice(d.src.version(e.file, v));
            }
            n
        };
        r.plt = slots.plt.iter().map(|&k| key_name(k)).collect();
        r.iplt = slots.iplt.iter().map(|&k| key_name(k)).collect();
        r
    }
}

// ---- the text map (§9.3) ---------------------------------------------------

/// The text map of a link (I-120, I-121): from the same tables as
/// [`map_of`] (I-122). `output` is the output file as the command line
/// named it; `roots` the name of each root by its argument index (resolve
/// §4.2: the archive lines of the members a root took).
#[must_use]
pub fn text_map_of(
    link: &Link<'_, '_>,
    output: &[u8],
    roots: &[(u32, Vec<u8>)],
) -> crate::textmap::TextMap {
    LinkTables::new(*link).text_map(output, roots)
}

impl LinkTables<'_, '_> {
    /// `ARCHIVE(MEMBER)` or the path of `file`.
    fn file_text(&self, file: FileId) -> Vec<u8> {
        let Some(row) = self.link.inputs.files.get(file) else { return Vec::new() };
        let mut t = row.path.to_vec();
        if let Some(m) = &row.member {
            t.push(b'(');
            t.extend_from_slice(m.name);
            t.push(b')');
        }
        t
    }

    /// The name of entry `e` in its file's symbol table.
    fn entry_name(&self, e: SymEntry) -> Vec<u8> {
        self.sym_of(e)
            .and_then(|(t, s)| t.strings().get(s.st_name).ok())
            .map(<[u8]>::to_vec)
            .unwrap_or_default()
    }

    /// The `.symtab` name of a patch (S-80) and its size, as `mortar-map 1`
    /// lists it.
    fn patch_line(&self, p: &mortar_core::layout::Patch) -> (Vec<u8>, u64) {
        use mortar_core::layout::{PatchKind, ThunkTarget};
        let l = &self.link;
        match p.kind {
            PatchKind::Thunk(t) => {
                let (key, addend, plt) = match t {
                    ThunkTarget::Sym { key, addend } => (key, addend, false),
                    ThunkTarget::Plt(key) => (key, 0, true),
                };
                let mut name = b"__".to_vec();
                name.extend_from_slice(l.facts.name(key));
                if plt {
                    name.extend_from_slice(b"@plt");
                }
                if addend > 0 {
                    name.extend_from_slice(format!("+{addend:#x}").as_bytes());
                } else if addend < 0 {
                    name.extend_from_slice(format!("-{:#x}", addend.unsigned_abs()).as_bytes());
                }
                name.extend_from_slice(b"_veneer");
                (name, l.arch.thunk().map_or(0, |t| u64::from(t.size)))
            }
            PatchKind::Erratum { erratum, .. } => (
                mortar_synth::symtab::ERRATUM_843419.to_vec(),
                l.arch.patch(erratum).map_or(0, |s| u64::from(s.size)),
            ),
        }
    }

    #[allow(clippy::too_many_lines)]
    fn text_map(&self, output: &[u8], roots: &[(u32, Vec<u8>)]) -> crate::textmap::TextMap {
        use crate::textmap::{Item, OutSection as TextSection, TextMap, target_name};
        use mortar_core::resolution::{DropReason, FileLive, SectionState};
        use mortar_elf::Class;
        let l = &self.link;
        let pl = &l.layout.placement;
        let facts = l.arch.facts();
        let class64 = facts.class == Class::Elf64;
        let mut m = TextMap {
            digits: if class64 { 16 } else { 8 },
            output: output.to_vec(),
            target: target_name(facts.e_machine, class64),
            ..TextMap::default()
        };
        // 1. The archive members taken, and by whom (FileId order).
        for (file, live) in l.resolution.file_live.iter() {
            let FileLive::Extracted(by) = *live else { continue };
            let (taker, symbol) = if by.file == FileId::INTERNAL {
                let name = roots.iter().find(|r| r.0 == by.index.0).map(|r| r.1.clone());
                (Vec::new(), name.unwrap_or_default())
            } else {
                (self.file_text(by.file), self.entry_name(by))
            };
            m.archive.push((self.file_text(file), taker, symbol));
        }
        // 2, 4. Per live object: its discarded sections, its placed ones.
        // Per section index: (position, input section, item).
        let rows = self.sections.len();
        let mut items: Vec<Vec<(u64, Option<SectionId>, Item)>> = vec![Vec::new(); rows];
        for (file, row) in l.inputs.files.iter() {
            if file == FileId::INTERNAL
                || !l.resolution.file_live.get(file).is_some_and(|x| x.is_live())
            {
                continue;
            }
            let Reader::Object(o) = &row.reader else { continue };
            let text = self.file_text(file);
            let range = l.resolution.section_base[file];
            for i in 1..range.len() {
                let sid = SectionId(range.start.0 + i);
                let Some(r) = l.resolution.sections.get(sid) else { continue };
                let name = o.section(SectionIndex(i)).ok().and_then(|s| s.name().ok());
                let name = name.map(<[u8]>::to_vec).unwrap_or_default();
                let size = self.section_size(sid).unwrap_or(0);
                let dropped = match r.state {
                    SectionState::Dropped(DropReason::Consumed) => continue,
                    SectionState::Dropped(_) => true,
                    SectionState::Kept => !l.liveness.is_live(sid),
                };
                if dropped {
                    m.discarded.push((name, size, text.clone()));
                    continue;
                }
                // Merged and `.eh_frame` bytes are their output section's
                // linker-made chunk (below).
                if matches!(r.kind, SectionKind::Merge | SectionKind::EhFrame) {
                    continue;
                }
                let Some((c, at)) = pl.place.get(sid).and_then(|p| p.get()) else { continue };
                let Some((shndx, pos)) = self.chunk_at(c, at) else { continue };
                if let Some(v) = items.get_mut(shndx as usize) {
                    let item = Item::Input { name, addr: pos, size, file: text.clone() };
                    v.push((pos, Some(sid), item));
                }
            }
        }
        // Each global symbol, under the input section defining it.
        // A symbol in merged or `.eh_frame` bytes goes under its output
        // section's linker-made chunk, as do the patches' names (S-80).
        let mut syms: Vec<(SectionId, u64, Vec<u8>)> = Vec::new();
        let mut chunk_syms: Vec<(u32, u64, Vec<u8>)> = Vec::new();
        for (id, key) in l.inputs.symbols.iter() {
            let Some(Def::Defined(e)) = l.assignment.def(l.resolution, id) else { continue };
            let Some(Ok((sid, _))) = self.entry_place(e) else { continue };
            if self.section_flags(sid) & SHF_ALLOC == 0 {
                continue;
            }
            let Some(Some(addr)) = self.global.get(id.0 as usize) else { continue };
            if matches!(self.kind_of(sid), Some(SectionKind::Merge | SectionKind::EhFrame)) {
                let shndx = self.chunk_of(sid).and_then(|(c, _)| self.chunk_at(c, 0));
                if let Some((shndx, _)) = shndx {
                    chunk_syms.push((shndx, *addr, key.name.to_vec()));
                }
            } else {
                syms.push((sid, *addr, key.name.to_vec()));
            }
        }
        syms.sort();
        // The linker-made chunks: merged fragments, synthetic sections,
        // patches (with their `.symtab` names, S-80).
        for (c, chunk) in pl.chunks.iter() {
            let Some(shndx) = chunk.shndx else { continue };
            let Some(v) = items.get_mut(shndx as usize) else { continue };
            let Some((_, pos)) = self.chunk_at(c, 0) else { continue };
            let (name, file): (&[u8], &[u8]) = match chunk.kind {
                ChunkKind::Merged(o) => (l.assignment.outsecs[o].name, b"mortar"),
                ChunkKind::Synthetic(s) => (s.name, b"mortar"),
                ChunkKind::Patches(p) => (l.assignment.outsecs[p.out].name, b"linker stubs"),
                _ => continue,
            };
            if chunk.size == 0 {
                continue;
            }
            let item = Item::Input {
                name: name.to_vec(),
                addr: pos,
                size: chunk.size,
                file: file.to_vec(),
            };
            v.push((pos, None, item));
        }
        for p in &pl.patches {
            let Some((shndx, pos)) = self.chunk_at(p.chunk, p.offset) else { continue };
            chunk_syms.push((shndx, pos, self.patch_line(p).0));
        }
        chunk_syms.sort();
        // The name and flags of each output section, from its first chunk.
        let mut owners: Vec<Option<(&[u8], u64)>> = vec![None; rows];
        for (_, chunk) in pl.chunks.iter() {
            let Some(shndx) = chunk.shndx else { continue };
            let name: &[u8] = match chunk.kind {
                ChunkKind::Body(p) | ChunkKind::Patches(p) => l.assignment.outsecs[p.out].name,
                ChunkKind::Merged(o) | ChunkKind::Group(o) | ChunkKind::Relocs(o) => {
                    l.assignment.outsecs[o].name
                }
                ChunkKind::Synthetic(s) => s.name,
                _ => continue,
            };
            if let Some(slot) = owners.get_mut(shndx as usize)
                && slot.is_none()
            {
                *slot = Some((name, chunk.flags));
            }
        }
        // Output sections in section-index order: items by position, each
        // input's symbols after it, fill in an allocated section's gaps.
        for (shndx, mut row) in items.into_iter().enumerate() {
            let Some((name, flags)) = owners[shndx] else { continue };
            let sec = &self.sections[shndx];
            let alloc = flags & SHF_ALLOC != 0;
            let pattern = (flags & SHF_EXECINSTR != 0).then(|| facts.code_fill.to_vec());
            row.sort_by_key(|a| (a.0, a.1));
            let mut out = Vec::with_capacity(row.len());
            let start = if alloc { sec.addr } else { 0 };
            let mut end = start;
            for (pos, sid, item) in row {
                let Item::Input { size, .. } = item else { continue };
                if alloc && pos > end {
                    out.push(Item::Fill { addr: end, size: pos - end, pattern: pattern.clone() });
                }
                end = end.max(pos + size);
                out.push(item);
                let symbol = |s: &(u64, Vec<u8>)| Item::Symbol { addr: s.0, name: s.1.clone() };
                if let Some(sid) = sid {
                    let lo = syms.partition_point(|s| s.0 < sid);
                    let hi = syms.partition_point(|s| s.0 <= sid);
                    out.extend(syms[lo..hi].iter().map(|s| symbol(&(s.1, s.2.clone()))));
                } else {
                    let n = shndx as u32;
                    let lo = chunk_syms.partition_point(|s| (s.0, s.1) < (n, pos));
                    let hi = chunk_syms.partition_point(|s| (s.0, s.1) < (n, pos + size));
                    out.extend(chunk_syms[lo..hi].iter().map(|s| symbol(&(s.1, s.2.clone()))));
                }
            }
            if alloc && start + sec.size > end {
                out.push(Item::Fill { addr: end, size: start + sec.size - end, pattern });
            }
            m.sections.push(TextSection {
                name: name.to_vec(),
                addr: start,
                size: sec.size,
                items: out,
            });
        }
        // `LOAD` per input, an archive once (FileId order).
        let mut last_archive = None;
        for (file, row) in l.inputs.files.iter() {
            if file == FileId::INTERNAL {
                continue;
            }
            if let Some(mm) = &row.member {
                if last_archive == Some(mm.archive) {
                    continue;
                }
                last_archive = Some(mm.archive);
            }
            m.loads.push(row.path.to_vec());
        }
        m
    }
}
