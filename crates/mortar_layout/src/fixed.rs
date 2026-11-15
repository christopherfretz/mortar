//! Stage E (`spec/layout.md` §14, `spec/arch.md` §6): the fixed point of
//! range extension and erratum patches.
//!
//! The judgement is `mortar_arch`'s (`need::{thunk_need, erratum_need,
//! reaches}`, `Arch::{thunk, erratum_sites, patch_reach}`); what layout
//! does not own (the branch sites, which the relocation scan finds with
//! `need::branch_site`, their final targets, and the code words `fill`
//! writes) comes through [`Sites`].
//!
//! The state is the patch set: per island (L-111) a set of patch keys,
//! and the redirects `(SectionId, offset) → (island, key)`. Each round
//! lays out the islands by key (L-115), runs stage D, collects the needs
//! at those addresses, and adds patches. The addresses are a function of
//! the patch set alone (L-21), so the result does not depend on the round
//! that added a patch.

use std::collections::{BTreeMap, BTreeSet};

pub use mortar_arch::need::R_MAX;
use mortar_arch::need::{
    BranchSite, ErratumNeed, FIXED_POINT, ThunkNeed, erratum_need, reaches, thunk_need,
};
use mortar_arch::{Arch, Code, ErratumSite, ProofRun, Reach};
use mortar_core::column::Column;
use mortar_core::id::{ChunkId, OutSecId, SectionId, SymKey};
use mortar_core::layout::{
    Chunk, ChunkKind, Erratum, Part, Patch, PatchKind, Place, Redirect, SegmentRules, Sized,
    ThunkTarget,
};
use mortar_core::par::Pool;

use crate::address::{Addressed, address};
use crate::chunk::Staged;
use crate::diag::{LayoutDiag, internal, refused};
use crate::target::{Options, Target};

/// arch.md §6 item 6 and L-112: `need::FIXED_POINT`.
fn cap_refusal() -> LayoutDiag {
    refused(FIXED_POINT.class, FIXED_POINT.name.to_string())
}

/// a64.md §11.1 condition 5: `mortar_arch::diag::unproved`, with the
/// candidate's input section and the offset of `W1` in it (at `q`, an
/// address) as a note. `LayoutDiag` carries no file place or key; the
/// key is `diag::unproved_key` of that section's chunk.
fn unproved_refusal(e: Erratum, at: Option<(ChunkId, SectionId, u64)>) -> LayoutDiag {
    let d = LayoutDiag::from(mortar_arch::diag::unproved(e));
    match at {
        // `mortar_arch::diag::unproved_key`: `(90, 0, ChunkId)`; the caller
        // names the input section and the offset of `W1` as the place.
        Some((chunk, section, offset)) => d.on_chunk(chunk, Some((section, offset))),
        None => d,
    }
}

/// A branch site of the link: where it is, and what `need::branch_site`
/// made of its relocation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BranchAt {
    /// The section and offset of the branch (the redirect key).
    pub section: SectionId,
    pub offset: u64,
    /// The relocation's record index in its section (the arch.md §6
    /// step 3 tie-break).
    pub record: u32,
    pub site: BranchSite,
}

/// The words of one output code section at a round's addresses
/// (`mortar_arch::Code`, owned).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OwnedCode {
    /// The address of byte 0: the output section's first chunk.
    pub addr: u64,
    /// The bytes `fill` writes at the round's addresses before any
    /// erratum fix, padding and islands included.
    pub bytes: Vec<u8>,
    pub proof: Vec<ProofRun>,
}

/// What the fixed point needs that layout does not own (arch.md §6
/// step 1): the relocation scan's branch sites and fill's words.
pub trait Sites: Sync {
    /// Every branch site, in any order: the relocations of the link's
    /// code sections for which `need::branch_site` is `Some` (an
    /// input-only decision).
    fn branches(&self) -> &[BranchAt];

