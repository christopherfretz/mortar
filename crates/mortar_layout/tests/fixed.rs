//! Stage E, the fixed point (`spec/layout.md` §14, `spec/arch.md` §6),
//! end to end with `mortar_a64`: its branch reach, its 12-byte thunks,
//! its 843419 predicate, ADR fix and patch reach. The sites (what the
//! relocation scan and fill would give) are synthetic.
#![allow(clippy::many_single_char_names, clippy::cast_possible_truncation)]

mod common;

use common::{World, sec};
use mortar_a64::A64;
use mortar_a64::forms::{ADRP, insert};
use mortar_arch::need::{BranchSite, ThunkNeed, reaches, thunk_need};
use mortar_arch::{Arch, PatchCx, Proof, ProofRun, Site, Status};
use mortar_core::id::{OutSecId, SectionId, SymKey, SymbolId};
use mortar_core::layout::{ChunkKind, Erratum, PatchKind, ThunkTarget};
use mortar_core::par::{Pool, check_schedules};
use mortar_core::resolution::Def;
use mortar_elf::Machine;
use mortar_elf::consts::{SHF_ALLOC, SHF_EXECINSTR, SHT_PROGBITS};
use mortar_layout::fixed::{BranchAt, OwnedCode, R_MAX, Round};
use mortar_layout::{LayoutDiag, Placed, RangeExtension, Sites};

const AX: u64 = SHF_ALLOC | SHF_EXECINSTR;
const MIB: u64 = 1 << 20;
/// B/BL's forward reach (a64.md §9).
const REACH: u64 = (1 << 27) - 4;
/// `--fix-cortex-a53-843419` on.
const FIX: A64 = A64::new(Status::M1, true);

/// Where a branch goes.
#[derive(Debug, Clone, Copy)]
enum To {
    At(SectionId, u64),
    Abs(u64),
}

/// The sites of a synthetic link.
#[derive(Debug, Clone, Default)]
struct Synthetic {
    branches: Vec<BranchAt>,
    to: Vec<To>,
    /// 843419 sequences `adrp x0; str x2, [x3]; ldr x1, [x0, #8]` at
    /// (section, offset of `W1`), the ADRP's target far (no ADR fix) or
    /// near.
    errata: Vec<(SectionId, u64, bool)>,
    /// The words of every sequence move a page further each round, so
    /// every round has a new site (never converges).
    moving: bool,
}

/// `R_AARCH64_CALL26`'s branch class.
fn call26(key: ThunkTarget) -> BranchSite {
    let h = A64::M1.classify(283, Site { bytes: &[0; 4], offset: 0 }).unwrap();
    BranchSite { branch: A64::M1.branch(&h).unwrap(), key }
}

/// Thunk key `i`: `_start` (the one symbol) plus `i`.
fn sym(i: u32) -> ThunkTarget {
    ThunkTarget::Sym { key: SymKey::global(SymbolId(0)), addend: i64::from(i) }
}

impl Synthetic {
    fn branch(&mut self, section: SectionId, offset: u64, key: u32, to: To) {
        let record = self.branches.len() as u32;
        self.branches.push(BranchAt { section, offset, record, site: call26(sym(key)) });
        self.to.push(to);
    }

    fn target_at(&self, i: usize, round: &Round<'_>) -> Option<u64> {
        match self.to[i] {
            To::At(s, o) => round.site_addr(s, o),
            To::Abs(x) => Some(x),
        }
    }
}

impl Sites for Synthetic {
    fn branches(&self) -> &[BranchAt] {
        &self.branches
    }

    fn targets(&self, round: &Round<'_>, _pool: &Pool) -> Vec<Option<u64>> {
        (0..self.branches.len()).map(|i| self.target_at(i, round)).collect()
    }

