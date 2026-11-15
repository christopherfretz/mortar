//! Stage E, the fixed point (`spec/layout.md` §14, `spec/arch.md` §6),
//! over a synthetic architecture: branch sites with a reach, 12-byte
//! thunks, and an erratum site list.
#![allow(clippy::many_single_char_names, clippy::cast_possible_truncation)]

mod common;

use common::{World, sec};
use mortar_core::id::{SectionId, SymKey, SymbolId};
use mortar_core::layout::{ChunkKind, Erratum, PatchKind, ThunkTarget};
use mortar_core::par::{Pool, check_schedules};
use mortar_core::resolution::Def;
use mortar_elf::Machine;
use mortar_elf::consts::{SHF_ALLOC, SHF_EXECINSTR, SHT_PROGBITS};
use mortar_layout::fixed::{Need, NeedKind, R_MAX, RangeArch, Round};

const AX: u64 = SHF_ALLOC | SHF_EXECINSTR;
const MIB: u64 = 1 << 20;
/// The synthetic reach: B/BL's, `±2^27`, less 4 (a64.md §9).
const REACH: u64 = (1 << 27) - 4;

/// A branch site: `(section, offset)` to `target`'s symbol, which is
/// defined at `(section, offset)`.
#[derive(Debug, Clone, Copy)]
struct Site {
    section: SectionId,
    offset: u64,
    record: u32,
    target: SymbolId,
    at: (SectionId, u64),
}

#[derive(Debug, Clone, Default)]
struct Synthetic {
    sites: Vec<Site>,
    errata: Vec<(SectionId, u64)>,
    /// Every round asks for a thunk with a new addend (never converges).
    oscillate: bool,
    /// The thunk size.
    thunk: u64,
}

fn reach(a: u64, b: u64) -> bool {
    a.abs_diff(b) <= REACH
}

impl RangeArch for Synthetic {
    fn needs(&self, round: &Round<'_>, pool: &Pool) -> Vec<Need> {
        if self.oscillate {
            let s = self.sites[0];
            return vec![Need {
                addr: round.site_addr(s.section, s.offset).unwrap(),
                section: s.section,
                offset: s.offset,
                record: 0,
                kind: NeedKind::Thunk(ThunkTarget::Sym {
                    key: SymKey::global(s.target),
                    addend: i64::from(round.number),
                }),
            }];
        }
        let mut out: Vec<Need> = pool
            .map(self.sites.len(), |i| {
                let s = self.sites[i];
                let p = round.site_addr(s.section, s.offset)?;
                let far = match round.redirect(s.section, s.offset) {
                    Some((i, _)) => !reach(p, round.patch_addr(i)?),
                    None => !reach(p, round.site_addr(s.at.0, s.at.1)?),
                };
                far.then_some(Need {
                    addr: p,
                    section: s.section,
                    offset: s.offset,
                    record: s.record,
                    kind: NeedKind::Thunk(ThunkTarget::Sym {
                        key: SymKey::global(s.target),
                        addend: 0,
                    }),
                })
            })
            .into_iter()
            .flatten()
            .collect();
        for &(section, offset) in &self.errata {
            if round.redirect(section, offset).is_none() {
                out.push(Need {
                    addr: round.site_addr(section, offset).unwrap(),
                    section,
                    offset,
                    record: 0,
                    kind: NeedKind::Erratum(Erratum::E843419),
                });
            }
        }
        // The driver sorts: give them reversed.
        out.reverse();
        out
    }

    fn patch_shape(&self, kind: PatchKind) -> (u64, u8) {
        match kind {
            PatchKind::Thunk(_) => (self.thunk, 2),
            PatchKind::Erratum { .. } => (8, 2),
        }
    }