    /// The final target `T` of every branch site at the round's
    /// addresses (`[i]` is `branches()[i]`'s): the symbol, or its PLT or
    /// IPLT entry, plus the addend. One call per round, so that what a
    /// round's addresses give (fill's tables) is built once (WP-027a: per
    /// site, it was 250 ms per round of a protoc link).
    fn targets(&self, round: &Round<'_>, pool: &Pool) -> Vec<Option<u64>>;

    /// The words of output code section `out` at the round's addresses,
    /// for `Arch::erratum_sites`. Asked only when `Arch::errata()` is not
    /// empty. `Ok(None)`: `out` has no words at this round (no extent).
    ///
    /// # Errors
    ///
    /// A read of an input section's bytes that failed: the link fails
    /// with it (never "no erratum sites", `spec/landed/input/WP-021b.md`).
    fn code(
        &self,
        out: OutSecId,
        round: &Round<'_>,
        pool: &Pool,
    ) -> Result<Option<OwnedCode>, LayoutDiag>;
}

/// No branch sites and no code: nothing is ever needed.
#[derive(Debug, Clone, Copy, Default)]
pub struct NoSites;

impl Sites for NoSites {
    fn branches(&self) -> &[BranchAt] {
        &[]
    }
    fn targets(&self, _round: &Round<'_>, _pool: &Pool) -> Vec<Option<u64>> {
        Vec::new()
    }
    fn code(
        &self,
        _out: OutSecId,
        _round: &Round<'_>,
        _pool: &Pool,
    ) -> Result<Option<OwnedCode>, LayoutDiag> {
        Ok(None)
    }
}

/// The range extension of a link: the architecture and the sites.
#[derive(Clone, Copy)]
pub struct RangeExtension<'a> {
    pub arch: &'a dyn Arch,
    pub sites: &'a dyn Sites,
}

impl std::fmt::Debug for RangeExtension<'_> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("RangeExtension")
            .field("errata", &self.arch.errata())
            .field("branches", &self.sites.branches().len())
            .finish()
    }
}

/// What a need asks for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum NeedKind {
    /// A branch of this reach that must go through a thunk of this key.
    Thunk(ThunkTarget, Reach),
    /// An erratum site that needs a patch (its key is the need's site).
    Erratum(Erratum),
}

/// One need of arch.md §6 step 1, judged at the round's addresses.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Need {
    /// The site's address at `A_k` (the branch, or the final word `F`).
    addr: u64,
    /// The redirect key: the branch, or `F` (a64.md §11.1).
    section: SectionId,
    offset: u64,
    /// The relocation record index (0 for an erratum site).
    record: u32,
    kind: NeedKind,
}

impl Need {
    /// arch.md §6 step 3: output-address order, ties by (`FileId`, section
    /// index, record index); `SectionId` order is (`FileId`, section index)
    /// order. The offset and key make the order total.
    fn sort_key(&self) -> (u64, SectionId, u32, u64, Key) {
        (self.addr, self.section, self.record, self.offset, Key::of_need(self))
    }
}

/// A patch's key: its order inside an island (L-115). The derived order
/// is the spec's: thunks before erratum patches, `Sym` before `Plt`, then
/// (`SymKey`, addend); erratum patches by (`SectionId`, offset).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
enum Key {
    Sym(SymKey, i64),
    Plt(SymKey),
    Erratum(SectionId, u64, Erratum),
}

impl Key {
    fn of_need(need: &Need) -> Self {
        match need.kind {
            NeedKind::Thunk(ThunkTarget::Sym { key, addend }, _) => Self::Sym(key, addend),
            NeedKind::Thunk(ThunkTarget::Plt(key), _) => Self::Plt(key),
            NeedKind::Erratum(e) => Self::Erratum(need.section, need.offset, e),
        }
    }

    fn kind(self) -> PatchKind {
        match self {
            Self::Sym(key, addend) => PatchKind::Thunk(ThunkTarget::Sym { key, addend }),
            Self::Plt(key) => PatchKind::Thunk(ThunkTarget::Plt(key)),
            Self::Erratum(section, offset, erratum) => {
                PatchKind::Erratum { erratum, section, offset }
            }
        }
    }

    fn is_thunk(self) -> bool {
        !matches!(self, Self::Erratum(..))
    }
}

