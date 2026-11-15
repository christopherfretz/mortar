//! The thunk and erratum need functions (`spec/arch.md` §6 step 1,
//! `mortar_arch::need`), and a reference driver of the fixed point over a
//! synthetic layout with the island policy of `spec/layout.md` §14.1
//! (WP-023 stage B).
#![allow(clippy::cast_possible_truncation, clippy::many_single_char_names)]

mod support;

use std::collections::BTreeMap;

use mortar_a64::erratum;
use mortar_arch::need::{
    BranchSite, ErratumNeed, R_MAX, ThunkNeed, branch_site, erratum_need, reaches, thunk_need,
};
use mortar_arch::{
    Arch, DemandFlags, Erratum, OutputKind, PatchCx, Reloc, RelocAt, Site, TargetKind,
};
use mortar_core::layout::ThunkTarget;
use mortar_core::{FileId, SectionId};
use support::{Case, M2, Tables, key, words};

const E: Erratum = Erratum::E843419;
const BL: u32 = 0x9400_0000;
const MAX: u64 = (1 << 27) - 4;

fn call26() -> BranchSite {
    let h = M2.classify(283, Site { bytes: &[0; 4], offset: 0 }).unwrap();
    BranchSite { branch: M2.branch(&h).unwrap(), key: ThunkTarget::Sym { key: key(1), addend: 0 } }
}

/// `adrp xn` at `q` computing `Page(v)`.
fn adrp(q: u64, v: u64, n: u32) -> u32 {
    mortar_a64::forms::insert(
        mortar_a64::forms::ADRP,
        0x9000_0000 | n,
        ((v & !0xFFF).wrapping_sub(q & !0xFFF)) as i64,
    )
}

// ---------------------------------------------------------------- the functions

#[test]
fn s6_thunk_need() {
    let s = call26();
    let p = 0x4000_0000;
    assert_eq!(thunk_need(&s, p, p + MAX, None), ThunkNeed::None);
    assert_eq!(thunk_need(&s, p, p + MAX + 4, None), ThunkNeed::Unredirected);
    assert_eq!(thunk_need(&s, p, p - (1 << 27), None), ThunkNeed::None);
    assert_eq!(thunk_need(&s, p, p - (1 << 27) - 4, None), ThunkNeed::Unredirected);
    // A redirect is judged by its thunk, never by the direct target.
    assert_eq!(thunk_need(&s, p, p + 8, Some(p + MAX + 4)), ThunkNeed::ThunkOutOfReach);
    assert_eq!(thunk_need(&s, p, 1 << 40, Some(p + 64)), ThunkNeed::None);
    assert!(reaches(s.branch.reach, u64::MAX - 3, 4), "modulo 2^64");
}

#[test]
fn s6_branch_sites() {
    let mut t = Tables::default();
    t.sym(key(1), 0x40_0000, TargetKind::Local);
    let input = words(&[BL, 0x5400_0000]);
    let relocs = [
        Reloc { r_offset: 0, key: key(1), r_type: 283, addend: 8 },
        Reloc { r_offset: 4, key: key(1), r_type: 280, addend: 0 },
    ];
    let proof = support::all_code(8);
    let site = |i: usize, target, flags| {
        let r = relocs[i];
        let h = M2.classify(r.r_type, Site { bytes: &input, offset: r.r_offset }).unwrap();
        let at = RelocAt {
            file: FileId(1),
            section: SectionId(3),
            index: i as u32,
            r_offset: r.r_offset,
            key: r.key,
            r_type: r.r_type,
            addend: r.addend,
            howto: &h,
            site: Site { bytes: &input, offset: r.r_offset },
            records: &&relocs[..],
            proof: &proof,
            leads: true,
        };
        branch_site(&M2, OutputKind::Static, &at, &t, target, flags).map(|s| s.key)
    };
    let local = TargetKind::Local;
    assert_eq!(
        site(0, local, DemandFlags::empty()),
        Some(ThunkTarget::Sym { key: key(1), addend: 8 })
    );
    // Through the PLT or IPLT entry: keyed `Plt`.
    assert_eq!(
        site(0, TargetKind::Shlib { func: true }, DemandFlags::PLT),
        Some(ThunkTarget::Plt(key(1)))
    );
    assert_eq!(site(0, TargetKind::Ifunc, DemandFlags::IPLT), Some(ThunkTarget::Plt(key(1))));
    // Not sites: weak0 (X = 4), CONDBR19 (no veneer, a64.md §9).
    assert_eq!(site(0, TargetKind::Weak0, DemandFlags::empty()), None);
    assert_eq!(site(1, local, DemandFlags::empty()), None);
}

