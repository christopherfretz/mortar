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

use mortar_arch::{Arch, FillTables, OutputKind, ProofRun, SymFacts, SymNames, TlsBlock};
use mortar_core::assignment::Assignment;
use mortar_core::assignment::OutSec;
use mortar_core::demands::Demands;
use mortar_core::inputs::{CopyError, Reader};
use mortar_core::layout::{AllocKey, Chunk, ChunkKind, DefValue, Layout, Segment};
use mortar_core::par::Pool;
use mortar_core::records::RecIndex;
use mortar_core::resolution::{Def, SectionKind};
use mortar_core::{
    ChunkId, Column, FileId, Inputs, Liveness, OutSecId, Resolution, SectionId, SegmentId, Split,
    SymEntry, SymKey, SymbolId,
};
use mortar_diag::{Diagnostic, FileRef, Key, Kind};
use mortar_digest::TreeStyle;
use mortar_elf::{SectionIndex, SymbolIndex, SymbolSection};
use mortar_merge::order::Order;
use mortar_synth::chunks::SynthKind;
use mortar_synth::dynamic::{ArraySection, DynKind, DynamicPlan, RunpathTag};
use mortar_synth::dynstr::DynstrChunk;
use mortar_synth::dynsym::{DynsymChunk, DynsymInputs};
use mortar_synth::eh::CodeRef;
use mortar_synth::gnuhash::GnuHashChunk;
use mortar_synth::got::{GotAddrs, GotPlan, SlotValues};
use mortar_synth::hdr::{HdrAddrs, HdrPlan};
use mortar_synth::image::{EhChunk, GotChunk, GotPart, HdrChunk, SymTable, SymtabChunk};
use mortar_synth::symtab::{InternalValue, SymAddrs, SymCtx, SymtabPlan};
use mortar_synth::version::{VerdefChunk, VerneedChunk, VersymChunk};
use mortar_synth::{
    BuildIdPlan, Division, DynAddrs, DynRecord, DynSymSource, DynsymPlan, EhPlan, EhSource, FillCx,
    InterpPlan, PropertyPlan, RelUnit, RelaPlan, ShstrtabPlan, SizeCx, SynthChunk, SynthError,
    Window,
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
const SHF_EXECINSTR: u64 = 0x4;
const SHT_NOBITS: u32 = 8;
const SHT_RELA: u32 = 4;
const SHT_INIT_ARRAY: u32 = 14;
const SHT_FINI_ARRAY: u32 = 15;
const SHT_PREINIT_ARRAY: u32 = 16;
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
    /// `.note.mason` (`spec/odr.md` O-52), with `--odr-table`.
    pub note_mason: Option<&'a mortar_synth::NoteMasonPlan>,
    /// The context `.dynsym`'s exported values are read with, as
    /// `.symtab`'s (D-23 "exactly as `.symtab`").
    pub syms: SymCtx<'a>,
    /// A shared object's facts (`spec/dyn.md` D-100, D-102, D-116):
    /// `Some` exactly when `kind` is `DynKind::Shared`.
    pub shared: Option<SharedPlans<'a>>,
}

/// What the image of a shared object needs besides an executable's
/// plans (`spec/dyn.md` D-102, D-116; `spec/plumb.md` §3.2).
#[derive(Debug, Clone, Copy)]
pub struct SharedPlans<'a> {
    /// The `-soname` string and its `.dynstr` offset
    /// (`mortar_synth::dynstr::dynstr_plan_shared`): `Some` exactly when
    /// the kind's `SharedDyn::soname`.
    pub soname: Option<(&'a [u8], u32)>,
    /// The map's shared-object line records (`map::SharedLine::of(plan)`).
    pub line: &'a crate::map::SharedLine,
    /// D-115: the version definitions, their `.dynstr` offsets set
    /// (`mortar_synth::dynstr::dynstr_plan_versioned`); empty without a
    /// named version node.
    pub verdef: &'a mortar_synth::version::VerdefPlan,
}

/// The kind and the shared facts agree (an internal error otherwise: the
/// join builds both from one plan).
fn check_shared(d: &DynPlans<'_>) -> Result<(), Diagnostic> {
    let agree = match (d.kind, d.shared) {
        (DynKind::Shared(k), Some(s)) => k.soname == s.soname.is_some() && d.interp.is_none(),
        (DynKind::Exec | DynKind::Pie | DynKind::StaticPie, None) => true,
        _ => false,
    };
    if agree {
        Ok(())
    } else {
        Err(internal("image: the shared object's facts do not match its output kind"))
    }
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
    /// `.mason.meta` kept for tools (`spec/meta.md` MT-32), with
    /// `--keep-mason-meta` and some live unit; any output kind.
    pub mason_meta: Option<&'a mortar_synth::MasonMetaPlan>,
    /// `.gdb_index` (`spec/synth.md` §17), with `--gdb-index`.
    pub gdb_index: Option<&'a mortar_synth::GdbIndexPlan>,
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
    /// fill writes at the final addresses), each with the word fill must
    /// have written under it ([`crate::fill::Fixup`]).
    pub fixups: &'a [fill::Fixup],
    /// The command line's words, response files expanded, the program
    /// name excluded: the map's `arg` records, in every map
    /// (`spec/plumb.md` §3.2a, I-78).
    pub args: &'a [Vec<u8>],
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
    /// With `LinkOptions::keep_leaves`: `CHUNKS.members` (WP-255g), per
    /// chunk, from the plan's own model ([`layout_plan::member_counts`]).
    /// `None` in a plain link, and where no plan was built (an update's
    /// hand-made `Written`: its capture counts from its front).
    pub members: Option<Vec<u32>>,
}

fn internal(what: impl Into<String>) -> Diagnostic {
    Diagnostic::error(Kind::InternalError { what: what.into() })
}

fn failed(d: Diagnostic) -> Failed {
    Failed { diagnostics: vec![(Key::new(PHASE_IMAGE, 0, 0), d)] }
}

// ---- the tables ------------------------------------------------------------

pub use crate::rows::Valued;
use crate::rows::{FrontRows, RowTables, Rows};

/// The fill tables and the synthetic chunks' answers over one link: the
/// generic [`RowTables`] over the full link's rows (WP-255b).
pub type LinkTables<'a, 's> = RowTables<FrontRows<'a, 's>>;

impl<'a, 's> RowTables<FrontRows<'a, 's>> {
    /// The tables of `link`.
    #[must_use]
    pub fn new(link: Link<'a, 's>) -> Self {
        RowTables::from_rows(FrontRows::new(link))
    }

    /// The addresses of the synthetic chunks `.got`, `.got.plt`, `.plt`.
    #[must_use]
    pub fn got_addrs(&self) -> GotAddrs {
        self.rows.got_addrs
    }

    /// The link of these tables (WP-218: the map's decision records,
    /// `crate::map::decisions_of`).
    pub(crate) const fn link(&self) -> &Link<'a, 's> {
        &self.rows.link
    }

    fn value_in(&self, s: SectionId, v: u64) -> Valued {
        crate::rows::value_in(&self.rows, s, v)
    }

    fn def_entry(&self, key: SymKey) -> Option<SymEntry> {
        crate::rows::def_entry(&self.rows, key)
    }

    /// A global's value as an address for a consumer that has no use for
    /// the other cases (the map, `.dynamic`'s `DT_INIT`): `None` for
    /// each, so such a consumer omits the symbol; never 0 for "no place".
    fn global_addr(&self, id: SymbolId) -> Option<u64> {
        match self.rows.global.get(id.0 as usize)? {
            Valued::At(v) => Some(*v),
            Valued::Import | Valued::NoPlace | Valued::Missing => None,
        }
    }

    pub(crate) fn chunk_at(&self, c: ChunkId, offset: u64) -> Option<(u32, u64)> {
        crate::rows::chunk_at(&self.rows, c, offset)
    }

