//! WP-273 (`spec/landed/incr/WP-271.md` §2 "Option (a) in detail", as
//! ruled and amended by the review, R-1..R-6): the patch islands of an
//! incremental update re-planned at the update's own addresses, in ONE
//! round, with every island FIXED (R-1: the caller runs this only when no
//! `Patches` chunk's address changed).
//!
//! Called by the update alone (front 2's replay hook, `mortar::front`);
//! a plain link never calls it (its fixed point is [`crate::fixed`]'s and
//! is not changed).
//!
//! The judgement is the full link's (`fixed.rs` `needs`):
//! `Arch::erratum_sites` over the pre-fix words of a window,
//! `need::erratum_need` for the fix and the patch reach, `need::thunk_need`
//! for a branch. What changes is where a new patch goes: it is APPENDED to
//! its part's island inside the island's fixed capacity (L-115's key
//! order holds for a full link only), so no other address moves and one
//! round is the fixed point.

use std::collections::{BTreeMap, BTreeSet};

use mortar_arch::need::{ErratumNeed, ThunkNeed, erratum_need, thunk_need};
use mortar_arch::{Arch, Code, Erratum};
use mortar_core::id::{ChunkId, OutSecId, SectionId};
use mortar_core::layout::{
    ChunkKind, Hole, HoleKind, Part, Patch, PatchKind, Placement, Redirect, Sized,
};
use mortar_elf::consts::{PF_X, PT_LOAD, SHF_EXECINSTR};

use crate::diag::{LayoutDiag, internal};
use crate::fixed::{BranchAt, OwnedCode};

/// The pre-fix words of an update's exec output sections at its own
/// placement (WP-271 §2 "Where it runs"): phase A's provider is front 2's
/// `LinkSites` (every word, at the replayed placement); phase B's is C'
/// (the refill set's words only). The words are those `fill` writes before
/// any erratum fix, never the previous image's (ruling item 4: an old
/// `b patch` at `F` or an old ADR at `W1` would hide a site or fake one).
pub trait WindowWords {
    /// The words of `[lo, hi)` (addresses, 4-aligned) of exec output
    /// section `out`, with their proof runs relative to `lo`
    /// (`OwnedCode::addr` is `lo`). Bytes no input section covers are
    /// zero with no proof (padding). `Ok(None)`: `out` has no words.
    ///
    /// # Errors
    /// A failed read of an input section's bytes.
    fn words(&self, out: OutSecId, lo: u64, hi: u64) -> Result<Option<OwnedCode>, LayoutDiag>;
}

/// The branch sites of the link and their final targets at the update's
/// placement (`Sites::branches`, `Sites::targets`), for the v1 thunk
/// judgement.
#[derive(Debug, Clone, Copy)]
pub struct Branches<'a> {
    pub sites: &'a [BranchAt],
    /// `[i]` is `sites[i]`'s target.
    pub targets: &'a [Option<u64>],
}

/// What [`replan`] reads.
#[derive(Clone, Copy)]
pub struct Input<'a> {
    pub arch: &'a dyn Arch,
    /// The update's placement (U7's addresses, final), with the stored
    /// patches and redirects translated (materialize).
    pub placement: &'a Placement,
    /// The refilled exec bytes, as address ranges `[lo, hi)`, sorted and
    /// disjoint (the dirty set: every 843419 window meeting one is judged
    /// again, WP-271 §2 step 1).
    pub dirty: &'a [(u64, u64)],
    pub words: &'a dyn WindowWords,
    /// `None`: no branch sites given (an architecture with no thunks).
    pub branches: Option<Branches<'a>>,
    /// L-112's island cap `C` (`IslandRules::cap`); 0: no islands.
    pub cap: u64,
}

impl std::fmt::Debug for Input<'_> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Input")
            .field("dirty", &self.dirty.len())
            .field("branches", &self.branches.map(|b| b.sites.len()))
            .field("cap", &self.cap)
            .finish_non_exhaustive()
    }
}

/// How a judged site is fixed (a64.md §11.2).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Fix {
    /// `W1` becomes the ADR (no redirect).
    Adr,
    /// `F` branches to the patch at this address.
    Patch(u64),
}