#[test]
fn s6_erratum_need() {
    // A site at Q ≡ 0xFF8 whose ADRP computes a page 1 MiB away or less:
    // fixed in place by ADR, so no need.
    let q = 0x4000_0FF8;
    let f = q + 8;
    assert_eq!(erratum_need(&M2, E, q, adrp(q, q + 0x8_0000, 1), f, None), ErratumNeed::None);
    // Farther: a patch is needed.
    let w1 = adrp(q, q + 0x20_0000, 1);
    assert_eq!(erratum_need(&M2, E, q, w1, f, None), ErratumNeed::Unpatched);
    // Redirected: judged by the two branches `b patch` and `b F + 4`.
    assert_eq!(erratum_need(&M2, E, q, w1, f, Some(f + 0x1000)), ErratumNeed::None);
    assert_eq!(erratum_need(&M2, E, q, w1, f, Some(f + MAX)), ErratumNeed::None);
    assert_eq!(erratum_need(&M2, E, q, w1, f, Some(f + MAX + 4)), ErratumNeed::PatchOutOfRange);
    // Backward, `b patch` reaches −2^27 but the way back from patch + 4
    // to F + 4 then needs +2^27: the lowest patch is F − 2^27 + 4.
    let back = f - (1 << 27) + 4;
    assert_eq!(erratum_need(&M2, E, q, w1, f, Some(back)), ErratumNeed::None);
    assert_eq!(erratum_need(&M2, E, q, w1, f, Some(back - 4)), ErratumNeed::PatchOutOfRange);
}

// ---------------------------------------------------------------- the driver

/// A target: an offset into a part, or an absolute address.
#[derive(Clone, Copy)]
enum Target {
    In { part: usize, off: u64 },
    Abs(u64),
}

#[derive(Clone, Copy)]
struct Branch {
    part: usize,
    off: u64,
    target: Target,
    key: u32,
}

#[derive(Clone, Copy)]
struct Site843419 {
    part: usize,
    q: u64,
    v: u64,
}

#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Debug)]
enum PatchKey {
    Thunk(u32),
    Erratum(usize),
}

struct Synthetic {
    base: u64,
    /// Part sizes (bytes, multiples of 4); island `k` follows part `k`.
    parts: Vec<u64>,
    branches: Vec<Branch>,
    errata: Vec<Site843419>,
}

/// The patch set: per island, its patches in key order (layout.md L-115).
type Islands = Vec<Vec<PatchKey>>;

/// Site → the patch it goes through (island, patch key).
type Redirects = BTreeMap<PatchKey, (usize, PatchKey)>;

const fn size(k: PatchKey) -> u64 {
    match k {
        PatchKey::Thunk(_) => 12,
        PatchKey::Erratum(_) => 8,
    }
}

struct Addrs {
    parts: Vec<u64>,
    patches: BTreeMap<(usize, PatchKey), u64>,
}

impl Synthetic {
    /// The addresses, a pure function of the patch set (arch.md §6).
    fn layout(&self, islands: &Islands) -> Addrs {
        let mut at = self.base;
        let mut parts = Vec::new();
        let mut patches = BTreeMap::new();
        for (k, size_k) in self.parts.iter().enumerate() {
            parts.push(at);
            at += size_k;
            for &p in &islands[k] {
                patches.insert((k, p), at);
                at += size(p);
            }
        }
        Addrs { parts, patches }
    }