    fn tls_block(&self) -> Option<TlsBlock> {
        self.rows.tls()
    }

    fn kind_of(&self, s: SectionId) -> Option<SectionKind> {
        self.rows.section_kind(s)
    }

    /// The chunk holding input section `s`'s bytes (a `Merge` section's:
    /// its output section's merged chunk).
    fn chunk_of(&self, s: SectionId) -> Option<(ChunkId, u64)> {
        if self.kind_of(s)? == SectionKind::Merge {
            return Some((self.rows.merged_chunk(s)?, 0));
        }
        self.rows.place(s)
    }
}

impl RecordKeys for LinkTables<'_, '_> {
    /// As the scan decodes a record (`mortar_scan`'s `key_of`; model
    /// §3.3: `redirect` applies to undefined mentions).
    fn key(&self, file: FileId, r_sym: u32) -> Option<SymKey> {
        let local = |index| SymKey::local(SymEntry { file, index: SymbolIndex(index) });
        let Some(t) = self.rows.tables.get(file.0 as usize).copied().flatten() else {
            return (r_sym == 0).then(|| local(0));
        };
        if r_sym >= t.len() {
            return None;
        }
        let first = t.first_global().0;
        if r_sym < first {
            return Some(local(r_sym));
        }
        let id = *self.link().inputs.sym_map_of(file).get((r_sym - first) as usize)?;
        let to = self.link().resolution.redirect.get(id).copied().unwrap_or(id);
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
        crate::rows::address(&self.rows, key)
    }

    fn tls(&self) -> Option<TlsBlock> {
        self.tls_block()
    }

    fn dynamic(&self) -> Option<u64> {
        self.rows.dynamic
    }
}

impl SymAddrs for LinkTables<'_, '_> {
    fn section(&self, section: SectionId, value: u64) -> Option<(u32, u64)> {
        let (c, at) = self.chunk_of(section)?;
        let (shndx, base) = self.chunk_at(c, 0)?;
        let addr = match self.kind_of(section)? {
            SectionKind::Merge => match self.value_in(section, value) {
                Valued::At(a) => a,
                Valued::Import | Valued::NoPlace | Valued::Missing => return None,
            },
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
            // I-124: a target without a place has no address; the thunk
            // is then an internal error below, never a branch to 0.
            PatchKind::Thunk(ThunkTarget::Sym { key, addend }) => {
                let v = tables.sym(key);
                v.live.then(|| v.s.wrapping_add_signed(addend))
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
fn file_fixups(link: &Link<'_, '_>) -> Result<Vec<fill::Fixup>, Diagnostic> {
    let pl = &link.layout.placement;
    let mut spans: Vec<(u64, u64, u64)> = pl
        .chunks
        .iter()
        .filter(|(_, c)| c.in_file() && c.flags & SHF_ALLOC != 0)
        .filter_map(|(_, c)| Some((c.addr?, c.size, c.offset)))
        .collect();
    spans.sort_unstable();
    let mut out = Vec::with_capacity(link.fixups.len());
    for &(addr, word, under) in link.fixups {
        let at = spans.partition_point(|s| s.0 <= addr);
        let off = at
            .checked_sub(1)
            .map(|i| spans[i])
            .filter(|&(a, size, _)| addr + 4 <= a + size)
            .map(|(a, _, off)| off + (addr - a))
            .ok_or_else(|| internal(format!("image: a fixed word at {addr:#x} is in no chunk")))?;
        out.push((off, word, under));
    }
    out.sort_unstable_by_key(|f| f.0);
    Ok(out)
}

/// What the three structural chunks (I-40) are written from (WP-255b):
/// data lists, not the `Link`, so that an update writes them from its
/// own rows with the same writer ([`write_structure`]).
#[derive(Debug, Clone)]
pub struct StructureData<'d> {
    pub class: mortar_elf::Class,
    pub osabi: u8,
    pub e_machine: u16,
    pub e_type: u16,
    pub e_flags: u32,
    pub e_entry: u64,
    pub chunks: &'d Column<ChunkId, Chunk>,
    pub segments: &'d Column<SegmentId, Segment>,
    /// The section header table ([`section_headers_of`]).
    pub shdrs: Vec<Shdr>,
    /// `.shstrtab`'s section index (0: none).
    pub shstrndx: u32,
}

/// What the section header table (I-40, L-86) is written from: the
/// chunks, the synthetic chunks' kinds, `.shstrtab`'s names, the output
/// sections (a merge class's entsize) and the few values its synthetic
/// headers' `sh_link`/`sh_info` take.
#[derive(Clone, Copy)]
pub struct ShdrData<'d, 'n> {
    pub class: mortar_elf::Class,
    /// The target's word, in bytes.
    pub word: u64,
    /// REL relocations (i386).
    pub rel: bool,
    pub chunks: &'d Column<ChunkId, Chunk>,
    pub synth: &'d [(SynthKind, ChunkId)],
    pub shstrtab: &'d ShstrtabPlan,
    pub outsecs: &'d Column<OutSecId, OutSec<'n>>,
    /// `.symtab`'s `sh_info` (0 without one).
    pub symtab_info: u32,
    /// Whether the output has `.dynamic`.
    pub dynamic: bool,
    /// `.gnu.version_r`'s `sh_info`: the needed files.
    pub verneed: u32,
    /// `.gnu.version_d`'s `sh_info`: the definitions.
    pub verdef: u32,
}

impl std::fmt::Debug for ShdrData<'_, '_> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ShdrData").field("synth", &self.synth).finish_non_exhaustive()
    }
}

/// The bytes of the three structural chunks (I-40): the ELF header, the
/// program header table and the section header table, by chunk; their
/// data lists from `link`, written by [`write_structure`].
///
/// # Errors
///
/// `internal-error` when the placement's header chunks do not have the
/// size of what they hold, or a section's header cannot be built.
pub fn structure(
    link: &Link<'_, '_>,
) -> Result<std::collections::BTreeMap<ChunkId, Box<[u8]>>, Diagnostic> {
    let facts = link.arch.facts();
    let pl = &link.layout.placement;
    if !pl.chunks.iter().any(|(_, c)| c.kind == ChunkKind::FileHeader) {
        return Err(internal("image: no file header chunk (I-40)"));
    }
    let shdrs = section_headers_of(&ShdrData {
        class: facts.class,
        word: u64::from(facts.word),
        rel: matches!(facts.rel_form, mortar_arch::RelForm::Rel),
        chunks: &pl.chunks,
        synth: link.synth.chunks,
        shstrtab: link.synth.shstrtab,
        outsecs: &link.assignment.outsecs,
        symtab_info: link.synth.symtab.map_or(0, |s| s.0.sh_info()),
        dynamic: link.synth.dynamic.is_some(),
        verneed: link.layout.slots.verneed.len() as u32,
        verdef: link
            .synth
            .dynamic
            .as_ref()
            .and_then(|d| d.shared)
            .map_or(0, |s| u32::from(s.verdef.count())),
    })?;
    let shstrndx = synth_shndx(link, SynthKind::Shstrtab).unwrap_or(0);
    let e_type = match link.options.output {
        OutputKind::Static | OutputKind::Exec => ET_EXEC,
        OutputKind::Pie | OutputKind::Shared => ET_DYN,
        OutputKind::Relocatable => ET_REL,
    };
    let phnum = pl.segments.len();
    if u16::try_from(phnum).is_err() {
        return Err(internal(format!("image: {phnum} program headers")));
    }
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
                shared: matches!(d.kind, DynKind::Shared(_)),
            };
            mortar_synth::dynsym::gnu_symbols(d.dynsym, &inputs)
        });
    write_structure(StructureData {
        class: facts.class,
        osabi: if gnu { ELFOSABI_GNU } else { facts.osabi },
        e_machine: facts.e_machine,
        e_type,
        e_flags,
        e_entry: pl.entry.unwrap_or(0),
        chunks: &pl.chunks,
        segments: &pl.segments,
        shdrs,
        shstrndx,
    })
}