/// One site judged at the update's addresses: its key `(SectionId,
/// offset of F)`, `Q`, `F` and its fix.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Judged {
    pub section: SectionId,
    pub offset: u64,
    pub q: u64,
    pub f: u64,
    pub fix: Fix,
}

/// The re-planned rows (WP-271 §2 step 6): written whole.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Replan {
    /// Sorted by (chunk, offset) (`check.rs` rule 4): appends and holes
    /// renumber later redirects, and every redirect is rewritten.
    pub patches: Vec<Patch>,
    /// Sorted by (section, offset).
    pub redirects: Vec<Redirect>,
    /// The placement's holes, unchanged (gone keys' holes are
    /// materialize's, [`hole_gone`]).
    pub holes: Vec<Hole>,
    /// The placement's `sized` rows with every island's size re-planned.
    pub sized: Vec<Sized>,
    /// Every site of the judged windows, sorted.
    pub judged: Vec<Judged>,
    /// Patches appended; erratum redirects dropped (stale or out of their
    /// island); redirects kept or added.
    pub appended: u32,
    pub dropped: u32,
}

/// Why the update cannot take the re-plan (each a named fall-back).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Refused {
    /// `island-exhausted`: a new patch does not fit its island's
    /// capacity, the cap `C` (R-5) or the exact reach.
    IslandExhausted(String),
    /// `update-not-implemented (layout)`: what v1 does not do (a thunk
    /// need, L-113's fast path failing, a first patch that would move an
    /// empty island, an unproved candidate).
    Layout(String),
    /// A diagnostic of the words' provider, or an internal error.
    Diag(LayoutDiag),
}

impl From<LayoutDiag> for Refused {
    fn from(d: LayoutDiag) -> Self {
        Self::Diag(d)
    }
}

/// The 843419 start offsets L-118 forbids for a non-empty synthetic code
/// chunk (page offsets mod 4096).
const FORBIDDEN: [u64; 4] = [0xFFC, 0x000, 0x004, 0x008];

/// How far an 843419 window reaches around a word it holds: a window is
/// `[p − 8, p + 12)` for a page end `p` (a64.md §11.1, "The words the
/// predicate reads"), 20 bytes.
const WINDOW: u64 = 20;

/// The addresses of every exec output section's chunks: (out, lo, hi).
fn extents(pl: &Placement) -> Vec<(OutSecId, u64, u64)> {
    let mut by: BTreeMap<OutSecId, (u64, u64)> = BTreeMap::new();
    for (_, c) in pl.chunks.iter() {
        let (Some(out), Some(a)) = (c.kind.out(), c.addr) else { continue };
        if !matches!(c.kind, ChunkKind::Body(_) | ChunkKind::Patches(_))
            || c.flags & SHF_EXECINSTR == 0
        {
            continue;
        }
        let e = a.saturating_add(c.size);
        let x = by.entry(out).or_insert((a, e));
        *x = (x.0.min(a), x.1.max(e));
    }
    by.into_iter().map(|(o, (l, h))| (o, l, h)).collect()
}

/// The whole exec extents as a dirty set: what a full scan judges (phase
/// A's call, and the lane's compare).
#[must_use]
pub fn full_dirty(pl: &Placement) -> Vec<(u64, u64)> {
    let mut v: Vec<(u64, u64)> =
        extents(pl).into_iter().filter(|e| e.2 > e.1).map(|e| (e.1, e.2)).collect();
    v.sort_unstable();
    v
}

/// Per exec output section, the views to read: each dirty range widened by
/// a window on both sides (so every window that meets it is whole),
/// clipped to the section's extent, 4-aligned, merged.
fn views(pl: &Placement, dirty: &[(u64, u64)]) -> Vec<(OutSecId, u64, u64)> {
    let mut out = Vec::new();
    for (sec, lo, hi) in extents(pl) {
        let mut v: Vec<(u64, u64)> = Vec::new();
        for &(s, e) in dirty {
            let a = s.saturating_sub(WINDOW).max(lo) & !3;
            let b = e.saturating_add(WINDOW).min(hi);
            if a >= b {
                continue;
            }
            match v.last_mut() {
                Some(l) if a <= l.1 => l.1 = l.1.max(b),
                _ => v.push((a, b)),
            }
        }
        out.extend(v.into_iter().map(|(a, b)| (sec, a, b)));
    }
    out
}