    fn code(
        &self,
        out: OutSecId,
        round: &Round<'_>,
        _pool: &Pool,
    ) -> Result<Option<OwnedCode>, LayoutDiag> {
        let Some((start, end)) = round.out_extent(out) else { return Ok(None) };
        let mut bytes = vec![0u8; (end - start) as usize];
        for &(s, off, far) in &self.errata {
            let off = if self.moving { off + 0x1000 * u64::from(round.number) } else { off };
            let Some(q) = round.site_addr(s, off) else { continue };
            if q < start || q + 12 > end {
                continue;
            }
            let v = if far { q + 0x1000_0000 } else { q + 0x1000 };
            let w1 = insert(ADRP, 0x9000_0000, (v & !0xFFF).wrapping_sub(q & !0xFFF) as i64);
            let at = (q - start) as usize;
            for (k, w) in [w1, 0xf900_0062, 0xf940_0401].iter().enumerate() {
                bytes[at + 4 * k..at + 4 * k + 4].copy_from_slice(&w.to_le_bytes());
            }
        }
        let proof = vec![ProofRun {
            start: 0,
            end: bytes.len() as u64,
            proof: Proof::Insn,
            unit: 0,
            unit_offset: 0,
        }];
        Ok(Some(OwnedCode { addr: start, bytes, proof }))
    }
}

fn run(w: &World, arch: &dyn Arch, sites: &Synthetic, pool: &Pool) -> Placed {
    let range = RangeExtension { arch, sites };
    let (a, placed) = w.run_with(Some(&range), pool).unwrap();
    w.check(&a, &placed).unwrap();
    placed
}

fn round_of(p: &Placed) -> Round<'_> {
    let p = &p.placement;
    Round {
        number: 0,
        chunks: &p.chunks,
        place: &p.place,
        patches: &p.patches,
        redirects: &p.redirects,
    }
}

/// The address of the patch a site is redirected to.
fn via(r: &Round<'_>, section: SectionId, offset: u64) -> Option<u64> {
    r.redirect(section, offset).and_then(|(i, _)| r.patch_addr(i))
}

/// fill's view of the fixed point: every branch site reaches its target
/// or its thunk, and every thunk its target (a64.md §9).
fn assert_branches_reach(placed: &Placed, sites: &Synthetic) {
    let r = round_of(placed);
    for (i, b) in sites.branches.iter().enumerate() {
        let p = r.site_addr(b.section, b.offset).unwrap();
        let t = sites.target_at(i, &r).unwrap();
        let thunk = via(&r, b.section, b.offset);
        assert_eq!(thunk_need(&b.site, p, t, thunk), ThunkNeed::None, "site {i}");
        if let Some(th) = thunk {
            assert!(reaches(b.site.branch.reach, p, th));
            let cx = PatchCx {
                addr: th,
                target: t,
                site: p,
                word: 0,
                bti: false,
                target_word: None,
                target_abs: false,
            };
            A64::M1.write_thunk(&cx, &mut [0u8; 12]).expect("the thunk reaches its target");
        }
    }
}

/// Three 100 MiB code sections in one `.text`: three parts (L-110), each
/// with its island (L-111).
fn world() -> World {
    world_sized(100 * MIB)
}

fn world_sized(a_size: u64) -> World {
    let files = vec![
        vec![sec(b".text.a", SHT_PROGBITS, AX, a_size, 2)],
        vec![sec(b".text.b", SHT_PROGBITS, AX, 100 * MIB, 2)],
        vec![sec(b".text.c", SHT_PROGBITS, AX, 100 * MIB, 2)],
    ];
    let mut w =
        World::new(Machine::AArch64, &files, &[(b"_start", Def::Undefined { weak: false })]);
    w.define(0, 1, 1, 0);
    w
}

/// Site X (.text.a+0 → .text.c+16) needs a thunk in round 1; site Y (the
/// last word of .text.a → .text.b, in reach by 4 bytes across island 0)
/// goes out of reach when island 0 grows: round 2 adds its thunk; round 3
/// has no need.
fn two_round_sites(w: &World) -> Synthetic {
    let (a, b, c) = (w.sid(1, 1), w.sid(2, 1), w.sid(3, 1));
    let mut s = Synthetic::default();
    s.branch(a, 0, 1, To::At(c, 16));
    s.branch(a, 100 * MIB - 4, 2, To::At(b, REACH - 8));
    s
}