    fn reaches(&self, need: &Need, patch: u64) -> bool {
        reach(need.addr, patch)
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
    let mut w = World::new(
        Machine::AArch64,
        &files,
        &[
            (b"_start", Def::Undefined { weak: false }),
            (b"far", Def::Undefined { weak: false }),
            (b"near", Def::Undefined { weak: false }),
        ],
    );
    w.define(0, 1, 1, 0);
    w.define(1, 3, 1, 16);
    w
}

/// Site X (.text.a+0 → far in .text.c) needs a thunk in round 1; site Y
/// (the last word of .text.a → near, just in reach across island 0) goes
/// out of reach when island 0 grows: round 2 adds its thunk; round 3 has
/// no need.
fn two_round_arch(w: &World) -> Synthetic {
    let a = w.sid(1, 1);
    let b = w.sid(2, 1);
    let c = w.sid(3, 1);
    // Y at a+100M-4, near at b+off: distance 4 + off = REACH - 4.
    let near_off = REACH - 8;
    Synthetic {
        sites: vec![
            Site { section: a, offset: 0, record: 0, target: SymbolId(1), at: (c, 16) },
            Site {
                section: a,
                offset: 100 * MIB - 4,
                record: 1,
                target: SymbolId(2),
                at: (b, near_off),
            },
        ],
        errata: Vec::new(),
        oscillate: false,
        thunk: 12,
    }
}

// spec: layout.md §14.1 L-110 (parts by size), L-111 (an island per part)
#[test]
fn l110_l111_parts_and_islands() {
    let w = world();
    let (_, placed) = w.run(&Pool::sequential()).unwrap();
    w.check(&placed_assignment(&w), &placed).unwrap();
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

fn placed_assignment(w: &World) -> mortar_core::assignment::Assignment<'static> {
    w.run(&Pool::sequential()).unwrap().0
}

// spec: arch.md §6 (rounds, monotone, 2N+1), layout.md L-111 (new thunks in the site's island), L-115
#[test]
fn fixed_point_two_rounds() {
    let w = world();
    let arch = two_round_arch(&w);
    let (a, placed) = w.run_with(&arch, &Pool::sequential()).unwrap();
    w.check(&a, &placed).unwrap();
    let p = &placed.placement;
    assert_eq!(placed.rounds, 3, "two rounds that add, one that stops");
    assert_eq!(p.patches.len(), 2);
    assert!(placed.rounds <= 2 * p.patches.len() as u32 + 1);
    // Both thunks are in island 0 (the sites' part), in key order: far
    // (SymbolId 1) before near (SymbolId 2).
    let island0 = p.patches[0].chunk;
    assert!(p.patches.iter().all(|x| x.chunk == island0));
    assert_eq!(p.chunks[island0].kind.part().unwrap().part, 0);
    let keys: Vec<_> = p.patches.iter().map(|x| x.kind).collect();
    assert_eq!(
        keys,
        [
            PatchKind::Thunk(ThunkTarget::Sym { key: SymKey::global(SymbolId(1)), addend: 0 }),
            PatchKind::Thunk(ThunkTarget::Sym { key: SymKey::global(SymbolId(2)), addend: 0 }),
        ]
    );
    assert_eq!((p.patches[0].offset, p.patches[1].offset), (0, 12));
    assert_eq!(p.redirects.len(), 2);
    // The fixed point: the arch has no need at the final addresses.
    let round = Round {
        number: 0,
        chunks: &p.chunks,
        place: &p.place,
        patches: &p.patches,
        redirects: &p.redirects,
    };
    assert!(arch.needs(&round, &Pool::sequential()).is_empty());
}

// spec: arch.md §6 "Determinism" (needs collected in parallel; one result)
#[test]
fn fixed_point_schedules() {
    let w = world();
    let arch = two_round_arch(&w);
    check_schedules(|pool| {
        let (_, placed) = w.run_with(&arch, pool).unwrap();
        (placed.placement, placed.rounds)
    });
}

// spec: arch.md §6 "Addresses are a function of the patch set" (L-21): adding
// the same patches in one round gives the same layout as in two.
#[test]
fn l21_function_of_patch_set() {
    let w = world();
    let two = two_round_arch(&w);
    let (_, by_two) = w.run_with(&two, &Pool::sequential()).unwrap();
    // One round: Y's target is far enough to need its thunk at once.
    let mut one = two.clone();
    one.sites[1].at = (w.sid(3, 1), 100 * MIB - 4);
    let (_, by_one) = w.run_with(&one, &Pool::sequential()).unwrap();
    assert_eq!(by_one.rounds, 2);
    let strip = |p: &mortar_layout::Placed| {
        (p.placement.chunks.clone(), p.placement.segments.clone(), p.placement.patches.len())
    };
    assert_eq!(strip(&by_one), strip(&by_two));
}

// spec: arch.md §6 item 6 (more than R_max = 32 rounds is refused by name)
#[test]
fn cap_refusal_oscillating() {
    let w = world();
    let mut arch = two_round_arch(&w);
    arch.oscillate = true;
    let e = w.run_with(&arch, &Pool::sequential()).unwrap_err();
    assert_eq!(e[0].refusal().map(|r| r.1), Some("range-extension fixed point"));
    assert_eq!(R_MAX, 32);
}

// spec: layout.md §14.1 L-112 (an island over C = 4 MiB is refused like the cap)
#[test]
fn l112_island_cap() {
    let w = world();
    let mut arch = two_round_arch(&w);
    arch.thunk = (1 << 22) + 4;
    let e = w.run_with(&arch, &Pool::sequential()).unwrap_err();
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

// spec: layout.md §14.2 L-115 (thunks, then errata by site), L-117 (an erratum patch in its site's island)
#[test]
fn l115_errata_after_thunks() {
    let w = world();
    let mut arch = two_round_arch(&w);
    let b = w.sid(2, 1);
    arch.errata = vec![(b, 0x1ffc), (w.sid(1, 1), 0xffc), (b, 0xffc)];
    let (a, placed) = w.run_with(&arch, &Pool::sequential()).unwrap();
    w.check(&a, &placed).unwrap();
    let p = &placed.placement;
    let rows: Vec<(u32, PatchKind)> =
        p.patches.iter().map(|x| (p.chunks[x.chunk].kind.part().unwrap().part, x.kind)).collect();
    let thunk =
        |s| PatchKind::Thunk(ThunkTarget::Sym { key: SymKey::global(SymbolId(s)), addend: 0 });
    let erratum =
        |section, offset| PatchKind::Erratum { erratum: Erratum::E843419, section, offset };
    assert_eq!(
        rows,
        [
            (0, thunk(1)),
            (0, thunk(2)),
            (0, erratum(w.sid(1, 1), 0xffc)),
            (1, erratum(b, 0xffc)),
            (1, erratum(b, 0x1ffc)),
        ]
    );
}

// spec: layout.md §14.2 L-118 (843419 start padding of a non-empty island, at most 16 bytes)
#[test]
fn l118_island_padding() {
    let island0 = |w: &World| {
        let arch = two_round_arch(w);
        let (a, placed) = w.run_with(&arch, &Pool::sequential()).unwrap();
        w.check(&a, &placed).unwrap();
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
