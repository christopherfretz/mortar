//! What layout's fixed point needs that layout does not own
//! (`mortar_layout::Sites`, `spec/arch.md` §6 step 1): the branch sites
//! of the link's code, their final targets at a round's addresses, and
//! the words of each output code section as fill would write them before
//! any erratum fix (for `Arch::erratum_sites`).
//!
//! Every relocation is read through the scan's own view
//! ([`mortar_scan::Walker`]): the keys, howtos and target kinds are the
//! scan's, and the words are `Arch::apply`'s at the round's addresses.

use std::collections::BTreeMap;
use std::sync::atomic::{AtomicBool, Ordering};

use mortar_arch::need::branch_site;
use mortar_arch::{
    Arch, FillCx, FillTables, OutputKind, ProofRun, SectionOut, SlotKind, SymNames, SymValue,
    TargetKind, TlsBlock,
};
use mortar_core::demands::DemandFlags;
use mortar_core::id::{ChunkId, FileId, OutSecId, SectionId, SymEntry, SymKey, SymKind};
use mortar_core::inputs::{Inputs, Reader};
use mortar_core::layout::{ChunkKind, MergeMap, ThunkTarget};
use mortar_core::par::Pool;
use mortar_core::resolution::{Def, Resolution, SectionKind};
use mortar_core::split::Split;
use mortar_core::{Demands, Liveness};
use mortar_core::assignment::Assignment;
use mortar_core::layout::Slots;
use mortar_diag::{Diagnostic, Key};
use mortar_elf::consts::{SHF_ALLOC, SHF_EXECINSTR, SHF_TLS};
use mortar_elf::SymbolSection;
use mortar_layout::{Headers, LayoutDiag};
use mortar_layout::fixed::{BranchAt, OwnedCode, Round, Sites};
use mortar_scan::{RecIndex, Walker};
use mortar_synth::GotPlan;
use mortar_synth::got::{GotAddrs, Region};

/// A live input section with `SHF_EXECINSTR`.
#[derive(Debug, Clone, Copy)]
struct CodeSec {
    file: FileId,
    sid: SectionId,
    index: u32,
    /// Its relocation section, 0 for none.
    rel: u32,
    out: OutSecId,
}

/// The synthetic chunks whose addresses relocations read.
#[derive(Debug, Clone, Copy, Default)]
pub struct SynthIds {
    pub got: Option<ChunkId>,
    pub got_plt: Option<ChunkId>,
    pub plt: Option<ChunkId>,
}

/// The tables the sites read.
#[derive(Clone, Copy)]
pub struct SiteInputs<'a, 's> {
    pub inputs: &'a Inputs<'s>,
    pub resolution: &'a Resolution,
    pub assignment: &'a Assignment<'a>,
    pub headers: &'a Headers<'s>,
    pub liveness: &'a Liveness,
    pub split: &'a Split,
    pub merge: &'a MergeMap,
    pub demands: &'a Demands,
    pub order: &'a RecIndex,
    pub slots: &'a Slots,
    pub got: &'a GotPlan,
    pub ids: SynthIds,
}

/// [`Sites`] of one link.
pub struct LinkSites<'a, 's> {
    t: SiteInputs<'a, 's>,
    arch: &'a dyn Arch,
    walker: &'a Walker<'a, 's>,
    code: Vec<CodeSec>,
    branches: Vec<BranchAt>,
    /// Whether `code` is asked (`Arch::errata()` is not empty).
    errata: bool,
}

impl std::fmt::Debug for LinkSites<'_, '_> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("LinkSites")
            .field("code", &self.code.len())
            .field("branches", &self.branches.len())
            .finish_non_exhaustive()
    }
}

