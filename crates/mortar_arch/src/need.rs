//! The needs of one round of the range-extension fixed point (§6 step 1).
//!
//! Every function here judges one site at the addresses `A_k` the round
//! starts with, and is a pure function of its arguments. The iteration
//! itself (collecting the needs in parallel, sorting them, adding patches,
//! the cap of `R_MAX` rounds) is layout's (`spec/layout.md` §14), which
//! calls these.

use mortar_core::layout::ThunkTarget;
use mortar_diag::RefusalClass;

use crate::cx::{RelocAt, SymNames};
use crate::error::Refusal;
use crate::flags::DemandFlags;
use crate::shapes::{Branch, Erratum, Reach};
use crate::vocab::{Base, OutputKind, TargetKind};
use crate::{Arch, PatchCx};

/// The cap on rounds (§6 "The cap is part of the definition").
pub const R_MAX: u32 = 32;

/// The refusal of a link that needs more than [`R_MAX`] rounds, key
/// `(90, 0, 0)` (§6).
pub const FIXED_POINT: Refusal =
    Refusal::named(RefusalClass::Feature, "range-extension fixed point");

/// Whether a branch at `from` reaches `to`: `to − from` in `reach`,
/// computed modulo 2^64 as the field is.
#[must_use]
pub const fn reaches(reach: Reach, from: u64, to: u64) -> bool {
    reach.contains(to.wrapping_sub(from).cast_signed())
}

/// A branch site (§6 step 1): a branch whose class allows a veneer.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BranchSite {
    pub branch: Branch,
    /// The thunk key (a64.md §9 "Key"): `Plt` when the class routes the
    /// branch through the PLT or IPLT entry, else the symbol and addend.
    pub key: ThunkTarget,
}

/// The site a relocation is, if it is one (§6 step 1; a64.md §9 "Need").
/// Not a site: a type that is no branch or allows no veneer (an overflow
/// there is fill's `relocation-out-of-range`), a branch a matched sequence
/// consumes, a branch to an undefined weak (it is written `X = 4`, a64.md
/// §7). `flags` are the target's `Demands` flags.
#[must_use]
pub fn branch_site(
    arch: &dyn Arch,
    output: OutputKind,
    at: &RelocAt<'_>,
    names: &dyn SymNames,
    target: TargetKind,
    flags: DemandFlags,
) -> Option<BranchSite> {
    let branch = arch.branch(at.howto)?;
    if !branch.veneer || target == TargetKind::Weak0 || arch.consumed(output, at, names) {
        return None;
    }
    let via_plt = at.howto.value.base == Base::L
        && (flags.contains(DemandFlags::PLT) || flags.contains(DemandFlags::IPLT));
    let key = if via_plt {
        ThunkTarget::Plt(at.key)
    } else {
        ThunkTarget::Sym { key: at.key, addend: at.addend }
    };
    Some(BranchSite { branch, key })
}

/// What a branch site needs in this round (§6 step 1).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ThunkNeed {
    /// It reaches its target, or its redirect's thunk.
    None,
    /// Unredirected, and its final target is out of reach of `P`.
    Unredirected,
    /// Redirected, and its thunk is out of reach of `P`.
    ThunkOutOfReach,
}

/// The need of a branch site at `p` whose final target is `t` (the
/// symbol, its PLT or IPLT entry, plus `A`), redirected to the thunk at
/// `redirect` if any (§6 step 1).
#[must_use]
pub const fn thunk_need(site: &BranchSite, p: u64, t: u64, redirect: Option<u64>) -> ThunkNeed {
    match redirect {
        None if reaches(site.branch.reach, p, t) => ThunkNeed::None,
        None => ThunkNeed::Unredirected,
        Some(thunk) if reaches(site.branch.reach, p, thunk) => ThunkNeed::None,
        Some(_) => ThunkNeed::ThunkOutOfReach,
    }
}

/// What an erratum site needs in this round (§6 step 1).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ErratumNeed {
    /// Fixed in place, or its patch is within range.
    None,
    /// No redirect, and `erratum_in_place` does not fix it.
    Unpatched,
    /// Redirected, and one of the two branches between `F` and the patch
    /// is out of reach.
    PatchOutOfRange,
}

/// The need of an erratum site whose first word is at `q` and holds `w1`
/// (as `fill` writes it before any fix) and whose final word is at `f`,
/// redirected to the patch at `redirect` if any (§6 step 1).
#[must_use]
pub fn erratum_need(
    arch: &dyn Arch,
    e: Erratum,
    q: u64,
    w1: u32,
    f: u64,
    redirect: Option<u64>,
) -> ErratumNeed {
    match redirect {
        None => {
            let cx = PatchCx {
                addr: q,
                target: 0,
                site: q,
                word: w1,
                bti: false,
                target_word: None,
                target_abs: false,
            };
            if arch.erratum_in_place(e, &cx) { ErratumNeed::None } else { ErratumNeed::Unpatched }
        }
        Some(patch) => {
            let r = arch.patch_reach(e);
            let back = f.wrapping_add(4);
            if reaches(r, f, patch) && reaches(r, patch.wrapping_add(4), back) {
                ErratumNeed::None
            } else {
                ErratumNeed::PatchOutOfRange
            }
        }
    }
}
