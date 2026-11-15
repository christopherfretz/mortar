//! What layout's fixed point needs that layout does not own
//! (`mortar_layout::Sites`, `spec/arch.md` §6 step 1): the branch sites
//! of the link's code, their final targets at a round's addresses, and
//! the words of each output code section as fill would write them before
//! any erratum fix (for `Arch::erratum_sites`).
//!
//! Every relocation is read through the scan's own view
//! ([`mortar_scan::Walker`]): the keys, howtos and target kinds are the
//! scan's, and the words are `Arch::apply`'s at the round's addresses.

#![allow(clippy::result_large_err, reason = "the error is a phase's diagnostic, made once")]

use std::collections::BTreeMap;
use std::sync::atomic::{AtomicBool, Ordering};

use mortar_arch::need::branch_site;
use mortar_arch::{
    Arch, FillCx, FillTables, OutputKind, ProofRun, SectionOut, SlotKind, SymNames, SymValue,
    TargetKind, TlsBlock,
};
use mortar_core::assignment::Assignment;
use mortar_core::demands::DemandFlags;
use mortar_core::id::{ChunkId, FileId, OutSecId, SectionId, SymEntry, SymKey, SymKind};
use mortar_core::inputs::{Inputs, Reader};
use mortar_core::layout::Slots;
use mortar_core::layout::{ChunkKind, MergeMap, ThunkTarget};
use mortar_core::par::Pool;
use mortar_core::resolution::{Def, Resolution, SectionKind};
use mortar_core::split::Split;
use mortar_core::{Demands, Liveness};
use mortar_diag::{Diagnostic, Key};
use mortar_elf::consts::{SHF_ALLOC, SHF_EXECINSTR, SHF_TLS};
use mortar_elf::{SymbolSection, SymbolTable};
use mortar_layout::fixed::{BranchAt, OwnedCode, Round, Sites};
use mortar_layout::{Headers, LayoutDiag};
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
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
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
    /// Each object's symbol table, found once (`Object::symbols` scans the
    /// section headers: per branch target and round it cost ~2 µs, 245 ms
    /// a round in a protoc link — WP-027a).
    symtabs: Vec<Option<SymbolTable<'s>>>,
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
        let files: Vec<&mortar_core::inputs::File<'s>> =
            t.inputs.files.iter().map(|(_, f)| f).collect();
        let symtabs = pool.map(files.len(), |i| match files[i].reader {
            Reader::Object(obj) => obj.symbols().ok().flatten(),
            _ => None,
        });
        Ok(Self { t, arch, walker, code, branches, symtabs, errata: !arch.errata().is_empty() })
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
        }
    }
}

impl Sites for LinkSites<'_, '_> {
    fn branches(&self) -> &[BranchAt] {
        &self.branches
    }

    fn targets(&self, round: &Round<'_>, pool: &Pool) -> Vec<Option<u64>> {
        let tables = self.tables(round);
        pool.map(self.branches.len(), |i| match self.branches[i].site.key {
            ThunkTarget::Plt(key) => tables.plt(key),
            ThunkTarget::Sym { key, addend } => {
                let v = tables.value(key)?;
                Some(v.wrapping_add_signed(addend))
            }
        })
    }

    fn code(
        &self,
        out: OutSecId,
        round: &Round<'_>,
        pool: &Pool,
    ) -> Result<Option<OwnedCode>, LayoutDiag> {
        if !self.errata {
            return Ok(None);
        }
        let Some((start, end)) = round.out_extent(out) else { return Ok(None) };
        let size = usize::try_from(end - start)
            .map_err(|_| internal("an output code section larger than memory"))?;
        let tables = self.tables(round);
        // The code sections of `out` at this round, by address. A live
        // code section with no address at this round is not in the image
        // (folded away); it has no words here.
        let mut secs: Vec<(usize, u64, CodeSec)> = Vec::new();
        for c in self.code.iter().filter(|c| c.out == out) {
            let Some(addr) = round.site_addr(c.sid, 0) else { continue };
            let off = usize::try_from(addr - start)
                .map_err(|_| internal("a code section before its output section"))?;
            secs.push((off, addr, *c));
        }
        secs.sort_by_key(|s| s.0);
        // Each section's words are a prefix of the bytes from its offset
        // to the next section's: disjoint parts, filled in parallel.
        let mut bytes = vec![0u8; size];
        let mut results: Vec<Result<Vec<ProofRun>, LayoutDiag>> =
            secs.iter().map(|_| Ok(Vec::new())).collect();
        let mut parts = Vec::with_capacity(secs.len());
        let mut rest: &mut [u8] = &mut bytes;
        let mut at = 0usize;
        for ((i, &(off, addr, c)), result) in secs.iter().enumerate().zip(results.iter_mut()) {
            let next = secs.get(i + 1).map_or(size, |n| n.0);
            if off < at || next < off || next > size {
                return Err(internal("code sections that overlap"));
            }
            let (_, tail) = std::mem::take(&mut rest).split_at_mut(off - at);
            let (mine, tail) = tail.split_at_mut(next - off);
            rest = tail;
            at = next;
            parts.push((c, addr - start, addr, mine, result));
        }
        pool.for_each_part(parts, |_, (c, rel_start, addr, mine, result)| {
            *result = self.section_code(&tables, c, addr, rel_start, mine);
        });
        let mut proof: Vec<ProofRun> = Vec::new();
        for r in results {
            proof.extend(r?);
        }
        proof.sort_by_key(|r| r.start);
        Ok(Some(OwnedCode { addr: start, bytes, proof }))
    }
}