/// A small link with two code output sections, `.init` and `.text`, each
/// its own part and island, page-aligned when `align` is 12.
fn small(align: u8) -> World {
    let files = vec![
        vec![sec(b".init", SHT_PROGBITS, AX, 0x3000, align)],
        vec![sec(b".text", SHT_PROGBITS, AX, 0x4_0000, align)],
    ];
    let mut w =
        World::new(Machine::AArch64, &files, &[(b"_start", Def::Undefined { weak: false })]);
    w.define(0, 2, 1, 0);
    w.options.fix_843419 = true;
    w
}

/// The section offset whose address has page offset 0xFF8, `page` pages on.
fn at_ff8(placed: &Placed, s: SectionId, page: u64) -> u64 {
    let addr = round_of(placed).site_addr(s, 0).unwrap();
    ((0xFF8 + 0x1000 - (addr & 0xFFF)) & 0xFFF) + 0x1000 * page
}

// spec: layout.md §14.1 L-110 (parts by size), L-111 (an island per part)
#[test]
fn l110_l111_parts_and_islands() {
    let w = world();
    let (a, placed) = w.run(&Pool::sequential()).unwrap();
    w.check(&a, &placed).unwrap();
    let p = &placed.placement;
    let kinds: Vec<ChunkKind> = p.order.iter().map(|&c| p.chunks[c].kind).collect();
    let bodies = kinds.iter().filter(|k| matches!(k, ChunkKind::Body(_))).count();
    let islands = kinds.iter().filter(|k| matches!(k, ChunkKind::Patches(_))).count();
    assert_eq!((bodies, islands), (3, 3));
    for w2 in kinds.windows(2) {
        if let ChunkKind::Body(part) = w2[0] {
            assert_eq!(w2[1], ChunkKind::Patches(part), "L-111: the island follows its part");
        }
    }
}

// spec: arch.md §6 (rounds, monotone, 2N+1, one thunk per key), layout.md L-111 (new thunks in the site's island), L-115
#[test]
fn fixed_point_two_rounds() {
    let w = world();
    let mut sites = two_round_sites(&w);
    // A second site of key 1 later in .text.a reuses X's thunk.
    sites.branch(w.sid(1, 1), 0x200, 1, To::At(w.sid(3, 1), 16));
    let placed = run(&w, &A64::M1, &sites, &Pool::sequential());
    let p = &placed.placement;
    assert_eq!(placed.rounds, 3, "two rounds that add, one that stops");
    assert_eq!(p.patches.len(), 2);
    assert!(placed.rounds <= 2 * p.patches.len() as u32 + 1);
    // Both thunks are in island 0 (the sites' part), in key order.
    let island0 = p.patches[0].chunk;
    assert!(p.patches.iter().all(|x| x.chunk == island0));
    assert_eq!(p.chunks[island0].kind.part().unwrap().part, 0);
    let keys: Vec<_> = p.patches.iter().map(|x| x.kind).collect();
    assert_eq!(keys, [PatchKind::Thunk(sym(1)), PatchKind::Thunk(sym(2))]);
    assert_eq!((p.patches[0].offset, p.patches[1].offset), (0, 12), "12-byte thunks");
    assert_eq!(p.redirects.len(), 3);
    assert_eq!(p.redirects[0].patch, p.redirects[1].patch, "one thunk per key");
    assert_branches_reach(&placed, &sites);
}

// spec: arch.md §6 "Determinism" (needs collected in parallel; one result)
#[test]
fn fixed_point_schedules() {
    let w = world();
    let sites = two_round_sites(&w);
    check_schedules(|pool| {
        let placed = run(&w, &A64::M1, &sites, pool);
        (placed.placement, placed.rounds)
    });
}