impl<'a, 's> LinkSites<'a, 's> {
    /// The branch sites: one parallel pass over the relocations of the
    /// live code sections (`need::branch_site` on each, with the target's
    /// final demand flags).
    ///
    /// # Errors
    ///
    /// The scan's diagnostic of a section that cannot be opened (the scan
    /// reported it already; the link has failed).
    pub fn new(
        t: SiteInputs<'a, 's>,
        arch: &'a dyn Arch,
        walker: &'a Walker<'a, 's>,
        pool: &Pool,
    ) -> Result<Self, (Key, Diagnostic)> {
        let mut code = Vec::new();
        for (sid, row) in t.resolution.sections.iter() {
            if row.kind != SectionKind::Regular || !t.liveness.is_live(sid) {
                continue;
            }
            let h = &t.headers[sid];
            if h.flags & SHF_EXECINSTR == 0 || h.flags & SHF_ALLOC == 0 {
                continue;
            }
            let Some(out) = t.assignment.out(sid) else { continue };
            let Some((file, index)) = t.resolution.sh_index(sid) else { continue };
            code.push(CodeSec { file, sid, index: index.0, rel: row.reloc.0, out });
        }
        let found: Vec<Result<Vec<BranchAt>, (Key, Diagnostic)>> = pool.map(code.len(), |i| {
            let c = code[i];
            let mut out = Vec::new();
            if c.rel == 0 {
                return Ok(out);
            }
            let leads = t.order.leads(c.sid);
            walker.walk(c.file, c.sid, c.index, c.rel, 0..u64::MAX, leads, &mut |v| {
                if arch.branch(v.at.howto).is_none() {
                    return;
                }
                let kind = v.tables.sym(v.at.key).kind;
                let flags = t.demands.of(v.at.key);
                if let Some(site) =
                    branch_site(arch, OutputKind::Static, &v.at, walker.names(), kind, flags)
                {
                    out.push(BranchAt {
                        section: c.sid,
                        offset: v.at.r_offset,
                        record: v.at.index,
                        site,
                    });
                }
            })?;
            Ok(out)
        });
        let mut branches = Vec::new();
        for f in found {
            branches.extend(f?);
        }
        Ok(Self { t, arch, walker, code, branches, errata: !arch.errata().is_empty() })
    }

    /// The addresses of a round, as fill's tables.
    fn tables<'r>(&'r self, round: &'r Round<'r>) -> RoundTables<'r, 'a, 's> {
        let mut merged = BTreeMap::new();
        let mut tls: Option<(u64, u64, u64)> = None;
        for (_, c) in round.chunks.iter() {
            if let ChunkKind::Merged(out) = c.kind
                && let Some(a) = c.addr
            {
                merged.entry(out).or_insert(a);
            }
            if c.flags & SHF_TLS != 0
                && let Some(a) = c.addr
            {
                let end = a + c.size;
                let al = 1u64 << c.align;
                tls = Some(match tls {
                    None => (a, end, al),
                    Some((s, e, m)) => (s.min(a), e.max(end), m.max(al)),
                });
            }
        }
        let at = |c: Option<ChunkId>| c.and_then(|c| round.chunks.get(c)?.addr);
        let got_addrs = GotAddrs {
            got: at(self.t.ids.got).unwrap_or(0),
            got_plt: at(self.t.ids.got_plt).unwrap_or(0),
            plt: at(self.t.ids.plt).unwrap_or(0),
        };
        RoundTables {
            sites: self,
            round,
            merged,
            got_addrs,
            got_base: at(self.t.ids.got),
            tls: tls.map(|(start, end, align)| TlsBlock { start, size: end - start, align }),
            fallback: round.chunks.iter().find_map(|(_, c)| c.addr).unwrap_or(0),
            unknown: AtomicBool::new(false),
        }
    }
}

impl Sites for LinkSites<'_, '_> {
    fn branches(&self) -> &[BranchAt] {
        &self.branches
    }

    fn target(&self, i: usize, round: &Round<'_>) -> Option<u64> {
        let b = self.branches.get(i)?;
        let tables = self.tables(round);
        match b.site.key {
            ThunkTarget::Plt(key) => tables.plt(key),
            ThunkTarget::Sym { key, addend } => {
                let v = tables.value(key)?;
                Some(v.wrapping_add_signed(addend))
            }
        }
    }