/// A site found: `Q`, `F`, the pre-fix `W1`, its erratum and the part of
/// its final word's section.
type SiteAt = (u64, u64, u32, Erratum, Part);

/// The placed sections of every exec body chunk, by address: the map
/// from a site's final word to its key.
struct Bodies {
    /// (address, section, part), sorted.
    at: Vec<(u64, SectionId, Part)>,
}

impl Bodies {
    fn new(pl: &Placement) -> Self {
        let mut at = Vec::new();
        for (s, p) in pl.place.iter() {
            let Some((chunk, off)) = p.get() else { continue };
            let Some(c) = pl.chunks.get(chunk) else { continue };
            let (ChunkKind::Body(part), Some(a)) = (c.kind, c.addr) else { continue };
            if c.flags & SHF_EXECINSTR == 0 {
                continue;
            }
            at.push((a + off, s, part));
        }
        at.sort_unstable();
        // A section ends where the next one starts (or its chunk ends):
        // the key lookup needs only "the last section at or before".
        Self { at }
    }

    /// The section holding the word at `addr` and the offset in it.
    fn key(&self, addr: u64) -> Option<(SectionId, u64, Part)> {
        let i = self.at.partition_point(|x| x.0 <= addr).checked_sub(1)?;
        let (a, s, part) = self.at[i];
        Some((s, addr - a, part))
    }
}

fn in_ranges(r: &[(u64, u64)], a: u64) -> bool {
    let i = r.partition_point(|x| x.1 <= a);
    r.get(i).is_some_and(|x| x.0 <= a)
}

/// L-113 at the update's addresses (WP-271 ruling 3, v1): the RX
/// `PT_LOAD`, with every island at its cap `C`, spans at most `2²⁷ − 4`
/// bytes, and it is the only executable `PT_LOAD`. `Err` names what fails.
///
/// # Errors
/// The name of the failing condition.
pub fn fast_path(pl: &Placement, cap: u64) -> Result<(), String> {
    let rx: Vec<_> = pl
        .segments
        .as_slice()
        .iter()
        .filter(|g| g.p_type == PT_LOAD && g.flags & PF_X != 0 && g.memsz > 0)
        .collect();
    let [g] = rx.as_slice() else {
        return Err(format!("{} executable PT_LOADs (L-113 needs one)", rx.len()));
    };
    let (lo, hi) = (g.addr, g.addr.saturating_add(g.memsz));
    let mut span = g.memsz;
    for (_, c) in pl.chunks.iter() {
        if let (ChunkKind::Patches(_), Some(a)) = (c.kind, c.addr)
            && a >= lo
            && a <= hi
        {
            span = span.saturating_add(cap.saturating_sub(c.size));
        }
    }
    let limit = (1u64 << 27) - 4;
    if span > limit {
        return Err(format!(
            "the RX PT_LOAD at {lo:#x} spans {span:#x} with every island at its cap, over \
             {limit:#x} (L-113's fast path)"
        ));
    }
    Ok(())
}