    fn target(a: &Addrs, t: Target) -> u64 {
        match t {
            Target::In { part, off } => a.parts[part] + off,
            Target::Abs(x) => x,
        }
    }

    /// Runs the rounds; returns the patch set, the redirects and the number
    /// of rounds (the last one finding no need).
    fn run(&self) -> (Islands, Redirects, u32, Vec<usize>) {
        let site = call26();
        let mut islands: Islands = vec![Vec::new(); self.parts.len()];
        let mut redirects = Redirects::new();
        let mut growth = Vec::new();
        for round in 1..=R_MAX {
            let a = self.layout(&islands);
            // Step 1: the needs, judged at A_k, in output-address order.
            let mut needs: Vec<(u64, PatchKey)> = Vec::new();
            for (i, b) in self.branches.iter().enumerate() {
                let p = a.parts[b.part] + b.off;
                let t = Self::target(&a, b.target);
                let r = redirects.get(&PatchKey::Thunk(i as u32)).map(|x| a.patches[x]);
                if thunk_need(&site, p, t, r) != ThunkNeed::None {
                    needs.push((p, PatchKey::Thunk(i as u32)));
                }
            }
            for (i, e) in self.errata.iter().enumerate() {
                let q = a.parts[e.part] + e.q;
                let r = redirects.get(&PatchKey::Erratum(i)).map(|x| a.patches[x]);
                if erratum_need(&M2, E, q, adrp(q, e.v, 1), q + 8, r) != ErratumNeed::None {
                    needs.push((q + 8, PatchKey::Erratum(i)));
                }
            }
            growth.push(islands.iter().map(Vec::len).sum());
            if needs.is_empty() {
                return (islands, redirects, round, growth);
            }
            needs.sort_unstable();
            // Step 3: add, every reach judged at A_k.
            for (p, need) in needs {
                match need {
                    PatchKey::Thunk(i) => {
                        let b = self.branches[i as usize];
                        let key = PatchKey::Thunk(b.key);
                        // The in-reach thunk of the key with the lowest address.
                        let existing = a
                            .patches
                            .iter()
                            .filter(|((_, k), at)| *k == key && reaches(site.branch.reach, p, **at))
                            .min_by_key(|(_, at)| **at)
                            .map(|(x, _)| *x);
                        let chosen = existing.unwrap_or((b.part, key));
                        if !islands[chosen.0].contains(&chosen.1) {
                            islands[chosen.0].push(chosen.1);
                            islands[chosen.0].sort_unstable();
                        }
                        redirects.insert(need, chosen);
                    }
                    PatchKey::Erratum(i) => {
                        let chosen = (self.errata[i].part, need);
                        islands[chosen.0].push(need);
                        islands[chosen.0].sort_unstable();
                        redirects.insert(need, chosen);
                    }
                }
            }
        }
        panic!("refused: {:?}", mortar_arch::need::FIXED_POINT);
    }
}

/// layout.md L-110's part limit.
const S: u64 = (1 << 27) - (1 << 22) - 32;