/// One round's addresses and patch set, as [`Sites`] sees them.
#[derive(Debug, Clone, Copy)]
pub struct Round<'a> {
    /// 1 for the first round.
    pub number: u32,
    pub chunks: &'a Column<ChunkId, Chunk>,
    pub place: &'a Column<SectionId, Place>,
    /// Sorted by (chunk, offset).
    pub patches: &'a [Patch],
    /// Sorted by (section, offset).
    pub redirects: &'a [Redirect],
}

impl Round<'_> {
    /// The address of `offset` bytes into a placed section.
    #[must_use]
    pub fn site_addr(&self, section: SectionId, offset: u64) -> Option<u64> {
        let (chunk, at) = self.place.get(section)?.get()?;
        self.chunks.get(chunk)?.addr?.checked_add(at)?.checked_add(offset)
    }

    /// The address of patch `i`.
    #[must_use]
    pub fn patch_addr(&self, i: u32) -> Option<u64> {
        let p = self.patches.get(i as usize)?;
        self.chunks.get(p.chunk)?.addr?.checked_add(p.offset)
    }

    /// The patch a site is redirected to, and its position.
    #[must_use]
    pub fn redirect(&self, section: SectionId, offset: u64) -> Option<(u32, &Patch)> {
        let at = self
            .redirects
            .binary_search_by_key(&(section, offset), |r| (r.section, r.offset))
            .ok()?;
        let i = self.redirects[at].patch;
        Some((i, self.patches.get(i as usize)?))
    }

    /// The addresses of output section `out`'s chunks: from the first
    /// one's start to the last one's end (what [`Sites::code`] covers).
    #[must_use]
    pub fn out_extent(&self, out: OutSecId) -> Option<(u64, u64)> {
        let mut extent: Option<(u64, u64)> = None;
        for (_, c) in self.chunks.iter() {
            if c.kind.part().map(|p| p.out) != Some(out) {
                continue;
            }
            let Some(a) = c.addr else { continue };
            let end = a.checked_add(c.size)?;
            extent = Some(extent.map_or((a, end), |(s, e)| (s.min(a), e.max(end))));
        }
        extent
    }

    /// The address of the patch a site is redirected to.
    fn redirect_addr(&self, section: SectionId, offset: u64) -> Option<u64> {
        self.redirect(section, offset).and_then(|(i, _)| self.patch_addr(i))
    }
}

/// The result of stage E.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Fixed {
    /// Stage D at the fixed point.
    pub addressed: Addressed,
    pub patches: Box<[Patch]>,
    pub redirects: Box<[Redirect]>,
    /// The islands and the address-dependent synthetic chunks.
    pub sized: Box<[Sized]>,
    /// The rounds run, the last (with no need) included.
    pub rounds: u32,
}

/// One island's patches laid out by key (L-115).
struct Laid {
    /// (key, offset), in key order.
    rows: Vec<(Key, u64)>,
    size: u64,
}

/// The size and log2 alignment of a patch.
fn patch_shape(arch: &dyn Arch, k: Key) -> Result<(u64, u8), LayoutDiag> {
    match k {
        Key::Sym(..) | Key::Plt(..) => {
            let t = arch
                .thunk()
                .ok_or_else(|| internal("a thunk need on an architecture with no thunk"))?;
            let align = u8::try_from(t.align.max(1).trailing_zeros()).unwrap_or(0);
            Ok((u64::from(t.size), align))
        }
        Key::Erratum(_, _, e) => {
            let p = arch.patch(e).ok_or_else(|| internal("an erratum with no patch shape"))?;
            let align = u8::try_from(p.align.max(1).trailing_zeros()).unwrap_or(0);
            Ok((u64::from(p.size), align))
        }
    }
}

fn lay_out(keys: &BTreeSet<Key>, arch: &dyn Arch) -> Result<Laid, LayoutDiag> {
    let mut rows = Vec::with_capacity(keys.len());
    let mut end = 0u64;
    for &k in keys {
        let (size, align) = patch_shape(arch, k)?;
        let a = 1u64 << align.min(63);
        let at = end.div_ceil(a).saturating_mul(a);
        rows.push((k, at));
        end = at.saturating_add(size);
    }
    Ok(Laid { rows, size: end })
}

/// The redirect keys `(SectionId, offset)` of a round's erratum sites.
type SiteKeys = BTreeSet<(SectionId, u64)>;