// spec: arch.md §6 "Addresses are a function of the patch set" (L-21): adding
// the same patches in one round gives the same layout as in two.
#[test]
fn l21_function_of_patch_set() {
    let w = world();
    let two = two_round_sites(&w);
    let by_two = run(&w, &A64::M1, &two, &Pool::sequential());
    // One round: Y's target is far enough to need its thunk at once.
    let mut one = two.clone();
    one.to[1] = To::At(w.sid(3, 1), 100 * MIB - 4);
    let by_one = run(&w, &A64::M1, &one, &Pool::sequential());
    assert_eq!(by_one.rounds, 2);
    let strip = |p: &Placed| {
        (p.placement.chunks.clone(), p.placement.segments.clone(), p.placement.patches.len())
    };
    assert_eq!(strip(&by_one), strip(&by_two));
}

// spec: arch.md §6 item 6 (more than R_max = 32 rounds is refused by name)
#[test]
fn cap_refusal_never_converging() {
    let w = small(12);
    let plain = w.run(&Pool::sequential()).unwrap().1;
    let text = w.sid(2, 1);
    let sites = Synthetic {
        errata: vec![(text, at_ff8(&plain, text, 0), true)],
        moving: true,
        ..Synthetic::default()
    };
    let range = RangeExtension { arch: &FIX, sites: &sites };
    let e = w.run_with(Some(&range), &Pool::sequential()).unwrap_err();
    assert_eq!(e[0].refusal().map(|r| r.1), Some("range-extension fixed point"));
    assert_eq!(R_MAX, 32);
}

// spec: layout.md §14.1 L-112 (an island over C = 4 MiB is refused like the cap)
#[test]
fn l112_island_cap() {
    let w = world();
    let mut sites = Synthetic::default();
    // One more 12-byte thunk than 4 MiB holds, each its own key.
    for i in 0..=(4 * MIB / 12) as u32 {
        sites.branch(w.sid(1, 1), 4 * u64::from(i), i, To::Abs(1 << 31));
    }
    let range = RangeExtension { arch: &A64::M1, sites: &sites };
    let e = w.run_with(Some(&range), &Pool::sequential()).unwrap_err();
    assert_eq!(e[0].refusal().map(|r| r.1), Some("range-extension fixed point"));
}

// spec: layout.md §14.1 L-110 (a code section larger than S is refused)
#[test]
fn l110_section_too_large() {
    let files = vec![vec![sec(b".text", SHT_PROGBITS, AX, (1 << 27) - (1 << 22), 2)]];
    let w = World::new(Machine::AArch64, &files, &[]);
    let e = w.run(&Pool::sequential()).unwrap_err();
    assert_eq!(e[0].refusal().map(|r| r.1), Some("code section larger than branch reach"));
}

// spec: layout.md §14.2 L-117 (an ADR fix in place, else an erratum patch), a64.md §11.2 (the patch's two branches reach)
#[test]
fn l117_erratum_patch_end_to_end() {
    let w = small(12);
    let plain = w.run(&Pool::sequential()).unwrap().1;
    let text = w.sid(2, 1);
    let (far, near) = (at_ff8(&plain, text, 0), at_ff8(&plain, text, 1));
    let sites =
        Synthetic { errata: vec![(text, far, true), (text, near, false)], ..Synthetic::default() };
    let placed = run(&w, &FIX, &sites, &Pool::sequential());
    let p = &placed.placement;
    assert_eq!(placed.rounds, 2, "one round adds the patch, the second has no need");
    assert_eq!(
        p.patches.iter().map(|x| x.kind).collect::<Vec<_>>(),
        [PatchKind::Erratum { erratum: Erratum::E843419, section: text, offset: far + 8 }],
        "keyed by the final word"
    );
    assert_eq!(p.redirects.len(), 1, "the near site is fixed in place by ADR");
    assert_eq!((p.redirects[0].section, p.redirects[0].offset), (text, far + 8));
    let r = round_of(&placed);
    let island = p.patches[0].chunk;
    assert_eq!(placed.staged.island_of(r.place[text].get().unwrap().0), Some(island));
    let f = r.site_addr(text, far + 8).unwrap();
    let patch = r.patch_addr(0).unwrap();
    mortar_a64::erratum_branch(f, patch).expect("b patch from F reaches");
    let cx = PatchCx {
        addr: patch,
        target: f,
        site: f - 8,
        word: 0xf940_0401,
        bti: false,
        target_word: None,
        target_abs: false,
    };
    FIX.write_patch(Erratum::E843419, &cx, &mut [0u8; 8]).expect("b F + 4 reaches");
    // Without the fix, no erratum is scanned and nothing is patched.
    let off = run(&w, &A64::M1, &sites, &Pool::sequential());
    assert!(off.placement.patches.is_empty());
}