    fn code(&self, out: OutSecId, round: &Round<'_>) -> Result<Option<OwnedCode>, LayoutDiag> {
        if !self.errata {
            return Ok(None);
        }
        let Some((start, end)) = round.out_extent(out) else { return Ok(None) };
        let size = usize::try_from(end - start)
            .map_err(|_| internal("an output code section larger than memory"))?;
        let mut bytes = vec![0u8; size];
        let mut proof: Vec<ProofRun> = Vec::new();
        let tables = self.tables(round);
        let cx = FillCx {
            tables: &tables,
            output: OutputKind::Static,
            got_base: tables.got_base,
            tls: tables.tls,
        };
        for c in self.code.iter().filter(|c| c.out == out) {
            // A live code section with no address at this round is not in
            // the image (folded away); it has no words here.
            let Some(addr) = round.site_addr(c.sid, 0) else { continue };
            let off = usize::try_from(addr - start)
                .map_err(|_| internal("a code section before its output section"))?;
            // Its bytes: resident, or copied (a `.text` over the staged
            // store's limits is not resident); a failed read fails the
            // link, never "no sites".
            let len = self
                .walker
                .section_bytes(c.file, c.index, |data| {
                    let dst = bytes.get_mut(off..off + data.len())?;
                    dst.copy_from_slice(data);
                    Some(data.len())
                })
                .map_err(keyed)?
                .ok_or_else(|| internal("a code section outside its output section's extent"))?;
            let len = len as u64;
            for mut run in self.walker.proof(c.file, c.index, len, true) {
                run.unit_offset = run.start;
                run.start += addr - start;
                run.end += addr - start;
                run.unit = c.sid.0;
                proof.push(run);
            }
            if c.rel == 0 {
                continue;
            }
            // The candidate words (a64.md §11.1 condition 1: `W1` at an
            // address ≡ 0xFF8 or 0xFFC mod 4096) and the three after.
            let mut windows: Vec<(u64, u64)> = Vec::new();
            let mut q = 0xFF8u64.wrapping_sub(addr) & 0xFFF;
            while q < len {
                windows.push((q, q + 4));
                q += 0x1000;
            }
            if windows.is_empty() {
                continue;
            }
            let in_window = |r: u64| {
                let i = windows.partition_point(|w| w.0 + 16 <= r);
                windows.get(i).is_some_and(|w| w.0 <= r)
            };
            let leads = self.t.order.leads(c.sid);
            let name = self.walker.section_name(c.file, c.index);
            let mut forced: Vec<u64> = Vec::new();
            let section = &mut bytes[off..off + len as usize];
            self.walker
                .walk(c.file, c.sid, c.index, c.rel, 0..u64::MAX, leads, &mut |v| {
                    if !in_window(v.at.r_offset) {
                        return;
                    }
                    let mut so = SectionOut::new(section, 0, addr, true, name.as_bytes());
                    tables.unknown.store(false, Ordering::Relaxed);
                    // A relocation that does not apply is fill's to report
                    // (the link fails there); the words are those of the
                    // relocations that applied.
                    let _ = self.arch.apply(&cx, &v.at, &mut so);
                    if tables.unknown.load(Ordering::Relaxed) {
                        forced.push(v.at.r_offset);
                    }
                })
                .map_err(keyed)?;
            // A word whose target's address is not known at this round
            // (a linker-defined symbol, a common): when it is an ADRP at a
            // `W1` position its page is unknown, so the in-place ADR fix
            // is not asked for; its immediate is set out of the fix's
            // reach, which makes the site take a patch, always correct
            // (a64.md §11.2).
            for r in forced {
                let Ok(at) = usize::try_from(r) else { continue };
                let Some(w) = section.get(at..at + 4) else { continue };
                let w = u32::from_le_bytes([w[0], w[1], w[2], w[3]]);
                if mortar_a64::erratum::is_adrp(w) {
                    // immlo (bits 29-30) and immhi (bits 5-23): 2^20 pages.
                    let far = (w & !0x60FF_FFE0) | (0x8_0000 << 5);
                    section[at..at + 4].copy_from_slice(&far.to_le_bytes());
                }
            }
        }
        proof.sort_by_key(|r| r.start);
        Ok(Some(OwnedCode { addr: start, bytes, proof }))
    }
}

/// A scan diagnostic as layout carries it: its key and place kept.
fn keyed((key, d): (Key, Diagnostic)) -> LayoutDiag {
    LayoutDiag::Diag { diagnostic: Box::new(d), key: Some(key), at: None }
}

fn internal(what: &str) -> LayoutDiag {
    LayoutDiag::from(Diagnostic::error(mortar_diag::Kind::InternalError { what: what.to_owned() }))
}

/// Fill's tables at one round's addresses.
struct RoundTables<'r, 'a, 's> {
    sites: &'r LinkSites<'a, 's>,
    round: &'r Round<'r>,
    /// The address of each output section's `Merged` chunk.
    merged: BTreeMap<OutSecId, u64>,
    got_addrs: GotAddrs,
    got_base: Option<u64>,
    tls: Option<TlsBlock>,
    /// An address in the image, for a target whose address is unknown at
    /// the round (every range check of a static image passes with it).
    fallback: u64,
    /// Set when `sym` was asked for a key whose address is unknown.
    unknown: AtomicBool,
}

