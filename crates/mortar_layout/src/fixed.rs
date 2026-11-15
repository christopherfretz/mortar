//! Stage E (`spec/layout.md` §14, `spec/arch.md` §6): the fixed point of
//! range extension and erratum patches, over a minimal architecture
//! trait.
//!
//! The state is the patch set: per island (L-111) a set of patch keys,
//! and the redirects `(SectionId, offset) → (island, key)`. Each round
//! lays out the islands by key (L-115), runs stage D, asks the
//! architecture for the needs at those addresses, and adds patches. The
//! addresses are a function of the patch set alone (L-21), so the result
//! does not depend on the round that added a patch.

use std::collections::{BTreeMap, BTreeSet};

use mortar_core::column::Column;
use mortar_core::id::{ChunkId, SectionId, SymKey};
use mortar_core::layout::{
    Chunk, ChunkKind, Erratum, Part, Patch, PatchKind, Place, Redirect, SegmentRules, Sized,
    ThunkTarget,
};
use mortar_core::par::Pool;

use crate::address::{Addressed, address};
use crate::chunk::Staged;
use crate::diag::{LayoutDiag, feature, internal};
use crate::target::{Options, Target};

/// arch.md §6 item 6: more rounds than this is refused.
pub const R_MAX: u32 = 32;

/// The refusal of arch.md §6 item 6 and L-112.
fn cap_refusal() -> LayoutDiag {
    feature("range-extension fixed point")
}

/// What a need asks for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NeedKind {
    /// A branch that must go through a thunk to this target.
    Thunk(ThunkTarget),
    /// An erratum site that needs a patch (its key is the need's site).
    Erratum(Erratum),
}

/// One need of arch.md §6 step 1, judged at the round's addresses.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Need {
    /// The site's address at `A_k`.
    pub addr: u64,
    /// The site: the section and the offset of the branch or of the key
    /// instruction (the redirect key).
    pub section: SectionId,
    pub offset: u64,
    /// The relocation record index of the site in its section (the
    /// arch.md tie-break); 0 for an erratum site.
    pub record: u32,
    pub kind: NeedKind,
}

impl Need {
    /// arch.md §6 step 3: output-address order, ties by (`FileId`, section
    /// index, record index); `SectionId` order is (`FileId`, section index)
    /// order. The offset and kind make the order total.
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
            NeedKind::Thunk(ThunkTarget::Sym { key, addend }) => Self::Sym(key, addend),
            NeedKind::Thunk(ThunkTarget::Plt(key)) => Self::Plt(key),
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

/// What the architecture gives the driver (arch.md §6; `spec/a64.md` §9,
/// §11 for AArch64).
pub trait RangeArch: Sync {
    /// arch.md §6 step 1 at the round's addresses: every need, in any
    /// order (the driver sorts them). Collected in parallel on `pool` if
    /// the architecture wants; the result must be a function of `round`.
    fn needs(&self, round: &Round<'_>, pool: &Pool) -> Vec<Need>;

    /// The size and log2 alignment of a patch of this kind.
    fn patch_shape(&self, kind: PatchKind) -> (u64, u8);

    /// Whether the branch of thunk need `need` (at `need.addr`) reaches a
    /// thunk of its key at `patch` (`mortar_arch::need::reaches` with the
    /// site's branch reach). Only existing thunks are judged: a thunk
    /// created in round `r` is judged from round `r + 1` (WP-023 ruling).
    fn reaches(&self, need: &Need, patch: u64) -> bool;
}

/// One round's addresses and patch set, as the architecture sees them.
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

fn lay_out(keys: &BTreeSet<Key>, arch: &impl RangeArch) -> Laid {
    let mut rows = Vec::with_capacity(keys.len());
    let mut end = 0u64;
    for &k in keys {
        let (size, align) = arch.patch_shape(k.kind());
        let a = 1u64 << align.min(63);
        let at = end.div_ceil(a).saturating_mul(a);
        rows.push((k, at));
        end = at.saturating_add(size);
    }
    Laid { rows, size: end }
}

/// Stage E: rounds of arch.md §6 until one has no needs.
///
/// # Errors
///
/// `refused` (`feature`, `range-extension fixed point`): more than
/// [`R_MAX`] rounds, an island over its cap (L-112), a need with no island
/// in reach (a site outside every exec part, or a target with no
/// islands); stage D's errors.
#[allow(clippy::too_many_lines)]
pub fn fixed_point(
    staged: &Staged,
    target: &Target,
    options: &Options,
    rules: &SegmentRules,
    arch: &impl RangeArch,
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
    loop {
        number += 1;
        let laid: Vec<Laid> = sets.iter().map(|s| lay_out(s, arch)).collect();
        if laid.iter().any(|l| l.size > cap) {
            return Err(cap_refusal());
        }
        let island_sizes: Vec<u64> = laid.iter().map(|l| l.size).collect();
        let addressed = address(staged, &island_sizes, target, options, rules)?;
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
        let mut needs = arch.needs(&round, pool);
        if needs.is_empty() {
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
        let mut added: BTreeSet<(usize, Key)> = BTreeSet::new();
        for need in &needs {
            let key = Key::of_need(need);
            let site = (need.section, need.offset);
            if key.is_thunk()
                && let Some(&(_, island)) =
                    thunks.get(&key).and_then(|v| v.iter().find(|&&(a, _)| arch.reaches(need, a)))
            {
                redirect.insert(site, (island, key));
                continue;
            }
            // A new patch in the island of the site's part (L-111).
            let island = island_of_section(need.section).ok_or_else(cap_refusal)?;
            if sets[island].contains(&key) {
                // Only a patch this round added can be reused unjudged:
                // an older one was out of reach at A_k, which (C) excludes.
                if !added.contains(&(island, key)) {
                    return Err(cap_refusal());
                }
            } else {
                sets[island].insert(key);
                added.insert((island, key));
            }
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