impl LinkSites<'_, '_> {
    /// The erratum fixes of the final placement (`spec/a64.md` §11.2), as
    /// words written over fill's bytes, by address: for each site the
    /// architecture finds in the words fill writes (before any fix), a
    /// redirected final word `F` becomes `b patch` and its patch holds
    /// `W4'` (the word fill writes at `F`) and `b F + 4`; any other site's
    /// `W1` becomes the ADR. Sorted by address. Empty when no erratum is
    /// enabled.
    ///
    /// # Errors
    ///
    /// A failed read of a code section; `internal-error` for a site with
    /// neither fix (layout's fixed point gives every site one).
    pub fn erratum_fixes(
        &self,
        pl: &mortar_core::layout::Placement,
        pool: &Pool,
    ) -> Result<Vec<(u64, [u8; 4])>, LayoutDiag> {
        use mortar_core::layout::PatchKind;
        let errata = self.arch.errata();
        if errata.is_empty() {
            return Ok(Vec::new());
        }
        let round = Round {
            number: 0,
            chunks: &pl.chunks,
            place: &pl.place,
            patches: &pl.patches,
            redirects: &pl.redirects,
        };
        // The redirected final words, by address: the patch's address.
        let mut patch_of: BTreeMap<u64, (mortar_core::layout::Erratum, u64)> = BTreeMap::new();
        for r in &pl.redirects {
            let Some(p) = pl.patches.get(r.patch as usize) else { continue };
            let PatchKind::Erratum { erratum, .. } = p.kind else { continue };
            let (Some(f), Some(at)) =
                (pl.section_addr(r.section, r.offset), round.patch_addr(r.patch))
            else {
                return Err(internal("an erratum redirect with no address"));
            };
            patch_of.insert(f, (erratum, at));
        }
        let outs: std::collections::BTreeSet<OutSecId> = pl
            .chunks
            .iter()
            .filter(|(_, c)| matches!(c.kind, ChunkKind::Body(_)) && c.flags & SHF_EXECINSTR != 0)
            .filter_map(|(_, c)| c.kind.out())
            .collect();
        let mut fixes = Vec::new();
        for out in outs {
            let Some(code) = self.code(out, &round, pool)? else { continue };
            let view =
                mortar_arch::Code { bytes: &code.bytes, addr: code.addr, proof: &code.proof };
            for &e in errata {
                let mut sites = Vec::new();
                self.arch
                    .erratum_sites(e, &view, &mut sites)
                    .map_err(|_| internal("an unproved erratum site after layout"))?;
                for s in sites {
                    let (q, f) = (code.addr.wrapping_add(s.q), code.addr.wrapping_add(s.f));
                    let (Some(w1), Some(w4)) = (view.word(s.q), view.word(s.f)) else {
                        return Err(internal("an erratum site outside its code"));
                    };
                    if let Some(&(pe, patch)) = patch_of.get(&f) {
                        let b = mortar_a64::erratum_branch(f, patch)
                            .map_err(|_| internal("an erratum patch out of reach"))?;
                        fixes.push((f, b.to_le_bytes()));
                        let cx = mortar_arch::PatchCx {
                            addr: patch,
                            target: f,
                            site: q,
                            word: w4,
                            bti: false,
                            target_word: None,
                            target_abs: false,
                        };
                        let size = self.arch.patch(pe).map_or(0, |p| p.size as usize);
                        let mut words = vec![0u8; size];
                        self.arch
                            .write_patch(pe, &cx, &mut words)
                            .map_err(|_| internal("an erratum patch that cannot be written"))?;
                        for (i, w) in words.chunks_exact(4).enumerate() {
                            fixes.push((patch + 4 * i as u64, [w[0], w[1], w[2], w[3]]));
                        }
                    } else {
                        let adr = mortar_a64::erratum_adr(q, w1)
                            .ok_or_else(|| internal("an erratum site with neither fix"))?;
                        fixes.push((q, adr.to_le_bytes()));
                    }
                }
            }
        }
        fixes.sort_by_key(|f| f.0);
        Ok(fixes)
    }