// spec: layout.md §14.2 L-115 (thunks by key, then errata by site key), L-117 (an erratum patch in its site's island)
#[test]
fn l115_errata_after_thunks() {
    let w = small(12);
    let plain = w.run(&Pool::sequential()).unwrap().1;
    let (init, text) = (w.sid(1, 1), w.sid(2, 1));
    let mut sites = Synthetic {
        errata: vec![
            (text, at_ff8(&plain, text, 3), true),
            (init, at_ff8(&plain, init, 1), true),
            (text, at_ff8(&plain, text, 1), true),
        ],
        ..Synthetic::default()
    };
    sites.branch(text, 0x10, 2, To::Abs(1 << 31));
    sites.branch(text, 0x20, 1, To::Abs(1 << 31));
    let placed = run(&w, &FIX, &sites, &Pool::sequential());
    let p = &placed.placement;
    let r = round_of(&placed);
    let island = |s: SectionId| placed.staged.island_of(r.place[s].get().unwrap().0).unwrap();
    let erratum = |section, offset: u64| PatchKind::Erratum {
        erratum: Erratum::E843419,
        section,
        offset: offset + 8,
    };
    let rows: Vec<_> = p.patches.iter().map(|x| (x.chunk, x.kind)).collect();
    let mut expect = vec![
        (island(text), PatchKind::Thunk(sym(1))),
        (island(text), PatchKind::Thunk(sym(2))),
        (island(text), erratum(text, at_ff8(&plain, text, 1))),
        (island(text), erratum(text, at_ff8(&plain, text, 3))),
        (island(init), erratum(init, at_ff8(&plain, init, 1))),
    ];
    expect.sort_by_key(|&(c, _)| c);
    assert_eq!(rows, expect);
    assert_branches_reach(&placed, &sites);
}

// spec: a64.md §11.2 "Stale redirects" (an erratum redirect whose key is no longer a site is dropped; its patch stays, L-116)
#[test]
fn stale_erratum_redirect_dropped() {
    // Unaligned output sections: the first one's island moves the second.
    let w = small(2);
    let plain = w.run(&Pool::sequential()).unwrap().1;
    let r = round_of(&plain);
    let (init, text) = (w.sid(1, 1), w.sid(2, 1));
    let (first, second) =
        if r.site_addr(init, 0) < r.site_addr(text, 0) { (init, text) } else { (text, init) };
    let sites = Synthetic {
        errata: vec![
            (first, at_ff8(&plain, first, 0), true),
            (second, at_ff8(&plain, second, 0), true),
        ],
        ..Synthetic::default()
    };
    let placed = run(&w, &FIX, &sites, &Pool::sequential());
    let p = &placed.placement;
    assert_eq!(placed.rounds, 2);
    assert_eq!(p.patches.len(), 2, "both patches stay");
    assert_eq!(p.redirects.len(), 1, "the moved site's redirect is dropped");
    assert_eq!(p.redirects[0].section, first);
}