#[test]
fn s6_fixed_point_two_rounds_of_thunks() {
    let far = 0x40_0000 + (1 << 30);
    let l = Synthetic {
        base: 0x40_0000,
        parts: vec![S, (1 << 22) + 64],
        branches: vec![
            // Round 1: out of reach; its thunk goes in island 0.
            Branch { part: 0, off: 0x100, target: Target::Abs(far), key: 1 },
            // In reach at A_1 by exactly 0 bytes; the round-1 thunk in
            // island 0 pushes its target out, so round 2 needs it.
            Branch { part: 0, off: 0, target: Target::In { part: 1, off: MAX - S }, key: 2 },
            // A second site of key 1 later in part 0 reuses the thunk.
            Branch { part: 0, off: 0x200, target: Target::Abs(far), key: 1 },
            // Backward, in reach: never a need.
            Branch { part: 1, off: 0x40, target: Target::In { part: 0, off: S - 4 }, key: 3 },
        ],
        errata: Vec::new(),
    };
    let (islands, redirects, rounds, growth) = l.run();
    assert_eq!(rounds, 3, "two rounds add, the third finds no need");
    assert!(rounds <= 2 * 2 + 1, "2N + 1 (arch.md §6 Termination)");
    assert_eq!(growth, [0, 1, 2], "monotone: patches are only added");
    assert_eq!(islands, [vec![PatchKey::Thunk(1), PatchKey::Thunk(2)], vec![]]);
    assert_eq!(redirects.len(), 3);
    assert_eq!(redirects[&PatchKey::Thunk(0)], redirects[&PatchKey::Thunk(2)], "one thunk per key");
    // Deterministic: the same tables give the same result.
    assert_eq!(l.run().0, islands);

    // fill: every redirected site branches to its thunk, and each thunk
    // reaches its target (a64.md §9).
    let a = l.layout(&islands);
    for (i, b) in l.branches.iter().enumerate() {
        let p = a.parts[b.part] + b.off;
        let t = Synthetic::target(&a, b.target);
        let via = redirects.get(&PatchKey::Thunk(i as u32)).map(|x| a.patches[x]);
        let mut tables = Tables::default();
        tables.sym(key(1), t, TargetKind::Local);
        if let Some(th) = via {
            tables.redirects.insert((SectionId(3), 0), th);
        }
        let input = words(&[BL]);
        let c = Case { arch: M2, ..Case::new(&tables, &input, p) };
        let w = c.word(283, 0, key(1), 0).expect("in reach after the fixed point");
        let dest = p.wrapping_add_signed(i64::from(((w & 0x03FF_FFFF) << 6).cast_signed() >> 4));
        assert_eq!(dest, via.unwrap_or(t), "site {i}");
        if let Some(th) = via {
            let cx = PatchCx {
                addr: th,
                target: t,
                site: p,
                word: 0,
                bti: false,
                target_word: None,
                target_abs: false,
            };
            let mut out = [0u8; 12];
            M2.write_thunk(&cx, &mut out).expect("the thunk reaches its target");
        }
    }
}

#[test]
fn s6_fixed_point_erratum_patch() {
    // An 843419 site at a page's 0xFF8 whose ADRP targets a page 2 MiB on:
    // ADR cannot reach, so round 1 adds a patch in island 0.
    let base = 0x40_0000;
    let l = Synthetic {
        base,
        parts: vec![0x2000, 0x1000],
        branches: Vec::new(),
        errata: vec![
            Site843419 { part: 0, q: 0xFF8, v: base + 0x20_0000 },
            Site843419 { part: 0, q: 0x1FF8, v: base + 0x1000 },
        ],
    };
    let (islands, redirects, rounds, growth) = l.run();
    assert_eq!(rounds, 2);
    assert_eq!(growth, [0, 1]);
    assert_eq!(islands, [vec![PatchKey::Erratum(0)], vec![]]);
    assert!(!redirects.contains_key(&PatchKey::Erratum(1)), "ADR reaches: fixed in place");
    // fill: `b patch` at F and the patch's branch back.
    let a = l.layout(&islands);
    let patch = a.patches[&(0, PatchKey::Erratum(0))];
    let f = base + 0xFF8 + 8;
    let b = mortar_a64::erratum_branch(f, patch).unwrap();
    assert_eq!(b, erratum::branch(f, patch).unwrap());
    let cx = PatchCx {
        addr: patch,
        target: f,
        site: base + 0xFF8,
        word: 0xF940_0021,
        bti: false,
        target_word: None,
        target_abs: false,
    };
    let mut out = [0u8; 8];
    M2.write_patch(E, &cx, &mut out).unwrap();
    assert_eq!(&out[..4], &0xF940_0021u32.to_le_bytes());
    // The second site's ADR fix.
    let q = base + 0x1FF8;
    assert!(mortar_a64::erratum_adr(q, adrp(q, base + 0x1000, 1)).is_some());
}