    /// One code section's words at `addr` (`rel_start` bytes into its
    /// output section) into `room`, and its proof runs.
    fn section_code(
        &self,
        tables: &RoundTables<'_, '_, '_>,
        c: CodeSec,
        addr: u64,
        rel_start: u64,
        room: &mut [u8],
    ) -> Result<Vec<ProofRun>, LayoutDiag> {
        // Its bytes: resident, or copied (a `.text` over the staged
        // store's limits is not resident); a failed read fails the link,
        // never "no sites".
        let len = self
            .walker
            .section_bytes(c.file, c.index, |data| {
                let dst = room.get_mut(..data.len())?;
                dst.copy_from_slice(data);
                Some(data.len())
            })
            .map_err(keyed)?
            .ok_or_else(|| internal("a code section outside its output section's extent"))?;
        let section = &mut room[..len];
        let len = len as u64;
        let mut proof = Vec::new();
        for mut run in self.walker.proof(c.file, c.index, len, true) {
            run.unit_offset = run.start;
            run.start += rel_start;
            run.end += rel_start;
            run.unit = c.sid.0;
            proof.push(run);
        }
        if c.rel == 0 {
            return Ok(proof);
        }
        // The words the erratum scan reads (a64.md §11.1 condition 1: `W1`
        // at an address ≡ 0xFF8 or 0xFFC mod 4096, and the three words
        // after each): `[0xFF8, 0x100C)` of every page, as offsets into
        // this section, which may hold only the end of a window (its `W1`
        // in the section before). A record applies to `[r, r + 8)` at most;
        // one that touches a window is applied (applying more is fill's
        // own words, never wrong).
        let windows = erratum_windows(addr, len);
        if windows.is_empty() {
            return Ok(proof);
        }
        let in_window = |r: u64| {
            let i = windows.partition_point(|w| w.1 <= r);
            windows.get(i).is_some_and(|w| w.0 < r.saturating_add(8))
        };
        let leads = self.t.order.leads(c.sid);
        let name = self.walker.section_name(c.file, c.index);
        // This section's own view of the round's tables: whether the
        // relocation just applied asked for an address not known yet.
        let probe = Probe { tables, unknown: AtomicBool::new(false) };
        let cx = FillCx {
            tables: &probe,
            output: OutputKind::Static,
            got_base: tables.got_base,
            tls: tables.tls,
        };
        let mut forced: Vec<u64> = Vec::new();
        self.walker
            .walk(c.file, c.sid, c.index, c.rel, 0..u64::MAX, leads, &mut |v| {
                if !in_window(v.at.r_offset) {
                    return;
                }
                let mut so =
                    SectionOut::new(section, 0, addr, true, name.as_bytes()).with_exec(true);
                probe.unknown.store(false, Ordering::Relaxed);
                // A relocation that does not apply is fill's to report (the
                // link fails there); the words are those of the
                // relocations that applied.
                let _ = self.arch.apply(&cx, &v.at, &mut so);
                if probe.unknown.load(Ordering::Relaxed) {
                    forced.push(v.at.r_offset);
                }
            })
            .map_err(keyed)?;
        // A word whose target's address is not known at this round (a
        // linker-defined symbol, a common): when it is an ADRP at a `W1`
        // position its page is unknown, so the in-place ADR fix is not
        // asked for; its immediate is set out of the fix's reach, which
        // makes the site take a patch, always correct (a64.md §11.2).
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
        Ok(proof)
    }
}