/// The writer of the three structural chunks (I-40) from their data
/// lists.
///
/// # Errors
///
/// `internal-error` when there is no file header chunk, when the header
/// chunks do not have the size of what they hold, or for more program
/// headers than `e_phnum` counts.
pub fn write_structure(
    d: StructureData<'_>,
) -> Result<std::collections::BTreeMap<ChunkId, Box<[u8]>>, Diagnostic> {
    let class = d.class;
    let find =
        |kind: ChunkKind| d.chunks.iter().find(|(_, c)| c.kind == kind).map(|(id, c)| (id, *c));
    let (eh_id, eh_chunk) = find(ChunkKind::FileHeader)
        .ok_or_else(|| internal("image: no file header chunk (I-40)"))?;
    let ph = find(ChunkKind::ProgramHeaders);
    let sh = find(ChunkKind::SectionHeaders);
    let phdrs: Vec<Phdr> = d
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
    let mut shdrs = d.shdrs;
    let shnum = shdrs.len() as u32;
    let shstrndx = d.shstrndx;
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
    let ehdr = Ehdr {
        osabi: d.osabi,
        e_type: d.e_type,
        e_machine: d.e_machine,
        e_entry: d.e_entry,
        e_phoff: ph.map_or(0, |p| p.1.offset),
        e_shoff: sh.map_or(0, |s| s.1.offset),
        e_flags: d.e_flags,
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

/// The section header table (I-40, L-86) from its data list: the null
/// header, then one per output section in index order.
///
/// # Errors
///
/// `internal-error` for a section without a name in `.shstrtab`, a
/// synthetic chunk without a plan, or a section index without a chunk.
pub fn section_headers_of(d: &ShdrData<'_, '_>) -> Result<Vec<Shdr>, Diagnostic> {
    let class = d.class;
    let word = d.word;
    let rel = d.rel;
    let secs = crate::rows::out_sections(d.chunks);
    let mut rows: Vec<Option<Shdr>> = vec![None; secs.len().max(1)];
    rows[0] = Some(Shdr::default());
    let kind_of = |id: ChunkId| d.synth.iter().find(|c| c.1 == id).map(|c| c.0);
    let shndx = |kind: SynthKind| {
        let id = d.synth.iter().find(|c| c.0 == kind)?.1;
        d.chunks.get(id)?.shndx
    };
    for (id, c) in d.chunks.iter() {
        let Some(n) = c.shndx else { continue };
        let n = n as usize;
        let sec = secs[n];
        let row = rows[n].get_or_insert(Shdr {
            sh_name: d.shstrtab.sh_name(n as u32).ok_or_else(|| {
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
                        row.sh_link = shndx(SynthKind::Strtab).unwrap_or(0);
                        row.sh_info = d.symtab_info;
                    }
                    SynthKind::SymtabShndx => {
                        row.sh_link = shndx(SynthKind::Symtab).unwrap_or(0);
                    }
                    // S-69: `sh_link` 0 in a static output (`.dynsym` in a
                    // dynamic one), `sh_info` `.got.plt`.
                    SynthKind::RelaPlt => {
                        row.sh_info = shndx(SynthKind::GotPlt).unwrap_or(0);
                        if d.dynamic {
                            row.sh_link = shndx(SynthKind::Dynsym).unwrap_or(0);
                        }
                    }
                    // gABI: a symbol table's and `.dynamic`'s strings, a
                    // hash table's, versym's and relocations' symbols.
                    SynthKind::Dynsym => {
                        row.sh_link = shndx(SynthKind::Dynstr).unwrap_or(0);
                        row.sh_info = 1;
                    }
                    SynthKind::Dynamic => {
                        row.sh_link = shndx(SynthKind::Dynstr).unwrap_or(0);
                    }
                    SynthKind::GnuHash | SynthKind::GnuVersion | SynthKind::RelaDyn => {
                        row.sh_link = shndx(SynthKind::Dynsym).unwrap_or(0);
                    }
                    SynthKind::GnuVersionR => {
                        row.sh_link = shndx(SynthKind::Dynstr).unwrap_or(0);
                        row.sh_info = d.verneed;
                    }
                    // D-115.
                    SynthKind::GnuVersionD => {
                        row.sh_link = shndx(SynthKind::Dynstr).unwrap_or(0);
                        row.sh_info = d.verdef;
                    }
                    _ => {}
                }
            }
            kind => {
                // L-50: the merge class's entsize when `SHF_MERGE` is kept;
                // the gABI's pointer arrays have entries of a word.
                if row.sh_flags & SHF_MERGE != 0 {
                    let class = kind.out().and_then(|o| d.outsecs.get(o)?.merge);
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
            shared: matches!(self.d.kind, DynKind::Shared(_)),
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
        self.t.global_addr(key)
    }

    fn soname(&self) -> Option<u32> {
        self.d.shared?.soname.map(|s| s.1)
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
    note_mason: Option<&'v mortar_synth::NoteMasonPlan>,
    gnu_hash: GnuHashChunk<'v>,
    dynsym: DynsymChunk<'v>,
    dynstr: DynstrChunk<'v>,
    versym: VersymChunk<'v>,
    verdef: VerdefChunk<'v>,
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
        let verdef = d.shared.map(|s| s.verdef);
        let verdefs = verdef.map_or(0, mortar_synth::version::VerdefPlan::count);
        let nobits = |kind: SynthKind| {
            Nobits(
                view.chunk_id(kind)
                    .and_then(|id| l.layout.placement.chunks.get(id))
                    .map_or(0, |c| c.size),
            )
        };
        Self {
            interp: d.interp,
            note_mason: d.note_mason,
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
                soname: d.shared.and_then(|s| s.soname),
                verdef,
                dynstr: &slots.dynstr,
                names: d.src,
            },
            versym: VersymChunk { plan: d.dynsym, inputs, verneed: &slots.verneed, verdefs, class },
            verdef: VerdefChunk { plan: verdef, class },
            verneed: VerneedChunk {
                needed: &slots.needed,
                verneed: &slots.verneed,
                verdefs,
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
            SynthKind::NoteMason => self.note_mason?,
            SynthKind::GnuHash => &self.gnu_hash,
            SynthKind::Dynsym => &self.dynsym,
            SynthKind::Dynstr => &self.dynstr,
            SynthKind::GnuVersion => &self.versym,
            SynthKind::GnuVersionD => &self.verdef,
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
fn rela_dyn(view: &DynView<'_, '_, '_>) -> Result<crate::dynrel::RelaDyn, Failed> {
    use crate::dynrel::{RecordError, SectionCx, SectionRecords, Unvalued};
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
    // into a split section through its piece, else the key's value. A
    // reference S-18 refuses is that refusal, as fill's in a static link
    // (WP-148); a piece the mark collected, or a key without a place, is
    // not live: the scan plans no record for either (P-1, I-7), so the
    // caller's internal error, never `RELATIVE` + 0 (I-124).
    let value = |key: SymKey, addend: i64| -> Result<u64, Unvalued> {
        match t.split_target(key, addend) {
            Some(Ok(Some(v))) => Ok(v),
            Some(Ok(None)) => Err(Unvalued::NotLive),
            Some(Err(d)) => Err(Unvalued::Refused(Box::new(d))),
            None => {
                let v = t.sym(key);
                debug_assert!(v.live, "P-1: a RELATIVE record planned for a key without a place");
                if v.live { Ok(v.s.wrapping_add_signed(addend)) } else { Err(Unvalued::NotLive) }
            }
        }
    };
    let dynsym = |id: SymbolId| d.dynsym.index(id);
    // Per section: its address and records, or the refusals of its
    // records' references (S-18, in record order, each at its referring
    // place with fill's key, so that every refused record is reported, as
    // fill reports them in a static link), or an internal fault.
    let per: Vec<Result<(u64, SectionRecords), Failed>> = l.pool.map(sections.len(), |i| {
        let sid = sections[i];
        let what = |m: &str| failed(internal(format!(".rela.dyn: section {}: {m}", sid.0)));
        let (file, index) = l.resolution.sh_index(sid).ok_or_else(|| what("no input section"))?;
        let rel = l.resolution.sections.get(sid).ok_or_else(|| what("no row"))?.reloc.0;
        let addr = pl.section_addr(sid, 0).ok_or_else(|| what("no address"))?;
        let leads = l.records.leads(sid);
        let mut out = SectionRecords::default();
        let mut err = None;
        let mut refused: Vec<(Key, Diagnostic)> = Vec::new();
        l.facts
            .walk_records(file, sid, index.0, rel, leads, &mut |cx, at| {
                if err.is_some() {
                    return;
                }
                let scx = SectionCx { arch, scan: *cx, addr, value: &value, dynsym: &dynsym };
                match crate::dynrel::push_records(&scx, at, &mut out) {
                    Ok(()) => {}
                    Err(RecordError::Fault(e)) => err = Some(e),
                    Err(RecordError::Refused(d)) => {
                        let d = *d;
                        let d = if d.place().is_some() {
                            d
                        } else {
                            let name = match &l.inputs.files.get(file).map(|f| &f.reader) {
                                Some(Reader::Object(o)) => o
                                    .section(index)
                                    .ok()
                                    .and_then(|h| h.name().ok())
                                    .map(|n| String::from_utf8_lossy(n).into_owned())
                                    .unwrap_or_default(),
                                _ => String::new(),
                            };
                            let path = layout_plan::file_ref(l.inputs, file);
                            d.at(mortar_diag::Place::section(path, name, at.r_offset))
                        };
                        refused.push((mortar_arch::cx::fill_key(file, index.0, at.index), d));
                    }
                }
            })
            .map_err(failed)?;
        if let Some(e) = err {
            return Err(what(&e));
        }
        if !refused.is_empty() {
            return Err(Failed { diagnostics: refused });
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
    let mut refused = Vec::new();
    for r in per {
        match r {
            Ok(r) => recs.push(r),
            // An internal fault stops at once; refusals are all reported.
            Err(f)
                if f.diagnostics
                    .iter()
                    .any(|(_, d)| matches!(d.kind(), Kind::InternalError { .. })) =>
            {
                return Err(f);
            }
            Err(mut f) => refused.append(&mut f.diagnostics),
        }
    }
    if !refused.is_empty() {
        refused.sort_by_key(|r| r.0);
        return Err(Failed { diagnostics: refused });
    }
    let mut got_relative = Vec::new();
    let mut other = Vec::new();
    for r in got.got_records() {
        let r_offset = ga.got + r.offset;
        if r.role == DynRole::Relative {
            let v =
                r.key.and_then(|k| got.iplt_address(&ga, k).or_else(|| t.address(k))).ok_or_else(
                    || failed(internal(".rela.dyn: a GOT RELATIVE record without a value")),
                )?;
            got_relative.push(DynRecord { r_offset, r_sym: 0, r_type: r.r_type, addend: v as i64 });
        } else {
            // D-119: a shared object's `r_sym` 0 TLS record carries the
            // key's offset in the block; every other one 0.
            let addend = got.record_addend(r, t).map_err(|e| failed(e.into_diagnostic()))?;
            other.push(DynRecord { r_offset, r_sym: r.r_sym, r_type: r.r_type, addend });
        }
    }
    got_relative.sort_unstable_by_key(|r| r.r_offset);
    if !l.layout.slots.copies.is_empty() {
        let r_type = arch
            .dyn_type(DynRole::Copy)
            .ok_or_else(|| failed(internal(".rela.dyn: no COPY type (D-42)")))?;
        for (a, c) in l.layout.slots.copies.iter().enumerate() {
            let (_, r_offset) = view
                .copy(a as u32)
                .ok_or_else(|| failed(internal(format!(".rela.dyn: copy {a} has no address"))))?;
            let r_sym = d.dynsym.index(c.alias).ok_or_else(|| {
                failed(internal(format!(".rela.dyn: copy {a}'s key not in .dynsym")))
            })?;
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
    .map_err(|m| failed(internal(format!(".rela.dyn: {m}"))))?;
    for (_, r) in recs {
        other.extend(r.other);
    }
    // S-70: a `StaticPie`'s IPLT `IRELATIVE`s, last.
    let irelative = got.irelative_records(&ga, t).map_err(|e| failed(e.into_diagnostic()))?;
    crate::dynrel::RelaDyn::new(d.rela, class, form, relative, other, irelative)
        .map_err(|e| failed(e.into_diagnostic()))
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
            SynthKind::MasonMeta => link.synth.mason_meta?,
            SynthKind::GdbIndex => link.synth.gdb_index?,
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
    with_image(link, path, true, |img, plan, sealed, steps, members| {
        let out = Output::create(path, opts).map_err(|kd| Failed { diagnostics: vec![kd] })?;
        steps.step("create", 0, "");
        let (filled, checks) = fill::write_checked(img, &out, out.shown(), link.pool);
        *checked = true;
        // I-14: the scan's diagnostics of the deferred sections fill found a
        // fault in, or did not reach; of all of them when a job stopped early.
        let mut report_on =
            if checks.complete { checks.flagged.clone() } else { link.records.deferred().to_vec() };
        report_on.extend(unreached(link, plan));
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
                changed
                    .push((Key::new(PHASE_IMAGE, file.0, 0), crate::source::copy_error(e, &path)));
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
            leaves: img.leaf_fn.zip(filled.leaves),
            sealed,
            file_size: plan.file_size,
            file: written,
            rows: plan.rows.len(),
            jobs: plan.jobs.len(),
            members,
        })
    })
}

/// The image of `link` (§5, §13): the tables, the structural units, the
/// synthetic chunks and the fill plan, given to `go` with the plan, the
/// build id's sealed range and the phase's steps. [`write`] fills every
/// job of the plan into the output; the test-only update path
/// ([`refill`], `spec/incr.md` WP-061 stage B) fills the jobs of the
/// dirty windows into memory.
#[allow(clippy::too_many_lines)]
fn with_image<T>(
    link: &Link<'_, '_>,
    path: &Path,
    count_members: bool,
    go: impl FnOnce(
        &Image<'_>,
        &crate::plan::Plan,
        Option<std::ops::Range<u64>>,
        &mut mortar_core::wp032b_steps::Steps,
        Option<Vec<u32>>,
    ) -> Result<T, Failed>,
) -> Result<T, Failed> {
    let mut steps = mortar_core::wp032b_steps::Steps::new("image");
    let tables = LinkTables::new(*link);
    steps.step("tables", u64::from(link.inputs.symbols.len()), "symbol");
    let mut fixed = structure(link).map_err(failed)?;
    fixed.append(&mut patches(link, &tables).map_err(failed)?);
    steps.step("structure", 0, "");
    let chunks = Chunks::new(link);
    if let Some(d) = &link.synth.dynamic {
        check_shared(d).map_err(failed)?;
    }
    let view = link.synth.dynamic.map(|d| DynView { t: &tables, d });
    let dyn_chunks = match &view {
        Some(v) => Some(DynChunks::new(v, rela_dyn(v)?)),
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
    let mut plan =
        layout_plan::from_layout(&model, tables.rows(), target, &extras, IMAGE_BLOCK, link.pool)
            .map_err(failed)?;
    plan.deferred_order = deferred_orders(link, &plan)?;
    steps.step("plan", plan.rows.len() as u64, "row");
    // WP-255g: `CHUNKS.members` from this plan's model, in incremental
    // mode only (a plain link counts nothing). WP-255e4 item 2: only for
    // the caller that stores it (`write_link`); the update's refill paths
    // do not count (the update's capture counts from its front by the same
    // rule, `incremental.rs`, so a count here would be thrown away).
    let members = if count_members && link.options.keep_leaves {
        Some(layout_plan::member_counts(&model).map_err(failed)?)
    } else {
        None
    };
    let output_ref = FileRef::new(path.display().to_string());
    let code_addr = |c: CodeRef| -> Option<u64> {
        let off = u64::try_from(c.offset).ok()?;
        tables.addrs.addr_in(c.section, off)
    };
    // spec/synth.md S-G2: a non-allocated input section's offset from its
    // output section's start (`.gdb_index`'s CU list).
    let section_offset = |s: SectionId| -> Option<u64> {
        let (c, at) = link.layout.placement.place.get(s)?.get()?;
        Some(tables.chunk_at(c, at)?.1)
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
            section_offset: &section_offset,
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
    go(&img, &plan, sealed, &mut steps, members)
}

/// What [`refill`] filled: the jobs of the dirty windows, in memory.
#[derive(Debug)]
pub struct Refilled {
    /// Each job (I-31) that overlaps a dirty window: its file range and
    /// its bytes, the build id's descriptor as zeros (I-73).
    pub jobs: Vec<(std::ops::Range<u64>, Vec<u8>)>,
    /// The leaves of those jobs' windows, by window index, in the
    /// function [`Refilled::leaf_fn`].
    pub leaves: Vec<(u64, mortar_digest::Leaf)>,
    /// The leaves' function: the build id's, else L128 (I-73, IN-21).
    pub leaf_fn: mortar_digest::TreeFn,
    /// The build id's style, when the link computes one.
    pub build_id: Option<TreeStyle>,
    /// The build id's descriptor in the file.
    pub sealed: Option<std::ops::Range<u64>>,
    /// The output file's size.
    pub file_size: u64,
    /// The plan's rows and jobs (as [`Written`]'s).
    pub rows: usize,
    pub total_jobs: usize,
}

/// The test-only update path's image phase (`spec/incr.md` §5.2, I-95;
/// WP-061 stage B): every job of `link`'s plan that overlaps one of the
/// window indices `windows` (sorted), filled into memory as [`write`]
/// fills it, and the leaves of its windows. Nothing is written (IN-13).
/// A job is filled whole: a window inside a whole synthetic chunk brings
/// the chunk's other windows (I-31).
///
/// # Errors
///
/// Every diagnostic of the fill; a fault in a section deferred to fill
/// (I-14) is an internal error here.
pub fn refill(link: &Link<'_, '_>, path: &Path, windows: &[u64]) -> Result<Refilled, Failed> {
    with_image(link, path, false, |img, plan, sealed, steps, _| {
        let w = plan.window;
        let dirty = |j: &std::ops::Range<u64>| {
            let first =
                windows.partition_point(|&k| k.saturating_add(1).saturating_mul(w) <= j.start);
            windows.get(first).is_some_and(|&k| k.saturating_mul(w) < j.end)
        };
        let jobs: Vec<std::ops::Range<u64>> =
            plan.jobs.iter().filter(|j| dirty(j)).cloned().collect();
        let leaf_fn = img.build_id.map_or(mortar_digest::TreeFn::L128, TreeStyle::tree_fn);
        let filled = link.pool.map(jobs.len(), |k| {
            let j = jobs[k].clone();
            let mut buf = vec![0u8; (j.end - j.start) as usize];
            let mut errs = Vec::new();
            let r = fill::fill_range(img, j.start, &mut buf, &mut errs);
            let leaves: Vec<(u64, mortar_digest::Leaf)> = buf
                .chunks(w as usize)
                .enumerate()
                .map(|(i, win)| (j.start / w + i as u64, mortar_digest::leaf(leaf_fn, win)))
                .collect();
            (buf, errs, r, leaves)
        });
        let mut diagnostics = Vec::new();
        let mut out = Vec::with_capacity(jobs.len());
        let mut leaves = Vec::new();
        for (j, (buf, errs, r, l)) in jobs.into_iter().zip(filled) {
            diagnostics.extend(errs);
            if let Err(kd) = r {
                diagnostics.push(kd);
            }
            leaves.extend(l);
            out.push((j, buf));
        }
        steps.step("refill", out.iter().map(|j| j.1.len() as u64).sum(), "byte");
        // WP-255 §9.1, the written-bitmap rule: every clip byte a row's.
        #[cfg(debug_assertions)]
        for (j, _) in &out {
            if let Err(d) = crate::clip_plan::check_written(plan, &link.layout.placement.chunks, j)
            {
                diagnostics.push((Key::new(PHASE_IMAGE, 0, 0), d));
            }
        }
        if !diagnostics.is_empty() {
            return Err(Failed { diagnostics });
        }
        Ok(Refilled {
            jobs: out,
            leaves,
            leaf_fn,
            build_id: img.build_id,
            sealed,
            file_size: plan.file_size,
            rows: plan.rows.len(),
            total_jobs: plan.jobs.len(),
        })
    })
}

/// The update's image phase (`spec/image.md` I-95; WP-255a): the file
/// ranges `ranges` (sorted, disjoint) of `link`'s output, filled into
/// memory by [`fill::fill_range`] from the WHOLE plan, as [`write`] fills
/// them; the 1 MiB block is the unit of the leaf hash, not of the fill.
/// Each range is cut at the plan's job boundaries (I-31: the parallel
/// unit), and a piece that touches a whole synthetic chunk (I-104) is
/// widened to the chunk, so that one call writes it; pieces of one job
/// that then touch are merged. [`Refilled::jobs`] are the pieces, sorted
/// and disjoint, and cover every range; [`Refilled::leaves`] is empty
/// (the caller hashes its blocks). Nothing is written (IN-13).
///
/// # Errors
///
/// Every diagnostic of the fill; a fault in a section deferred to fill
/// (I-14) is an internal error here.
pub(crate) fn refill_ranges(
    link: &Link<'_, '_>,
    ranges: &[std::ops::Range<u64>],
) -> Result<Refilled, Failed> {
    with_image(link, Path::new(""), false, |img, plan, sealed, steps, _| {
        let jobs = clip_jobs(plan, ranges);
        let leaf_fn = img.build_id.map_or(mortar_digest::TreeFn::L128, TreeStyle::tree_fn);
        let filled = link.pool.map(jobs.len(), |k| {
            let j = jobs[k].clone();
            let mut buf = vec![0u8; (j.end - j.start) as usize];
            let mut errs = Vec::new();
            let r = fill::fill_range(img, j.start, &mut buf, &mut errs);
            (buf, errs, r)
        });
        let mut diagnostics = Vec::new();
        let mut out = Vec::with_capacity(jobs.len());
        for (j, (buf, errs, r)) in jobs.into_iter().zip(filled) {
            diagnostics.extend(errs);
            if let Err(kd) = r {
                diagnostics.push(kd);
            }
            out.push((j, buf));
        }
        steps.step("refill", out.iter().map(|j| j.1.len() as u64).sum(), "byte");
        if !diagnostics.is_empty() {
            return Err(Failed { diagnostics });
        }
        Ok(Refilled {
            jobs: out,
            leaves: Vec::new(),
            leaf_fn,
            build_id: img.build_id,
            sealed,
            file_size: plan.file_size,
            rows: plan.rows.len(),
            total_jobs: plan.jobs.len(),
        })
    })
}

/// The fill pieces of [`refill_ranges`]: each of `ranges` (sorted,
/// disjoint) cut at the plan's job boundaries, widened over a whole
/// synthetic chunk it touches (I-104; such a chunk lies inside one job,
/// I-31), the pieces of one job merged where they then touch. Sorted,
/// disjoint, clipped to the file.
fn clip_jobs(
    plan: &crate::plan::Plan,
    ranges: &[std::ops::Range<u64>],
) -> Vec<std::ops::Range<u64>> {
    let whole_row = |at: u64| -> Option<std::ops::Range<u64>> {
        let row = plan.rows.get(plan.first_row(at))?;
        let crate::plan::Unit::Synth(i) = row.unit else { return None };
        let whole = plan.synth.get(i as usize).is_some_and(|s| s.whole);
        (whole && row.offset <= at).then(|| row.offset..row.end())
    };
    let mut out: Vec<std::ops::Range<u64>> = Vec::new();
    for r in ranges {
        let r = r.start..r.end.min(plan.file_size);
        if r.start >= r.end {
            continue;
        }
        let first = plan.jobs.partition_point(|j| j.end <= r.start);
        for j in plan.jobs[first..].iter().take_while(|j| j.start < r.end) {
            let (mut lo, mut hi) = (r.start.max(j.start), r.end.min(j.end));
            if let Some(w) = whole_row(lo) {
                lo = lo.min(w.start).max(j.start);
            }
            if let Some(w) = whole_row(hi - 1) {
                hi = hi.max(w.end).min(j.end);
            }
            match out.last_mut() {
                // The same job (a job's pieces come in order): merge.
                Some(l) if lo <= l.end && l.start >= j.start => l.end = l.end.max(hi),
                _ => out.push(lo..hi),
            }
        }
    }
    out
}

/// The model of `link` the plan makers read.
fn clip_model<'l>(link: &Link<'l, '_>) -> Model<'l, 'l> {
    Model {
        inputs: link.inputs,
        resolution: link.resolution,
        liveness: link.liveness,
        split: link.split,
        layout: link.layout,
        assignment: link.assignment,
        order: Some(link.order),
        eh: link.eh.map(|e| e.1),
    }
}

/// The member index of `link` ([`crate::clip_plan::Members`], WP-255c2b):
/// for each in-file `Body` chunk, every section `from_layout` makes an
/// `Input` row of ([`layout_plan::is_member`] and
/// [`layout_plan::member_front`]), in offset order, with its input
/// header's alignment. A full link's form of what an update reads from a
/// stored table (255e). O(sections).
///
/// # Errors
///
/// An unreadable input section header.
pub fn clip_members(link: &Link<'_, '_>) -> Result<crate::clip_plan::Members, Diagnostic> {
    use crate::clip_plan::{FragMember, Member, Members, MergedMembers};
    let model = clip_model(link);
    let pl = &link.layout.placement;
    let mut by: std::collections::BTreeMap<ChunkId, Vec<(u64, Member)>> =
        std::collections::BTreeMap::new();
    for (s, place) in pl.place.iter() {
        let Some((c, off)) = place.get() else { continue };
        // c2b-1: the ONE member rule (`clip_plan::is_clip_member`), on this
        // link's fields; the update's index calls it on stored rows.
        let row = &link.resolution.sections[s];
        let Some((f, i)) = link.resolution.sh_index(s) else { continue };
        let Reader::Object(o) = &link.inputs.files[f].reader else { continue };
        let sec = o.section(i).map_err(|e| {
            Diagnostic::error(Kind::InternalError { what: format!("image: {e:?}") })
        })?;
        if !crate::clip_plan::is_clip_member(
            matches!(row.state, mortar_core::resolution::SectionState::Kept),
            row.kind.is_placed(),
            link.liveness.is_live(s),
            link.layout.folds.is_folded(s),
            sec.sh_type(),
            sec.size(),
            &pl.chunks[c],
        ) {
            continue;
        }
        // The row's own maker agrees (`from_layout`: `is_member` and
        // `member_front`); a member it would not make a row of is an
        // internal error, not a second rule.
        if !layout_plan::is_member(&model, s)
            || layout_plan::member_front(&model, s, &pl.chunks[c])?.is_none()
        {
            return Err(Diagnostic::error(Kind::InternalError {
                what: format!(
                    "image: section {} is a member by the member rule and takes no row",
                    s.0
                ),
            }));
        }
        let align = sec.alignment().unwrap_or(1);
        by.entry(c).or_default().push((off, Member { section: s, align }));
    }
    let by_chunk = by
        .into_iter()
        .map(|(c, mut v)| {
            v.sort_by_key(|m| m.0);
            (c, v.into_iter().map(|m| m.1).collect())
        })
        .collect();
    // The merged chunks: the whole plan's unit (`layout_plan::merged`) and
    // the fragment order's placed fragments.
    let mut merged = std::collections::BTreeMap::new();
    for (c, ch) in pl.chunks.iter() {
        let ChunkKind::Merged(o) = ch.kind else { continue };
        if !ch.in_file() || ch.size == 0 {
            continue;
        }
        let Ok(k) = link.order.chunks.binary_search_by_key(&o, |mc| mc.out) else {
            return Err(Diagnostic::error(Kind::InternalError {
                what: format!("image: merged chunk {} is not in the fragment order", c.0),
            }));
        };
        let span = link.order.chunks[k].frags;
        let mut frags: Vec<(u64, FragMember)> = link.order.order
            [span.start as usize..(span.start + span.len) as usize]
            .iter()
            .filter_map(|&f| {
                let row = link.split.frags[f];
                let off = link.layout.merge.offset(f)?;
                Some((
                    off,
                    FragMember {
                        frag: f,
                        section: row.section,
                        size: u64::from(row.size),
                        align: 1u64 << row.align.min(63),
                    },
                ))
            })
            .collect();
        frags.sort_by_key(|f| (f.0, f.1.frag.0));
        merged.insert(
            c,
            MergedMembers {
                order: k as u32,
                name: link.assignment.outsecs[o].name.into(),
                frags: frags.into_iter().map(|f| f.1).collect(),
            },
        );
    }
    Ok(Members { by_chunk, merged })
}

/// The inputs of [`crate::clip_plan::for_clips`] for `ranges` from
/// `link` (WP-255c2a; an update finds them in its state, 255e): the member
/// index ([`clip_members`], changed by `members_edit`, a test's hook), the
/// sections' fronts, the pad views, the holes of the chunks a range meets,
/// the abbreviation table's offset and the structural chunks' bytes;
/// WP-255c2c: the synthetic chunks' rows (`synth`, the fill's chunk list's
/// indices, from the whole plan's `Synth` units), the input `.eh_frame`
/// sections of a range of the output `.eh_frame` (from the `.eh_frame`
/// plan's records), the patch islands' bytes ([`patches`]) and the build
/// id's descriptor; the plan over `rows`.
fn clip_plan_of<R: Rows>(
    link: &Link<'_, '_>,
    rows: &R,
    ranges: &[std::ops::Range<u64>],
    members_edit: impl FnOnce(&mut crate::clip_plan::Members),
    synth: &std::collections::BTreeMap<ChunkId, (SynthKind, SynthRow)>,
    sealed: Option<&std::ops::Range<u64>>,
) -> Result<crate::plan::Plan, crate::clip_plan::ClipError> {
    use crate::clip_plan::ClipParts;
    let model = clip_model(link);
    let pl = &link.layout.placement;
    let meets = |lo: u64, hi: u64| ranges.iter().any(|r| r.start < hi && lo < r.end && lo < hi);
    let mut members = clip_members(link)?;
    members_edit(&mut members);
    let front =
        |s: SectionId, (c, _): (ChunkId, u64)| layout_plan::member_front(&model, s, &pl.chunks[c]);
    let (chunk_views, seg_views) = layout_plan::pad_views(&model);
    let holes: Vec<(mortar_core::layout::Hole, &[u8])> = pl
        .holes
        .iter()
        .filter(|h| {
            let c = &pl.chunks[h.chunk];
            meets(c.offset, c.offset + c.size.max(c.capacity))
        })
        .map(|h| (*h, pl.chunks[h.chunk].name(link.assignment).unwrap_or(b"")))
        .collect();
    let structure = structure(link)?;
    let tables = LinkTables::new(*link);
    let patches = patches(link, &tables)?;
    let eh_sections = |r: std::ops::Range<u64>| clip_eh_sections(link, &model, r);
    let parts = ClipParts {
        target: Target::from_facts(link.arch.facts()),
        chunk_views: &chunk_views,
        seg_views: &seg_views,
        separate_code: link.options.separate_code,
        holes: &holes,
        abbrev: layout_plan::abbrev_offset(&pl.holes, &pl.chunks)?,
        structure: &structure,
        proof: link.proof,
        merge_source: &|s| layout_plan::merge_source(&model, s),
        synth,
        eh_sections: &eh_sections,
        patches: &patches,
        sealed,
    };
    crate::clip_plan::for_clips(rows, ranges, &members, &front, &parts)
}

/// The full link's [`crate::clip_plan::ClipParts::eh_sections`]
/// (WP-255c2c): the input `.eh_frame` sections of the kept records that
/// meet `[a, b)` of the output `.eh_frame` (`eh::relocate`'s record
/// search over the `.eh_frame` plan), each as `eh_sections` lists it
/// ([`layout_plan::eh_part`]). O(log records + records met).
#[allow(clippy::type_complexity)]
fn clip_eh_sections(
    link: &Link<'_, '_>,
    model: &Model<'_, '_>,
    r: std::ops::Range<u64>,
) -> Result<Vec<(SectionId, FileId, SectionIndex, crate::plan::RelaTable)>, Diagnostic> {
    let Some((plan, src)) = link.eh else {
        return Err(internal("image: clip plan: the .eh_frame chunk and no .eh_frame plan"));
    };
    let frames = src.frames();
    let n = plan.records().len();
    let (mut lo, mut hi) = (0usize, n);
    while lo < hi {
        let mid = lo + (hi - lo) / 2;
        if plan.out_offset(mid + 1) <= r.start {
            lo = mid + 1;
        } else {
            hi = mid;
        }
    }
    let mut out = Vec::new();
    let mut seen = std::collections::BTreeSet::new();
    for k in lo..n {
        if plan.out_offset(k) >= r.end {
            break;
        }
        let Some(sec) = frames.sections.get(plan.source(k).0) else { continue };
        if seen.insert(sec.section)
            && let Some(p) = layout_plan::eh_part(model, sec.section)?
        {
            out.push(p);
        }
    }
    Ok(out)
}

/// [`crate::clip_plan::sections_in`] over `link`'s rows and
/// [`clip_members`] for each `(chunk, range)` query (a test's path,
/// WP-255c2b).
///
/// # Errors
///
/// [`clip_members`]' error.
#[doc(hidden)]
#[allow(clippy::type_complexity)]
pub fn clip_sections_for_test(
    link: &Link<'_, '_>,
    queries: &[(ChunkId, std::ops::Range<u64>)],
) -> Result<Vec<Result<Vec<crate::clip_plan::Placed>, crate::clip_plan::ClipError>>, Diagnostic> {
    let tables = LinkTables::new(*link);
    let members = clip_members(link)?;
    Ok(queries
        .iter()
        .map(|(c, r)| crate::clip_plan::sections_in(tables.rows(), &members, *c, r.clone()))
        .collect())
}

/// [`crate::clip_plan::frags_in`] over `link`'s rows and
/// [`clip_members`] (a test's path, WP-255c2b).
///
/// # Errors
///
/// [`clip_members`]' error, or `frags_in`'s.
#[doc(hidden)]
pub fn clip_frags_for_test(
    link: &Link<'_, '_>,
    c: ChunkId,
    range: std::ops::Range<u64>,
) -> Result<Vec<(mortar_core::FragId, crate::clip_plan::Placed)>, crate::clip_plan::ClipError> {
    let tables = LinkTables::new(*link);
    let members = clip_members(link)?;
    crate::clip_plan::frags_in(tables.rows(), &members, c, range)
}

/// What [`refill_clip_plan`] built and filled.
#[derive(Debug)]
pub struct ClipFill {
    /// The whole plan of the link.
    pub whole: crate::plan::Plan,
    /// The clip plan of the ranges, as filled (after the caller's edit).
    pub clip: crate::plan::Plan,
    /// The fill pieces ([`clip_jobs`] of the clip plan) and their bytes.
    pub jobs: Vec<(std::ops::Range<u64>, Vec<u8>)>,
}

/// WP-255c2a's clip filler, a test path until 255e: the clip plan of
/// `ranges` (sorted, disjoint) built from `link`'s rows
/// ([`crate::clip_plan::for_clips`]), changed by `edit` (a test's hook:
/// identity in every other use), and its pieces filled by
/// [`fill::fill_range`] as [`refill_ranges`] fills the whole plan's; under
/// `debug_assertions`, the written-bitmap rule over every piece (§9.1).
/// Nothing is written.
///
/// # Errors
///
/// `Err(Ok(why))`: the clip plan was not built
/// ([`crate::clip_plan::ClipError`]); `Err(Err(failed))`: every diagnostic
/// of the fill, and the bitmap rule's `internal-error`.
pub fn refill_clip_plan(
    link: &Link<'_, '_>,
    ranges: &[std::ops::Range<u64>],
    edit: impl FnOnce(&mut crate::plan::Plan),
) -> Result<ClipFill, Result<crate::clip_plan::ClipError, Failed>> {
    refill_clip_plan_with(link, ranges, |_| {}, edit)
}

/// [`refill_clip_plan`] with the member index changed by `members_edit`
/// first (a test's hook, WP-255c2b: one member dropped).
///
/// # Errors
///
/// As [`refill_clip_plan`].
pub fn refill_clip_plan_with(
    link: &Link<'_, '_>,
    ranges: &[std::ops::Range<u64>],
    members_edit: impl FnOnce(&mut crate::clip_plan::Members),
    edit: impl FnOnce(&mut crate::plan::Plan),
) -> Result<ClipFill, Result<crate::clip_plan::ClipError, Failed>> {
    let tables = LinkTables::new(*link);
    // The fill's chunk list is `with_image`'s: the synthetic rows' indices
    // are the whole plan's `Synth` units' (a test path's source; an update
    // gives those of the chunk list it fills with).
    let mut built: Option<Result<crate::plan::Plan, crate::clip_plan::ClipError>> = None;
    let r = with_image(link, Path::new(""), false, |img, whole, sealed, _, _| {
        let synth: std::collections::BTreeMap<ChunkId, (SynthKind, SynthRow)> = whole
            .synth
            .iter()
            .filter_map(|u| {
                let kind = link.synth.chunks.iter().find(|c| c.1 == u.id)?.0;
                Some((u.id, (kind, SynthRow { index: u.chunk, whole: u.whole, eh: u.eh })))
            })
            .collect();
        let mut clip = match clip_plan_of(
            link,
            tables.rows(),
            ranges,
            members_edit,
            &synth,
            sealed.as_ref(),
        ) {
            Ok(c) => c,
            Err(e) => {
                built = Some(Err(e));
                return Ok(None);
            }
        };
        edit(&mut clip);
        clip.deferred_order = deferred_orders(link, &clip)?;
        let cimg = Image { plan: &clip, ..*img };
        let jobs = clip_jobs(&clip, ranges);
        let mut diagnostics = Vec::new();
        let mut out = Vec::with_capacity(jobs.len());
        for j in jobs {
            let mut buf = vec![0u8; (j.end - j.start) as usize];
            let mut errs = Vec::new();
            if let Err(kd) = fill::fill_range(&cimg, j.start, &mut buf, &mut errs) {
                errs.push(kd);
            }
            diagnostics.extend(errs);
            #[cfg(debug_assertions)]
            if let Err(d) =
                crate::clip_plan::check_written(&clip, &link.layout.placement.chunks, &j)
            {
                diagnostics.push((Key::new(PHASE_IMAGE, 0, 0), d));
            }
            out.push((j, buf));
        }
        if !diagnostics.is_empty() {
            return Err(Failed { diagnostics });
        }
        Ok(Some(ClipFill { whole: whole.clone(), clip: clip.clone(), jobs: out }))
    });
    match (r, built) {
        (Ok(None), Some(Err(e))) => Err(Ok(e)),
        (Ok(Some(f)), _) => Ok(f),
        (Ok(None), _) => Err(Ok(crate::clip_plan::ClipError::Internal(Box::new(internal(
            "image: clip plan: not built",
        ))))),
        (Err(f), _) => Err(Err(f)),
    }
}

/// [`refill_ranges`] for a test outside the crate (WP-255c2a): the whole
/// plan's fill of `ranges`' pieces.
///
/// # Errors
///
/// As [`refill_ranges`].
#[doc(hidden)]
pub fn refill_ranges_for_test(
    link: &Link<'_, '_>,
    ranges: &[std::ops::Range<u64>],
) -> Result<Refilled, Failed> {
    refill_ranges(link, ranges)
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
                            let Some(addr) = crate::rows::frag_value(&self.rows, frag) else {
                                continue;
                            };
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
        // A NOBITS synthetic chunk is listed when it holds no allocation
        // of its own (`spec/plumb.md` §3.2, the allocation records: today
        // `.pad.relro`); the copy chunks' allocations are `copy` records.
        let holds: std::collections::BTreeSet<_> = pl.sym_place.iter().map(|r| r.chunk).collect();
        for (id, c) in pl.chunks.iter() {
            if let ChunkKind::Synthetic(s) = c.kind
                && c.size > 0
                && (c.sh_type != SHT_NOBITS || !holds.contains(&id))
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
        // The `common` records (§3.2): each common's allocation, in
        // `SymbolId` order (`sym_place` is sorted by key), at the
        // alignment layout placed it (L-88).
        for r in &pl.sym_place {
            let AllocKey::Common(id) = r.key else { continue };
            let (Some(k), Some((_, addr))) =
                (l.inputs.symbols.get(id), self.chunk_at(r.chunk, r.offset))
            else {
                continue;
            };
            m.commons.push(map::MapCommon {
                name: k.name.to_vec(),
                addr,
                size: r.size,
                align: 1u64 << r.align.min(63),
            });
        }
        for d in &pl.defined {
            if let (Some(k), Some(v)) = (l.inputs.symbols.get(d.symbol), pl.defined_of(d.symbol)) {
                m.defines.push((k.name.to_vec(), v));
            }
        }
        m.incremental = self.incremental_layout();
        m.dead = self.dead_lines();
        // WP-218: an incremental-mode map's decision records (I-78a).
        if m.incremental {
            m.decisions = Some(map::decisions_of(self));
        }
        if let Some(d) = l.synth.dynamic {
            m.dynamic = Some(self.dyn_records(d));
        }
        m.args = l.args.to_vec();
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
        let pos = |c: &Chunk| c.addr.unwrap_or(c.offset);
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
                    HoleKind::Fill | HoleKind::DeadSlot => None,
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
            let mut alloc: Vec<&Chunk> = pl
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
        // `spec/plumb.md` §3.2: the shared-object line records.
        r.shared = d.shared.map(|s| s.line.clone());
        r.needed = slots.needed.iter().map(|&f| d.src.needed(f).to_vec()).collect();
        r.copies = slots
            .copies
            .iter()
            .enumerate()
            .filter_map(|(a, c)| {
                let row = l.layout.placement.sym_place_of(AllocKey::Copy(a as u32))?;
                Some(CopyHint {
                    name: d.src.symbol(c.alias).to_vec(),
                    addr: view.copy(a as u32)?.1,
                    size: *d.copy_size.get(a)?,
                    align: 1u64 << row.align.min(63),
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

    /// The key of the global entry `e`, without `redirect`.
    fn key_unredirected(&self, e: SymEntry) -> Option<SymKey> {
        let t = (*self.tables.get(e.file.0 as usize)?)?;
        let first = t.first_global().0;
        let at = e.index.0.checked_sub(first)?;
        self.link.inputs.sym_map_of(e.file).get(at as usize).map(|&id| SymKey::global(id))
    }

    /// The name of entry `e` in its file's symbol table.
    fn entry_name(&self, e: SymEntry) -> Vec<u8> {
        if let Some((t, s)) = self.sym_of(e) {
            return t.strings().get(s.st_name).ok().map(<[u8]>::to_vec).unwrap_or_default();
        }
        // A shared object's `.dynsym` entry: the reference that pulled a
        // member (`spec/dyn.md` D-12; WP-167: the map names the key, as
        // GNU ld's `libu.so (g)`).
        let Some(Reader::Shared(so)) = self.link.inputs.files.get(e.file).map(|f| &f.reader) else {
            return Vec::new();
        };
        so.symbols()
            .ok()
            .flatten()
            .and_then(|t| {
                let s = t.get(e.index).ok()?;
                t.strings().get(s.st_name).ok().map(<[u8]>::to_vec)
            })
            .unwrap_or_default()
    }

    /// The `.symtab` name of a patch (S-80) and its size, as `mortar-map 2`
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
        // A `--whole-archive` member (always live, and a member) is
        // listed with no taker and `--whole-archive` as GNU ld writes it.
        for (file, live) in l.resolution.file_live.iter() {
            let by = match *live {
                FileLive::Extracted(by) => by,
                FileLive::Always
                    if l.inputs.files.get(file).is_some_and(|r| r.member.is_some()) =>
                {
                    m.archive.push((self.file_text(file), Vec::new(), b"--whole-archive".to_vec()));
                    continue;
                }
                _ => continue,
            };
            let (taker, symbol) = if by.file == FileId::INTERNAL {
                let name = roots.iter().find(|r| r.0 == by.index.0).map(|r| r.1.clone());
                (Vec::new(), name.unwrap_or_default())
            } else {
                // The key the reference demands: an undefined mention
                // `--wrap` redirects names its target (`__real_f` pulls
                // `f`'s member, `f` pulls `__wrap_f`'s), as GNU ld writes.
                let symbol = match self.key(by.file, by.index.0).and_then(SymKey::as_global) {
                    Some(k) if Some(SymKey::global(k)) != self.key_unredirected(by) => {
                        self.link.inputs.symbols.get(k).map(|n| n.name.to_vec())
                    }
                    _ => None,
                };
                (self.file_text(by.file), symbol.unwrap_or_else(|| self.entry_name(by)))
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
            let Some(addr) = self.global_addr(id) else { continue };
            if matches!(self.kind_of(sid), Some(SectionKind::Merge | SectionKind::EhFrame)) {
                let shndx = self.chunk_of(sid).and_then(|(c, _)| self.chunk_at(c, 0));
                if let Some((shndx, _)) = shndx {
                    chunk_syms.push((shndx, addr, key.name.to_vec()));
                }
            } else {
                syms.push((sid, addr, key.name.to_vec()));
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
                ChunkKind::Merged(o) | ChunkKind::Relocs(o) => l.assignment.outsecs[o].name,
                // `-r` only (`spec/layout.md` L-R3); no final link has one.
                ChunkKind::Group(_) => b".group",
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

#[cfg(test)]
mod place_tests {
    use crate::rows::{Valued, fill_parts};

    // spec: image.md I-124; model.md §4.5 invariant (NEGATIVE: no place is
    // never a live value, so fill never writes S = 0 for it, whichever
    // P-1 rule took the place: the rule is not carried, WP-255c3)
    #[test]
    fn i124_no_place_is_not_live() {
        assert_eq!(fill_parts(Valued::NoPlace), (None, false));
        assert_eq!(fill_parts(Valued::Missing), (None, false));
    }

    // spec: image.md I-124 (an address is live; an import is live with no
    // address of its own)
    #[test]
    fn i124_values() {
        assert_eq!(fill_parts(Valued::At(0x4000)), (Some(0x4000), true));
        assert_eq!(fill_parts(Valued::At(0)), (Some(0), true));
        assert_eq!(fill_parts(Valued::Import), (None, true));
    }
}