/// The placed sections of each body chunk of a code output section, by
/// offset: the map from an erratum site's final word back to its key.
type Bodies = BTreeMap<ChunkId, Vec<(u64, SectionId)>>;

fn bodies(staged: &Staged, by_part: &BTreeMap<Part, usize>) -> Bodies {
    let mut out: Bodies = BTreeMap::new();
    for (section, place) in staged.place.iter() {
        let Some((chunk, at)) = place.get() else { continue };
        if let Some(ChunkKind::Body(part)) = staged.chunks.get(chunk).map(|c| c.kind)
            && by_part.contains_key(&part)
        {
            out.entry(chunk).or_default().push((at, section));
        }
    }
    for v in out.values_mut() {
        v.sort_unstable();
    }
    out
}

/// The section and offset of the word at `addr`, if it is in a body chunk.
fn key_at(bodies: &Bodies, chunks: &Column<ChunkId, Chunk>, addr: u64) -> Option<(SectionId, u64)> {
    chunk_at(bodies, chunks, addr).map(|(_, section, offset)| (section, offset))
}

/// The body chunk, section and offset of the word at `addr`.
fn chunk_at(
    bodies: &Bodies,
    chunks: &Column<ChunkId, Chunk>,
    addr: u64,
) -> Option<(ChunkId, SectionId, u64)> {
    for (&chunk, secs) in bodies {
        let c = chunks.get(chunk)?;
        let start = c.addr?;
        if addr < start || addr - start >= c.size {
            continue;
        }
        let off = addr - start;
        let i = secs.partition_point(|&(at, _)| at <= off).checked_sub(1)?;
        let (at, section) = secs[i];
        return Some((chunk, section, off - at));
    }
    None
}

/// Step 1: the needs at the round's addresses, and the erratum site keys
/// (for the stale-redirect rule, a64.md §11.2).
fn needs(
    range: &RangeExtension<'_>,
    round: &Round<'_>,
    code_outs: &[OutSecId],
    bodies: &Bodies,
    pool: &Pool,
    steps: &mut mortar_core::wp032b_steps::Steps,
) -> Result<(Vec<Need>, SiteKeys), LayoutDiag> {
    let branches = range.sites.branches();
    let targets = range.sites.targets(round, pool);
    steps.step("targets", branches.len() as u64, "branch");
    if targets.len() != branches.len() {
        return Err(internal("not one branch target per branch site"));
    }
    let judged: Vec<Result<Option<Need>, LayoutDiag>> = pool.map(branches.len(), |i| {
        let b = branches[i];
        // A site in a section that is not placed is no site.
        let Some(p) = round.site_addr(b.section, b.offset) else { return Ok(None) };
        let t = targets[i].ok_or_else(|| internal("a branch site with no target address"))?;
        let r = round.redirect_addr(b.section, b.offset);
        Ok((thunk_need(&b.site, p, t, r) != ThunkNeed::None).then_some(Need {
            addr: p,
            section: b.section,
            offset: b.offset,
            record: b.record,
            kind: NeedKind::Thunk(b.site.key, b.site.branch.reach),
        }))
    });
    let mut out = Vec::new();
    for j in judged {
        out.extend(j?);
    }
    steps.step("branch needs", branches.len() as u64, "branch");
    let mut keys = BTreeSet::new();
    let errata = range.arch.errata();
    if errata.is_empty() {
        return Ok((out, keys));
    }
    // Each site, and whether it is a need.
    let scanned: Vec<Result<Vec<(Need, bool)>, LayoutDiag>> =
        pool.map(code_outs.len() * errata.len(), |u| {
            let out_sec = code_outs[u / errata.len()];
            let e = errata[u % errata.len()];
            let Some(code) = range.sites.code(out_sec, round, pool)? else { return Ok(Vec::new()) };
            let view = Code { bytes: &code.bytes, addr: code.addr, proof: &code.proof };
            let mut sites: Vec<ErratumSite> = Vec::new();
            range.arch.erratum_sites(e, &view, &mut sites).map_err(|u| {
                unproved_refusal(e, chunk_at(bodies, round.chunks, code.addr.wrapping_add(u.q)))
            })?;
            let mut found = Vec::with_capacity(sites.len());
            for s in sites {
                // `ErratumSite` holds code offsets; the need functions take
                // addresses.
                let w1 =
                    view.word(s.q).ok_or_else(|| internal("an erratum site outside its code"))?;
                let (q, f) = (code.addr.wrapping_add(s.q), code.addr.wrapping_add(s.f));
                // Proved unreachable (arch.md §3.7, a64.md §11.1 "Sites and
                // synthetic chunks"): no site has a word in a synthetic
                // chunk, so `F` is in an input section.
                let (section, offset) = key_at(bodies, round.chunks, f).ok_or_else(|| {
                    internal("an erratum site whose final word is in no input section")
                })?;
                let r = round.redirect_addr(section, offset);
                let needed = erratum_need(range.arch, e, q, w1, f, r) != ErratumNeed::None;
                let need = Need { addr: f, section, offset, record: 0, kind: NeedKind::Erratum(e) };
                found.push((need, needed));
            }
            Ok(found)
        });
    for s in scanned {
        for (n, needed) in s? {
            keys.insert((n.section, n.offset));
            if needed {
                out.push(n);
            }
        }
    }
    steps.step("erratum needs", keys.len() as u64, "site");
    Ok((out, keys))
}