/// The erratum scan's windows in a section of `len` bytes at `addr`: the
/// offset ranges, ascending and disjoint, of the bytes `[P + 0xFF8, P +
/// 0x100C)` of each page `P` (`mortar_a64::erratum::sites` reads the words
/// at `0xFF8` and `0xFFC` and the three after each) that lie in the
/// section.
fn erratum_windows(addr: u64, len: u64) -> Vec<(u64, u64)> {
    const LO: u128 = 0xFF8;
    const HI: u128 = 0x100C;
    let (start, end) = (u128::from(addr), u128::from(addr) + u128::from(len));
    let mut windows = Vec::new();
    // The page whose window may reach `start`: the one before `start`'s.
    let mut page = (start & !0xFFF).saturating_sub(0x1000);
    while page + LO < end {
        let (lo, hi) = ((page + LO).max(start), (page + HI).min(end));
        if lo < hi {
            // In u64: both lie in `start..=end`, `end - start = len`.
            windows.push(((lo - start) as u64, (hi - start) as u64));
        }
        page += 0x1000;
    }
    windows
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
            return base.checked_add(t.merge.offset(frag)?)?.checked_add(u64::from(piece.within));
        }
        self.round.site_addr(sid, offset)
    }

    /// The value of a file's entry: its section's place plus `st_value`.
    fn entry_value(&self, e: SymEntry) -> Option<u64> {
        let t = &self.sites.t;
        let table = self.sites.symtabs.get(e.file.0 as usize)?.as_ref()?;
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

/// One section's use of a round's [`RoundTables`] as fill's tables: the
/// flag of an address not known at the round is the section's own (the
/// sections of a round are filled in parallel).
struct Probe<'p, 'r, 'a, 's> {
    tables: &'p RoundTables<'r, 'a, 's>,
    /// Set when `sym` was asked for a key whose address is unknown.
    unknown: AtomicBool,
}

impl SymNames for Probe<'_, '_, '_, '_> {
    fn name(&self, key: SymKey) -> &[u8] {
        self.tables.sites.walker.name(key)
    }
}

impl FillTables for Probe<'_, '_, '_, '_> {
    fn sym(&self, key: SymKey) -> SymValue {
        let t = self.tables;
        let facts = t.sites.walker.facts(key);
        let s = match facts.kind {
            TargetKind::Weak0 => Some(0),
            _ => t.value(key),
        };
        let s = s.unwrap_or_else(|| {
            self.unknown.store(true, Ordering::Relaxed);
            t.fallback
        });
        SymValue { s, kind: facts.kind, size: 0, live: true, tls: facts.tls }
    }

    fn slot(&self, key: SymKey, kind: SlotKind) -> Option<u64> {
        self.tables.slot(key, kind)
    }

    fn plt(&self, key: SymKey) -> Option<u64> {
        self.tables.plt(key)
    }

    fn flags(&self, key: SymKey) -> DemandFlags {
        self.tables.sites.t.demands.of(key)
    }

    fn redirect(&self, section: SectionId, offset: u64) -> Option<u64> {
        let round = self.tables.round;
        round.redirect(section, offset).and_then(|(i, _)| round.patch_addr(i))
    }
}

impl RoundTables<'_, '_, '_> {
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
}

#[cfg(test)]
mod tests {
    use super::erratum_windows;

    // spec: a64.md §11.1 (the words of a site: `W1` at 0xFF8 or 0xFFC, and
    // up to three words after it, which may cross the page and the section)
    #[test]
    fn windows_cover_every_word_the_scan_reads() {
        // A section from a page start: the tail of the window before it
        // (the last words of a site whose `W1` is in the previous page),
        // then the window at each 0xFF8.
        assert_eq!(
            erratum_windows(0x40_0000, 0x2000),
            [(0, 0xC), (0xFF8, 0x100C), (0x1FF8, 0x2000)]
        );
        // A section that starts inside a window: its first bytes.
        assert_eq!(erratum_windows(0x40_1000, 0x40), [(0, 0xC)]);
        assert_eq!(erratum_windows(0x40_0FFC, 0x40), [(0, 0x10)]);
        // One that ends before any window, and one that touches none.
        assert_eq!(erratum_windows(0x40_0010, 0x40), []);
        assert_eq!(erratum_windows(0x40_100C, 0x40), []);
        // Low addresses do not underflow.
        assert_eq!(erratum_windows(0, 0x1000), [(0xFF8, 0x1000)]);
        assert_eq!(erratum_windows(0x10, 8), []);
    }
}