/// **The re-plan** (WP-271 §2 option (a), steps 1-6, v1 with islands
/// fixed): the dirty windows judged at the update's addresses; a site that
/// needs a patch keeps its redirect, reuses its key's patch in its part's
/// island, or gets a new one appended there; an erratum redirect whose
/// site is gone or whose section has no place is dropped (its patch stays
/// unused, L-116); every branch judged by `thunk_need` (v1: a need is a
/// refusal). Nothing moves: an island grows inside its capacity only.
///
/// An architecture without errata and without thunks (x86-64) gets the
/// placement's rows back unchanged.
///
/// # Errors
/// [`Refused`], each by name.
#[allow(clippy::too_many_lines, reason = "the record's steps 1-6 in order")]
pub fn replan(input: &Input<'_>) -> Result<Replan, Refused> {
    let pl = input.placement;
    let arch = input.arch;
    let mut patches: Vec<Patch> = pl.patches.to_vec();
    let unchanged = |judged| Replan {
        patches: pl.patches.to_vec(),
        redirects: pl.redirects.to_vec(),
        holes: pl.holes.to_vec(),
        sized: pl.sized.to_vec(),
        judged,
        appended: 0,
        dropped: 0,
    };
    let islands: BTreeMap<Part, ChunkId> = pl
        .chunks
        .iter()
        .filter_map(|(id, c)| match c.kind {
            ChunkKind::Patches(p) => Some((p, id)),
            _ => None,
        })
        .collect();
    if islands.is_empty() {
        return Ok(unchanged(Vec::new()));
    }
    // Step 5 (thunks, v1): L-113's fast path, then every branch judged.
    if arch.thunk().is_some() {
        fast_path(pl, input.cap).map_err(Refused::Layout)?;
        if let Some(b) = input.branches {
            if b.targets.len() != b.sites.len() {
                return Err(internal("replan: not one branch target per branch site").into());
            }
            for (s, t) in b.sites.iter().zip(b.targets) {
                let Some(p) = pl.section_addr(s.section, s.offset) else { continue };
                let Some(t) = *t else {
                    return Err(internal("replan: a branch site with no target address").into());
                };
                let r = redirect_of(pl, &patches, s.section, s.offset);
                if thunk_need(&s.site, p, t, r) != ThunkNeed::None {
                    return Err(Refused::Layout(format!(
                        "a branch at {p:#x} (section {} offset {:#x}) needs a thunk to {t:#x} at \
                         the update's addresses (v1 judges no new thunk)",
                        s.section.0, s.offset
                    )));
                }
            }
        }
    }
    let errata = arch.errata();
    if errata.is_empty() {
        return Ok(unchanged(Vec::new()));
    }
    // Step 1-2: the dirty windows, judged on the pre-fix words.
    let bodies = Bodies::new(pl);
    let mut sites: BTreeMap<(SectionId, u64), SiteAt> = BTreeMap::new();
    for (out, lo, hi) in views(pl, input.dirty) {
        let Some(code) = input.words.words(out, lo, hi)? else { continue };
        let view = Code { bytes: &code.bytes, addr: code.addr, proof: &code.proof };
        for &e in errata {
            let mut found = Vec::new();
            arch.erratum_sites(e, &view, &mut found).map_err(|u| {
                Refused::Layout(format!(
                    "an erratum 843419 candidate at {:#x} with no mapping symbol proof at the \
                     update's addresses (a full link refuses it by name)",
                    code.addr.wrapping_add(u.q)
                ))
            })?;
            for s in found {
                let (q, f) = (code.addr.wrapping_add(s.q), code.addr.wrapping_add(s.f));
                let w1 =
                    view.word(s.q).ok_or_else(|| internal("replan: a site outside its code"))?;
                let (section, offset, part) = bodies.key(f).ok_or_else(|| {
                    internal("replan: an erratum site whose final word is in no input section")
                })?;
                sites.insert((section, offset), (q, f, w1, e, part));
            }
        }
    }
    // The window of a final word in a dirty range was read whole: a
    // redirect there whose key is no site is stale.
    let judged_at = |f: u64| in_ranges(input.dirty, f);
    let mut ends: BTreeMap<ChunkId, u64> =
        islands.values().map(|&c| (c, pl.chunks.get(c).map_or(0, |x| x.size))).collect();
    // Redirects as (key) -> patch row; dropped ones counted.
    let mut kept: BTreeMap<(SectionId, u64), u32> = BTreeMap::new();
    let mut dropped = 0u32;
    for r in &pl.redirects {
        let erratum = matches!(
            patches.get(r.patch as usize).map(|p| p.kind),
            Some(PatchKind::Erratum { .. })
        );
        let Some(f) = pl.section_addr(r.section, r.offset) else {
            // No place (killed, or not in this link): its site is gone.
            dropped += 1;
            continue;
        };
        if erratum && judged_at(f) && !sites.contains_key(&(r.section, r.offset)) {
            dropped += 1;
            continue;
        }
        kept.insert((r.section, r.offset), r.patch);
    }
    let patch_addr = |patches: &[Patch], i: u32| -> Option<u64> {
        let p = patches.get(i as usize)?;
        pl.chunks.get(p.chunk)?.addr?.checked_add(p.offset)
    };
    let mut judged = Vec::with_capacity(sites.len());
    let mut appended = 0u32;
    for (&(section, offset), &(q, f, w1, e, part)) in &sites {
        let island =
            *islands.get(&part).ok_or_else(|| internal("replan: a body part with no island"))?;
        // Its redirect, if it is to a patch of its island in reach.
        if let Some(&i) = kept.get(&(section, offset)) {
            let same = patches.get(i as usize).is_some_and(|p| p.chunk == island);
            let at = patch_addr(&patches, i);
            if same && erratum_need(arch, e, q, w1, f, at) == ErratumNeed::None {
                judged.push(Judged { section, offset, q, f, fix: Fix::Patch(at.unwrap_or(0)) });
                continue;
            }
            kept.remove(&(section, offset));
            dropped += 1;
        }
        if erratum_need(arch, e, q, w1, f, None) == ErratumNeed::None {
            judged.push(Judged { section, offset, q, f, fix: Fix::Adr });
            continue;
        }
        // Reuse: an unused patch of its key in its island, in reach.
        let key = PatchKind::Erratum { erratum: e, section, offset };
        let reuse = patches.iter().position(|p| p.chunk == island && p.kind == key);
        if let Some(i) = reuse {
            let i = u32::try_from(i).map_err(|_| internal("replan: too many patches"))?;
            let at = patch_addr(&patches, i);
            if erratum_need(arch, e, q, w1, f, at) == ErratumNeed::None {
                kept.insert((section, offset), i);
                judged.push(Judged { section, offset, q, f, fix: Fix::Patch(at.unwrap_or(0)) });
                continue;
            }
            // Out of reach in its own island: L-112 says never (a second
            // patch of one key in one island breaks rule 4).
            return Err(Refused::IslandExhausted(format!(
                "the patch of site {}+{offset:#x} in chunk {} is out of reach of F {f:#x}",
                section.0, island.0
            )));
        }
        // Append (step 3).
        let c = pl.chunks.get(island).ok_or_else(|| internal("replan: no island chunk"))?;
        let start = c.addr.ok_or_else(|| internal("replan: an island with no address"))?;
        let shape = arch.patch(e).ok_or_else(|| internal("replan: an erratum with no patch"))?;
        let align = u64::from(shape.align.max(1));
        let end = ends[&island];
        if end == 0 && FORBIDDEN.contains(&(start & 0xFFF)) {
            return Err(Refused::Layout(format!(
                "the first patch of the empty island chunk {} at {start:#x} (page offset {:#x}): \
                 L-118's padding would move the island (R-1: islands stay fixed in v1)",
                island.0,
                start & 0xFFF
            )));
        }
        let off = end.div_ceil(align) * align;
        let new_end = off + u64::from(shape.size);
        let room = c.capacity.max(c.size);
        if new_end > room || new_end > input.cap {
            return Err(Refused::IslandExhausted(format!(
                "a patch for site {}+{offset:#x} (F {f:#x}) ends at {new_end:#x} in island chunk \
                 {}, past its capacity {room:#x} or the cap {:#x} (L-112, R-5)",
                section.0, island.0, input.cap
            )));
        }
        let at = start + off;
        if erratum_need(arch, e, q, w1, f, Some(at)) != ErratumNeed::None {
            return Err(Refused::IslandExhausted(format!(
                "a patch for site {}+{offset:#x} at {at:#x} is out of reach of F {f:#x}",
                section.0
            )));
        }
        let i = u32::try_from(patches.len()).map_err(|_| internal("replan: too many patches"))?;
        patches.push(Patch { chunk: island, offset: off, kind: key });
        ends.insert(island, new_end);
        kept.insert((section, offset), i);
        appended += 1;
        judged.push(Judged { section, offset, q, f, fix: Fix::Patch(at) });
    }
    // Step 6: the rows, written whole. Patches sorted by (chunk, offset);
    // every redirect renumbered.
    let mut order: Vec<u32> = (0..patches.len() as u32).collect();
    order.sort_by_key(|&i| (patches[i as usize].chunk, patches[i as usize].offset));
    let mut renum = vec![0u32; patches.len()];
    for (new, &old) in order.iter().enumerate() {
        renum[old as usize] = new as u32;
    }
    let patches: Vec<Patch> = order.iter().map(|&i| patches[i as usize]).collect();
    let redirects: Vec<Redirect> = kept
        .into_iter()
        .map(|((section, offset), p)| Redirect { section, offset, patch: renum[p as usize] })
        .collect();
    let sized = pl
        .sized
        .iter()
        .map(|s| Sized { chunk: s.chunk, size: ends.get(&s.chunk).copied().unwrap_or(s.size) })
        .collect();
    judged.sort_unstable();
    Ok(Replan { patches, redirects, holes: pl.holes.to_vec(), sized, judged, appended, dropped })
}