/// Stage E for a stage C that differs from `prev`'s only in the sizes of
/// non-alloc chunks after every alloc chunk (the symbol tables and
/// `.shstrtab`, sized after the fixed point: `spec/synth.md` R-L5, S-90):
/// `prev`'s fixed point, readdressed. `None` when `staged` differs in any
/// other way, or stage D fails on it (the caller then runs
/// [`fixed_point`], which reports that failure as its own).
///
/// It is the result [`fixed_point`] would give: stage D gives every alloc
/// chunk an address and offset that do not depend on a non-alloc chunk
/// after the last alloc one (L-72: those only move the file cursor past
/// it), so every round sees the addresses `prev`'s round saw, and the sites
/// read only alloc addresses ([`Sites`]). That is checked on the result:
/// every alloc chunk must keep `prev`'s address and offset.
#[must_use]
pub fn refix(
    staged: &Staged,
    prev: &Staged,
    prev_fixed: &mortar_core::layout::Placement,
    rounds: u32,
    target: &Target,
    options: &Options,
    rules: &SegmentRules,
) -> Option<Fixed> {
    if staged.chunks.len() != prev.chunks.len()
        || staged.order != prev.order
        || staged.islands != prev.islands
        || staged.relro_padding != prev.relro_padding
    {
        return None;
    }
    // The first chunk in file order whose size changed; no alloc chunk may
    // follow it, and nothing but sizes may differ.
    let mut changed = false;
    for &c in staged.order.iter() {
        let (a, b) = (&staged.chunks[c], &prev.chunks[c]);
        let resized = a.size != b.size || a.capacity != b.capacity;
        changed |= resized;
        let others_equal = Chunk { size: b.size, capacity: b.capacity, ..*a } == *b;
        if (changed && a.is_alloc()) || !others_equal {
            return None;
        }
    }
    if staged.ranks != prev.ranks
        || staged.out_of != prev.out_of
        || staged.sym_place != prev.sym_place
        || staged.place != prev.place
    {
        return None;
    }
    let island_sizes: Vec<u64> =
        staged.islands.iter().map(|&i| prev_fixed.chunks.get(i).map_or(0, |c| c.size)).collect();
    let addressed = address(staged, &island_sizes, target, options, rules).ok()?;
    let same = staged.order.iter().all(|&c| {
        let (a, b) = (&addressed.chunks[c], prev_fixed.chunks.get(c));
        !a.is_alloc()
            || b.is_some_and(|b| a.addr == b.addr && a.offset == b.offset && a.size == b.size)
    });
    if !same || addressed.chunks.len() != prev_fixed.chunks.len() {
        return None;
    }
    let mut sized: Vec<Sized> = staged
        .islands
        .iter()
        .zip(&island_sizes)
        .map(|(&chunk, &size)| Sized { chunk, size })
        .collect();
    for (chunk, c) in addressed.chunks.iter() {
        if matches!(c.kind, ChunkKind::Synthetic(s) if s.address_dependent) {
            sized.push(Sized { chunk, size: c.size });
        }
    }
    sized.sort_unstable_by_key(|s| s.chunk);
    Some(Fixed {
        addressed,
        patches: prev_fixed.patches.clone(),
        redirects: prev_fixed.redirects.clone(),
        sized: sized.into(),
        rounds,
    })
}