// spec: layout.md §14.2 L-118 (843419 start padding of a non-empty island, at most 16 bytes)
#[test]
fn l118_island_padding() {
    let island0 = |w: &World| {
        let sites = two_round_sites(w);
        let placed = run(w, &A64::M1, &sites, &Pool::sequential());
        let p = &placed.placement;
        let c = p.patches[0].chunk;
        (p.chunks[c].addr.unwrap(), p.place[w.sid(1, 1)].get().unwrap(), p.chunks.clone())
    };
    // Size .text.a so that island 0 starts at page offset 0 without the fix.
    let (addr, _, _) = island0(&world());
    let delta = (0x1000 - (addr & 0xfff)) & 0xfff;
    let plain = world_sized(100 * MIB + delta);
    let (at, (body, _), chunks) = island0(&plain);
    assert_eq!(at & 0xfff, 0);
    let mut fixed = plain.clone();
    fixed.options.fix_843419 = true;
    let (padded, _, _) = island0(&fixed);
    let end = chunks[body].addr.unwrap() + chunks[body].size;
    assert_eq!(end, at);
    assert!(![0xffc, 0, 4, 8].contains(&(padded & 0xfff)), "{padded:#x}");
    assert!(padded > at && padded - at <= 16, "{:#x}", padded - at);
}

/// `.symtab`-like and `.got`-like synthetic chunks of `w`, sized `sym` and
/// `got` bytes.
fn with_synthetic(mut w: World, sym: u64, got: u64) -> World {
    use mortar_core::layout::Synth;
    use mortar_elf::consts::{SHF_WRITE, SHT_SYMTAB};
    use mortar_layout::chunk::SynthChunk;
    w.synthetic = vec![
        SynthChunk {
            synth: Synth { name: b".got", address_dependent: false },
            sh_type: SHT_PROGBITS,
            flags: SHF_ALLOC | SHF_WRITE,
            align: 3,
            size: got,
        },
        SynthChunk {
            synth: Synth { name: b".symtab", address_dependent: false },
            sh_type: SHT_SYMTAB,
            flags: 0,
            align: 3,
            size: sym,
        },
    ];
    w
}

/// `place_again` from the placement of a link whose synthetic chunks had
/// other sizes, as the driver's second placement does.
fn placed_again(w: &World, sites: &Synthetic, prev: &Placed, pool: &Pool) -> Placed {
    let range = RangeExtension { arch: &A64::M1, sites };
    let mut a = w.stage_a(pool).unwrap();
    let entries = w.entries.clone();
    mortar_layout::place_again(
        &w.link(),
        &mut a,
        Some(&range),
        move |e| entries.iter().find(|(x, _)| *x == e).map(|(_, d)| *d),
        pool,
        Some(prev),
    )
    .unwrap()
}

// spec: layout.md §14 and L-21 (a function of the patch set), L-72
// (non-alloc chunks after the alloc ones): placing again with only a
// trailing non-alloc chunk resized reuses the fixed point and gives the
// placement a full run gives; an alloc chunk resized is a full run.
#[test]
fn place_again_is_place() {
    for pool in [Pool::sequential(), Pool::sequential().with_threads(4)] {
        let first = with_synthetic(world(), 0x100, 0x40);
        let sites = two_round_sites(&first);
        let prev = run(&first, &A64::M1, &sites, &pool);
        assert_eq!(prev.rounds, 3);
        for (sym, got) in [(0x100, 0x40), (0x1234, 0x40), (0x18, 0x40), (0x100, 0x48)] {
            let w = with_synthetic(world(), sym, got);
            let full = run(&w, &A64::M1, &sites, &pool);
            let again = placed_again(&w, &sites, &prev, &pool);
            assert_eq!(again.placement, full.placement, "sizes {sym:#x}, {got:#x}");
            assert_eq!(again.staged, full.staged);
            assert_eq!((again.rounds, again.rules), (full.rounds, full.rules));
        }
    }
}