/// The address of the patch the site `(section, offset)` is redirected to.
fn redirect_of(pl: &Placement, patches: &[Patch], section: SectionId, offset: u64) -> Option<u64> {
    let at =
        pl.redirects.binary_search_by_key(&(section, offset), |r| (r.section, r.offset)).ok()?;
    let p = patches.get(pl.redirects[at].patch as usize)?;
    pl.chunks.get(p.chunk)?.addr?.checked_add(p.offset)
}

/// [`hole_gone`]'s answer: the patches kept, the redirects renumbered, the
/// new holes.
pub type Holed = (Vec<Patch>, Vec<Redirect>, Vec<Hole>);

/// WP-271 §2 step 4 (materialize's half): the patches at the positions
/// `gone` (their key names a section of a changed or removed file) become
/// `Fill` holes of their island (R-6: the image writes a hole as zeros,
/// as a full link writes an unused island byte), their rows are removed,
/// and every redirect to them is dropped; the other redirects are
/// renumbered. A hole runs from the patch to the next patch of its island
/// or the island's end (`size`), so it covers the patch's words and its
/// alignment padding and overlaps nothing.
///
/// # Errors
/// `internal-error` for a position past the table or an island with no
/// size.
pub fn hole_gone(
    patches: &[Patch],
    redirects: &[Redirect],
    gone: &BTreeSet<u32>,
    island_size: &dyn Fn(ChunkId) -> Option<u64>,
) -> Result<Holed, LayoutDiag> {
    let mut holes = Vec::new();
    let mut renum: Vec<Option<u32>> = Vec::with_capacity(patches.len());
    let mut out = Vec::with_capacity(patches.len());
    for (i, p) in patches.iter().enumerate() {
        if !gone.contains(&(i as u32)) {
            renum.push(Some(out.len() as u32));
            out.push(*p);
            continue;
        }
        renum.push(None);
        let next = patches[i + 1..].iter().find(|n| n.chunk == p.chunk).map(|n| n.offset);
        let end = match next {
            Some(n) => n,
            None => island_size(p.chunk).ok_or_else(|| internal("hole_gone: no island size"))?,
        };
        if end <= p.offset {
            return Err(internal("hole_gone: a patch at or past its island's end"));
        }
        holes.push(Hole {
            chunk: p.chunk,
            offset: p.offset,
            size: end - p.offset,
            kind: HoleKind::Fill,
        });
    }
    if gone.iter().any(|&g| g as usize >= patches.len()) {
        return Err(internal("hole_gone: a position past PATCHES"));
    }
    let redirects = redirects
        .iter()
        .filter_map(|r| Some(Redirect { patch: renum.get(r.patch as usize).copied()??, ..*r }))
        .collect();
    Ok((out, redirects, holes))
}

/// The first difference between two re-plans of one placement (the lane's
/// full-scan compare: a dirty-only scan against a full one), or `None`.
#[must_use]
pub fn difference(dirty: &Replan, full: &Replan) -> Option<String> {
    if dirty.patches != full.patches {
        return Some(format!("patches: {} vs {}", dirty.patches.len(), full.patches.len()));
    }
    if dirty.redirects != full.redirects {
        return Some(format!("redirects: {:?} vs {:?}", dirty.redirects, full.redirects));
    }
    if dirty.sized != full.sized {
        return Some(format!("sized: {:?} vs {:?}", dirty.sized, full.sized));
    }
    let a: BTreeSet<&Judged> = dirty.judged.iter().collect();
    if let Some(j) = a.iter().find(|j| !full.judged.contains(j)) {
        return Some(format!("a site the full scan does not have: {j:?}"));
    }
    None
}

#[cfg(test)]
#[path = "replan_tests.rs"]
mod tests;