impl RoundTables<'_, '_, '_> {
    fn def(&self, id: mortar_core::SymbolId) -> Option<Def> {
        let t = &self.sites.t;
        t.assignment.def(t.resolution, id).or_else(|| t.resolution.def.get(id).copied())
    }

    /// `offset` bytes into input section `sid` (model §4.5: a `Merge`
    /// section through its piece's fragment).
    fn addr_in(&self, sid: SectionId, offset: u64) -> Option<u64> {
        let t = &self.sites.t;
        if t.resolution.sections.get(sid)?.kind == SectionKind::Merge {
            let piece = t.split.merge_piece_at(sid, u32::try_from(offset).ok()?)?;
            let frag = *t.split.piece_frag.get(piece.piece as usize)?;
            let base = *self.merged.get(&t.assignment.out(sid)?)?;
            return base
                .checked_add(t.merge.offset(frag)?)?
                .checked_add(u64::from(piece.within));
        }
        self.round.site_addr(sid, offset)
    }

    /// The value of a file's entry: its section's place plus `st_value`.
    fn entry_value(&self, e: SymEntry) -> Option<u64> {
        let t = &self.sites.t;
        let Reader::Object(obj) = t.inputs.files.get(e.file)?.reader else { return None };
        let table = obj.symbols().ok()??;
        let s = table.get(e.index).ok()?;
        match table.section_of(e.index, &s).ok()? {
            SymbolSection::Section(index) => {
                self.addr_in(t.resolution.section_id(e.file, index)?, s.st_value)
            }
            SymbolSection::Absolute => Some(s.st_value),
            _ => None,
        }
    }

    /// `S` of a key at the round (the IPLT entry of an IFUNC), or `None`
    /// when layout has not given it yet (a linker-defined symbol, a
    /// common: their values are stage F's).
    fn value(&self, key: SymKey) -> Option<u64> {
        if self.sites.t.demands.of(key).contains(DemandFlags::IPLT) {
            return self.plt(key);
        }
        match key.kind() {
            SymKind::Local(e) => self.entry_value(e),
            SymKind::Global(id) => match self.def(id)? {
                Def::Defined(e) => self.entry_value(e),
                Def::Undefined { .. } => Some(0),
                Def::Common(_) | Def::Internal(_) | Def::Shared(_) => None,
            },
        }
    }

    fn slot_index(list: &[SymKey], key: SymKey) -> Option<u32> {
        list.binary_search(&key).ok().map(|i| i as u32)
    }
}

impl SymNames for RoundTables<'_, '_, '_> {
    fn name(&self, key: SymKey) -> &[u8] {
        self.sites.walker.name(key)
    }
}

impl FillTables for RoundTables<'_, '_, '_> {
    fn sym(&self, key: SymKey) -> SymValue {
        let facts = self.sites.walker.facts(key);
        let s = match facts.kind {
            TargetKind::Weak0 => Some(0),
            _ => self.value(key),
        };
        let s = s.unwrap_or_else(|| {
            self.unknown.store(true, Ordering::Relaxed);
            self.fallback
        });
        SymValue { s, kind: facts.kind, size: 0, live: true }
    }

    fn slot(&self, key: SymKey, kind: SlotKind) -> Option<u64> {
        let slots = self.sites.t.slots;
        let (region, index) = match kind {
            SlotKind::Addr => (Region::Addr, Self::slot_index(&slots.got, key)?),
            SlotKind::TpOff { .. } => (Region::TpOff, Self::slot_index(&slots.tls_ie, key)?),
            SlotKind::TlsGd => (Region::TlsGd, Self::slot_index(&slots.tls_gd, key)?),
            SlotKind::TlsDesc => (Region::TlsDesc, Self::slot_index(&slots.tls_desc, key)?),
            SlotKind::TlsLd => (Region::TlsLd, 0),
            SlotKind::PltGot => return None,
        };
        Some(self.got_addrs.got + self.sites.t.got.got_offset(region, index))
    }

    fn plt(&self, key: SymKey) -> Option<u64> {
        self.sites.t.got.iplt_address(&self.got_addrs, key)
    }

    fn flags(&self, key: SymKey) -> DemandFlags {
        self.sites.t.demands.of(key)
    }

    fn redirect(&self, section: SectionId, offset: u64) -> Option<u64> {
        self.round.redirect(section, offset).and_then(|(i, _)| self.round.patch_addr(i))
    }
}