/// Stage E: rounds of arch.md §6 until one has no needs. `None`: no
/// range extension (an architecture with no veneers, or a caller with no
/// relocation scan); one round.
///
/// # Errors
///
/// `refused` (`feature`, `range-extension fixed point`): more than
/// [`R_MAX`] rounds, an island over its cap (L-112), a need with no island
/// in reach (a site outside every exec part, or a target with no
/// islands); `refused` (`feature`, `erratum 843419 scan without mapping
/// symbols`); stage D's errors.
#[allow(clippy::too_many_lines)]
pub fn fixed_point(
    staged: &Staged,
    target: &Target,
    options: &Options,
    rules: &SegmentRules,
    range: Option<&RangeExtension<'_>>,
    pool: &Pool,
) -> Result<Fixed, LayoutDiag> {
    let n = staged.islands.len();
    let cap = target.islands.map_or(0, |r| r.cap);
    // The island of each body chunk (L-111).
    let by_part: BTreeMap<Part, usize> = staged
        .islands
        .iter()
        .enumerate()
        .filter_map(|(i, &c)| staged.chunks[c].kind.part().map(|p| (p, i)))
        .collect();
    let code_outs: Vec<OutSecId> =
        by_part.keys().map(|p| p.out).collect::<BTreeSet<_>>().into_iter().collect();
    let bodies = bodies(staged, &by_part);
    let island_of_section = |s: SectionId| -> Option<usize> {
        let (chunk, _) = staged.place.get(s)?.get()?;
        match staged.chunks.get(chunk)?.kind {
            ChunkKind::Body(part) => by_part.get(&part).copied(),
            _ => None,
        }
    };
    let mut sets: Vec<BTreeSet<Key>> = vec![BTreeSet::new(); n];
    let mut redirect: BTreeMap<(SectionId, u64), (usize, Key)> = BTreeMap::new();
    let mut number = 0u32;
    let mut steps = mortar_core::wp032b_steps::Steps::new("fixed");
    steps.step("prepare", 0, "");
    loop {
        number += 1;
        let laid: Vec<Laid> = match range {
            Some(r) => sets.iter().map(|s| lay_out(s, r.arch)).collect::<Result<_, _>>()?,
            None => sets.iter().map(|_| Laid { rows: Vec::new(), size: 0 }).collect(),
        };
        if laid.iter().any(|l| l.size > cap) {
            return Err(cap_refusal());
        }
        let island_sizes: Vec<u64> = laid.iter().map(|l| l.size).collect();
        steps.step("lay out", 0, "");
        let addressed = address(staged, &island_sizes, target, options, rules)?;
        steps.step("address", addressed.chunks.len() as u64, "chunk");
        // The patch table, sorted by (chunk, offset): islands are in
        // ChunkId order and each island's rows in offset order.
        let mut patches = Vec::new();
        let mut first = Vec::with_capacity(n);
        for (i, l) in laid.iter().enumerate() {
            first.push(patches.len());
            for &(k, offset) in &l.rows {
                patches.push(Patch { chunk: staged.islands[i], offset, kind: k.kind() });
            }
        }
        let index_of = |island: usize, key: Key| -> Option<u32> {
            let at = laid[island].rows.binary_search_by_key(&key, |r| r.0).ok()?;
            u32::try_from(first[island] + at).ok()
        };
        let mut redirects = Vec::with_capacity(redirect.len());
        for (&(section, offset), &(island, key)) in &redirect {
            let patch = index_of(island, key).ok_or_else(|| internal("a redirect to no patch"))?;
            redirects.push(Redirect { section, offset, patch });
        }
        let round = Round {
            number,
            chunks: &addressed.chunks,
            place: &staged.place,
            patches: &patches,
            redirects: &redirects,
        };
        steps.step("tables", patches.len() as u64, "patch");
        let (mut needs, erratum_keys) = match range {
            Some(r) => needs(r, &round, &code_outs, &bodies, pool, &mut steps)?,
            None => (Vec::new(), BTreeSet::new()),
        };
        if needs.is_empty() {
            // a64.md §11.2 "Stale redirects": an erratum redirect whose key
            // is not a site at the final addresses is dropped; its patch
            // stays (L-116), so no address moves.
            redirects.retain(|r| {
                let key = (r.section, r.offset);
                let is_erratum = matches!(
                    patches.get(r.patch as usize).map(|p| p.kind),
                    Some(PatchKind::Erratum { .. })
                );
                !is_erratum || erratum_keys.contains(&key)
            });
            let mut sized: Vec<Sized> = staged
                .islands
                .iter()
                .zip(&island_sizes)
                .map(|(&chunk, &size)| Sized { chunk, size })
                .collect();
            for (chunk, c) in addressed.chunks.iter() {
                if matches!(c.kind, ChunkKind::Synthetic(s) if s.address_dependent) {
                    sized.push(Sized { chunk, size: c.size });
                }
            }
            sized.sort_unstable_by_key(|s| s.chunk);
            return Ok(Fixed {
                addressed,
                patches: patches.into(),
                redirects: redirects.into(),
                sized: sized.into(),
                rounds: number,
            });
        }
        if number >= R_MAX {
            return Err(cap_refusal());
        }
        needs.sort_unstable_by_key(Need::sort_key);
        // The thunks of each key at A_k, by address (arch.md §6 step 3).
        let mut thunks: BTreeMap<Key, Vec<(u64, usize)>> = BTreeMap::new();
        for (i, l) in laid.iter().enumerate() {
            let base = addressed.chunks[staged.islands[i]].addr.unwrap_or(0);
            for &(k, offset) in &l.rows {
                if k.is_thunk() {
                    thunks.entry(k).or_default().push((base + offset, i));
                }
            }
        }
        for v in thunks.values_mut() {
            v.sort_unstable();
        }
        for need in &needs {
            let key = Key::of_need(need);
            let site = (need.section, need.offset);
            // The thunk of its key that existed at A_k, in reach of P, with
            // the lowest address.
            if let NeedKind::Thunk(_, reach) = need.kind
                && let Some(&(_, island)) = thunks
                    .get(&key)
                    .and_then(|v| v.iter().find(|&&(a, _)| reaches(reach, need.addr, a)))
            {
                redirect.insert(site, (island, key));
                continue;
            }
            // Else the patch of its key in the island of the site's part
            // (L-111): the one already there if the island has one
            // (condition (C): one per key per island), else a new one. An
            // old one out of reach stays a need, which the cap bounds.
            let island = island_of_section(need.section).ok_or_else(cap_refusal)?;
            sets[island].insert(key);
            redirect.insert(site, (island, key));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // spec: layout.md §14.2 L-115 (thunks Sym before Plt by key and addend, then errata by site)
    #[test]
    fn l115_key_order() {
        let k = |i| SymKey::global(mortar_core::id::SymbolId(i));
        let mut keys = vec![
            Key::Erratum(SectionId(1), 8, Erratum::E843419),
            Key::Plt(k(1)),
            Key::Sym(k(2), 0),
            Key::Erratum(SectionId(1), 4, Erratum::E843419),
            Key::Sym(k(1), 4),
            Key::Sym(k(1), -4),
            Key::Erratum(SectionId(0), 100, Erratum::E843419),
            Key::Plt(k(0)),
        ];
        keys.sort();
        assert_eq!(
            keys,
            vec![
                Key::Sym(k(1), -4),
                Key::Sym(k(1), 4),
                Key::Sym(k(2), 0),
                Key::Plt(k(0)),
                Key::Plt(k(1)),
                Key::Erratum(SectionId(0), 100, Erratum::E843419),
                Key::Erratum(SectionId(1), 4, Erratum::E843419),
                Key::Erratum(SectionId(1), 8, Erratum::E843419),
            ]
        );
    }
}
