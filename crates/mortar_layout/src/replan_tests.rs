//! WP-273: the record's 9 unit tests of the re-plan (WP-271 §4), over
//! synthetic placements and words, with `mortar_a64`'s predicate, ADR fix
//! and patch reach; plus R-5 at the cap, the dirty-only scan against the
//! full one, and x86-64 (no islands: nothing changes).
#![allow(clippy::cast_possible_truncation)]

use super::*;
use mortar_a64::A64;
use mortar_a64::forms::{ADRP, insert};
use mortar_arch::need::BranchSite;
use mortar_arch::{Proof, ProofRun, Site, Status};
use mortar_core::column::Column;
use mortar_core::id::{SymKey, SymbolId};
use mortar_core::layout::{Chunk, Place, Segment, ThunkTarget};
use mortar_elf::consts::{SHF_ALLOC, SHT_PROGBITS};

const FIX: A64 = A64::new(Status::M1, true);
const AX: u64 = SHF_ALLOC | SHF_EXECINSTR;
const C: u64 = 4 << 20;
const TEXT: u64 = 0x40_0000;
const OUT: OutSecId = OutSecId(0);
const PART: Part = Part { out: OUT, part: 0 };
const E: Erratum = Erratum::E843419;

fn chunk(kind: ChunkKind, addr: u64, size: u64, capacity: u64) -> Chunk {
    Chunk {
        kind,
        sh_type: SHT_PROGBITS,
        flags: AX,
        align: 2,
        size,
        capacity,
        addr: Some(addr),
        offset: addr - TEXT + 0x1000,
        shndx: Some(1),
        relro: false,
    }
}

/// `.text`: one body chunk of `body` bytes at `TEXT` holding sections of
/// the given sizes back to back, then its island at the body's end with
/// `island` bytes used of `capacity`.
fn world(sections: &[u64], island: u64, capacity: u64) -> Placement {
    let body: u64 = sections.iter().sum();
    let chunks = vec![
        chunk(ChunkKind::Body(PART), TEXT, body, body),
        chunk(ChunkKind::Patches(PART), TEXT + body, island, capacity),
    ];
    let mut place = Vec::new();
    let mut at = 0;
    for &s in sections {
        place.push(Place::at(ChunkId(0), at));
        at += s;
    }
    let seg = Segment {
        p_type: PT_LOAD,
        flags: PF_X | 4,
        offset: 0x1000,
        addr: TEXT,
        filesz: body + capacity,
        memsz: body + capacity,
        align: 12,
    };
    Placement {
        chunks: Column::from_vec(chunks).unwrap(),
        order: vec![ChunkId(0), ChunkId(1)].into(),
        segments: Column::from_vec(vec![seg]).unwrap(),
        place: Column::from_vec(place).unwrap(),
        sized: vec![Sized { chunk: ChunkId(1), size: island }].into(),
        ..Placement::default()
    }
}

/// The pre-fix words: an `adrp x0; str x2, [x3]; ldr x1, [x0, #8]`
/// sequence at each `W1` address (`far`: its page out of ADR reach), every
/// placed section's bytes `$x`.
struct Words {
    /// The body chunk's addresses.
    body: (u64, u64),
    seqs: Vec<(u64, bool)>,
    /// Each window read, for the dirty-only test.
    asked: std::cell::RefCell<Vec<(u64, u64)>>,
}

impl WindowWords for Words {
    fn words(&self, _out: OutSecId, lo: u64, hi: u64) -> Result<Option<OwnedCode>, LayoutDiag> {
        self.asked.borrow_mut().push((lo, hi));
        let mut bytes = vec![0u8; (hi - lo) as usize];
        for &(q, far) in &self.seqs {
            let v = if far { q + 0x1000_0000 } else { q + 0x1000 };
            let w1 = insert(ADRP, 0x9000_0000, (v & !0xFFF).wrapping_sub(q & !0xFFF) as i64);
            for (k, w) in [w1, 0xf900_0062, 0xf940_0401].iter().enumerate() {
                let a = q + 4 * k as u64;
                if a >= lo && a + 4 <= hi {
                    let o = (a - lo) as usize;
                    bytes[o..o + 4].copy_from_slice(&w.to_le_bytes());
                }
            }
        }
        // The body's bytes are code; the island's are padding (no proof).
        let (bs, be) = self.body;
        let (s, e) = (bs.max(lo), be.min(hi));
        let proof = if s < e {
            vec![ProofRun {
                start: s - lo,
                end: e - lo,
                proof: Proof::Insn,
                unit: 0,
                unit_offset: 0,
            }]
        } else {
            Vec::new()
        };
        Ok(Some(OwnedCode { addr: lo, bytes, proof }))
    }
}

fn words(pl: &Placement, seqs: &[(u64, bool)]) -> Words {
    let body = pl.chunks.get(ChunkId(0)).unwrap();
    let body = (body.addr.unwrap(), body.addr.unwrap() + body.size);
    Words { body, seqs: seqs.to_vec(), asked: std::cell::RefCell::new(Vec::new()) }
}

fn run(pl: &Placement, w: &Words, dirty: &[(u64, u64)]) -> Result<Replan, Refused> {
    replan(&Input { arch: &FIX, placement: pl, dirty, words: w, branches: None, cap: C })
}

fn full(pl: &Placement, w: &Words) -> Result<Replan, Refused> {
    run(pl, w, &full_dirty(pl))
}

fn erratum(section: u32, offset: u64) -> PatchKind {
    PatchKind::Erratum { erratum: E, section: SectionId(section), offset }
}

/// A body of 0x2000 bytes in one section, the island at 0x402000 (page
/// offset 0: forbidden for a first patch) unless `pad` moves it.
fn one_page_world(pad: u64, island: u64, capacity: u64) -> Placement {
    world(&[0x2000 + pad], island, capacity)
}

// spec: WP-271 §2 step 3 (append)
#[test]
fn replan_appends_new_site() {
    let pl = one_page_world(0x40, 0, 0x100);
    // W1 at page offset 0xFF8 of the first page, F = Q + 8.
    let q = TEXT + 0xFF8;
    let w = words(&pl, &[(q, true)]);
    let r = full(&pl, &w).unwrap();
    assert_eq!(r.appended, 1);
    assert_eq!(r.patches, vec![Patch { chunk: ChunkId(1), offset: 0, kind: erratum(0, 0x1000) }]);
    assert_eq!(r.redirects, vec![Redirect { section: SectionId(0), offset: 0x1000, patch: 0 }]);
    assert_eq!(r.sized, vec![Sized { chunk: ChunkId(1), size: 8 }]);
    assert_eq!(r.judged.len(), 1);
    assert_eq!(r.judged[0].fix, Fix::Patch(TEXT + 0x2040));
    // A second new site appends after it, in the island's order (not
    // L-115's key order: an update appends).
    let mut pl2 = pl.clone();
    pl2.patches = vec![Patch { chunk: ChunkId(1), offset: 0, kind: erratum(0, 0x1004) }].into();
    pl2.sized = vec![Sized { chunk: ChunkId(1), size: 8 }].into();
    pl2.chunks = Column::from_vec(
        pl.chunks
            .iter()
            .map(|(i, c)| if i == ChunkId(1) { Chunk { size: 8, ..*c } } else { *c })
            .collect(),
    )
    .unwrap();
    let r = full(&pl2, &w).unwrap();
    assert_eq!(r.appended, 1);
    assert_eq!(r.patches[1], Patch { chunk: ChunkId(1), offset: 8, kind: erratum(0, 0x1000) });
    assert_eq!(r.redirects, vec![Redirect { section: SectionId(0), offset: 0x1000, patch: 1 }]);
    assert_eq!(r.sized, vec![Sized { chunk: ChunkId(1), size: 16 }]);
}

// spec: WP-271 §2 step 3 (reuse by key)
#[test]
fn replan_reuses_key() {
    let mut pl = one_page_world(0x40, 8, 0x100);
    pl.patches = vec![Patch { chunk: ChunkId(1), offset: 0, kind: erratum(0, 0x1000) }].into();
    let w = words(&pl, &[(TEXT + 0xFF8, true)]);
    let r = full(&pl, &w).unwrap();
    assert_eq!(r.appended, 0);
    assert_eq!(r.patches, pl.patches.to_vec());
    assert_eq!(r.redirects, vec![Redirect { section: SectionId(0), offset: 0x1000, patch: 0 }]);
    assert_eq!(r.sized, vec![Sized { chunk: ChunkId(1), size: 8 }]);
    // Its redirect already there: kept as it is.
    pl.redirects = r.redirects.clone().into();
    assert_eq!(full(&pl, &w).unwrap().redirects, r.redirects);
}

// spec: WP-271 §2 step 4 (stale redirect dropped, its patch unused)
#[test]
fn replan_drops_stale_redirect() {
    let mut pl = one_page_world(0x40, 8, 0x100);
    pl.patches = vec![Patch { chunk: ChunkId(1), offset: 0, kind: erratum(0, 0x1000) }].into();
    pl.redirects = vec![Redirect { section: SectionId(0), offset: 0x1000, patch: 0 }].into();
    // The edit moved the sequence off the page end: no site any more.
    let w = words(&pl, &[(TEXT + 0xFF0, true)]);
    let r = full(&pl, &w).unwrap();
    assert_eq!(r.dropped, 1);
    assert!(r.redirects.is_empty());
    assert_eq!(r.patches, pl.patches.to_vec(), "the patch stays, unused (L-116)");
    assert_eq!(r.sized, vec![Sized { chunk: ChunkId(1), size: 8 }]);
    // A redirect outside every dirty range is not judged: kept.
    let r = run(&pl, &w, &[(TEXT, TEXT + 0x100)]).unwrap();
    assert_eq!(r.redirects, pl.redirects.to_vec());
}

// spec: WP-271 §2 step 4 (a gone key's patch becomes a `Fill` hole; R-6)
#[test]
fn replan_hole_for_gone_key() {
    let patches = vec![
        Patch { chunk: ChunkId(1), offset: 0, kind: erratum(0, 0x1000) },
        Patch { chunk: ChunkId(1), offset: 8, kind: erratum(2, 0x10) },
        Patch { chunk: ChunkId(3), offset: 0, kind: erratum(5, 0x20) },
    ];
    let redirects = vec![
        Redirect { section: SectionId(0), offset: 0x1000, patch: 0 },
        Redirect { section: SectionId(2), offset: 0x10, patch: 1 },
        Redirect { section: SectionId(5), offset: 0x20, patch: 2 },
    ];
    let gone: BTreeSet<u32> = [1, 2].into_iter().collect();
    let size = |c: ChunkId| Some(if c == ChunkId(1) { 0x14 } else { 8 });
    let (p, r, h) = hole_gone(&patches, &redirects, &gone, &size).unwrap();
    assert_eq!(p, vec![patches[0]]);
    assert_eq!(r, vec![redirects[0]]);
    assert_eq!(
        h,
        vec![
            // To the island's end: the patch's words and the padding after it.
            Hole { chunk: ChunkId(1), offset: 8, size: 0xc, kind: HoleKind::Fill },
            Hole { chunk: ChunkId(3), offset: 0, size: 8, kind: HoleKind::Fill },
        ]
    );
    // The first gone: the later rows are renumbered.
    let gone: BTreeSet<u32> = [0].into_iter().collect();
    let (p, r, h) = hole_gone(&patches, &redirects, &gone, &size).unwrap();
    assert_eq!(p, patches[1..].to_vec());
    assert_eq!(r.iter().map(|x| x.patch).collect::<Vec<_>>(), vec![0, 1]);
    assert_eq!(h, vec![Hole { chunk: ChunkId(1), offset: 0, size: 8, kind: HoleKind::Fill }]);
    assert!(hole_gone(&patches, &redirects, &[7].into_iter().collect(), &size).is_err());
}

// spec: WP-271 §2 step 3 (L-118 padding of a first patch), as amended by
// R-1: the padding would move the island, so v1 refuses by name; a
// non-forbidden start takes the patch at offset 0.
#[test]
fn replan_first_patch_pads_empty_island() {
    // Page offset 0xFFC (a body of 0x1ffc bytes), 0x000, 0x004, 0x008.
    for body in [0x1ffc, 0x2000, 0x2004, 0x2008] {
        let pl = world(&[body], 0, 0x100);
        let w = words(&pl, &[(TEXT + 0xFF8, true)]);
        assert!(matches!(full(&pl, &w), Err(Refused::Layout(_))), "{body:#x}");
    }
    // 0x00c is allowed.
    let pl = world(&[0x200c], 0, 0x100);
    let w = words(&pl, &[(TEXT + 0xFF8, true)]);
    assert_eq!(full(&pl, &w).unwrap().patches[0].offset, 0);
    // A non-empty island was padded by its full link: it takes an append.
    let mut pl = world(&[0x2000], 8, 0x100);
    pl.patches = vec![Patch { chunk: ChunkId(1), offset: 0, kind: erratum(0, 0x4) }].into();
    let w = words(&pl, &[(TEXT + 0xFF8, true)]);
    assert_eq!(full(&pl, &w).unwrap().appended, 1);
}

// spec: WP-271 §2 step 3 (overflow); R-5: the cap C, exactly at the cap
#[test]
fn replan_island_exhausted() {
    let w0 = |pl: &Placement| words(pl, &[(TEXT + 0xFF8, true)]);
    // Capacity 4: no room for an 8-byte patch.
    let pl = one_page_world(0x40, 0, 4);
    assert!(matches!(full(&pl, &w0(&pl)), Err(Refused::IslandExhausted(_))));
    // Capacity 0 (`.fini`'s island, a nolibc `.text`'s).
    let pl = one_page_world(0x40, 0, 0);
    assert!(matches!(full(&pl, &w0(&pl)), Err(Refused::IslandExhausted(_))));
    // R-5: the end exactly at the cap is taken, one byte under it is not.
    let pl = one_page_world(0x40, 0, 0x100);
    let at = |cap| {
        replan(&Input {
            arch: &FIX,
            placement: &pl,
            dirty: &full_dirty(&pl),
            words: &w0(&pl),
            branches: None,
            cap,
        })
    };
    assert_eq!(at(8).unwrap().appended, 1);
    assert!(matches!(at(7), Err(Refused::IslandExhausted(_))));
    // At C itself: an island used up to C − 8 takes one more patch, ending
    // exactly at C; reach is tested exactly (the patch at the body's end
    // + C − 8 is within s(28) of F).
    let mut big = world(&[0x2040], C - 8, C + 0x100);
    big.patches = vec![Patch { chunk: ChunkId(1), offset: 0, kind: erratum(0, 0x4) }].into();
    let w = w0(&big);
    let r = full(&big, &w).unwrap();
    assert_eq!(r.sized, vec![Sized { chunk: ChunkId(1), size: C }]);
    let last = r.patches.last().unwrap();
    assert_eq!(last.offset, C - 8);
    let mut over = world(&[0x2040], C - 4, C + 0x100);
    over.patches = big.patches.clone();
    let w = w0(&over);
    assert!(matches!(full(&over, &w), Err(Refused::IslandExhausted(_))));
}

// spec: WP-271 §2 step 1 (a window straddling two sections: the dirty
// range holds only `W1`'s section, the site's key is the next section's)
#[test]
fn replan_window_neighbour_refilled() {
    // Section 0 ends at 0xFFC: W1 is its last word, F (Q + 8) section 1's
    // second word.
    let pl = world(&[0xFFC + 4, 0x1040], 0, 0x100);
    let q = TEXT + 0xFFC;
    let w = words(&pl, &[(q, true)]);
    let dirty = [(TEXT, TEXT + 0x1000)];
    let r = run(&pl, &w, &dirty).unwrap();
    assert_eq!(r.judged.len(), 1);
    assert_eq!((r.judged[0].section, r.judged[0].offset), (SectionId(1), 4));
    assert_eq!(r.patches[0].kind, erratum(1, 4));
    // And dirty on the other side (the neighbour's section refilled alone).
    let r = run(&pl, &w, &[(TEXT + 0x1000, TEXT + 0x1004)]).unwrap();
    assert_eq!(r.judged.len(), 1);
}

// spec: WP-271 §2 step 2 (ADR in place when `Page(V) − Q` is in s(21))
#[test]
fn replan_adr_when_in_reach() {
    let pl = one_page_world(0x40, 0, 0x100);
    let w = words(&pl, &[(TEXT + 0xFF8, false)]);
    let r = full(&pl, &w).unwrap();
    assert_eq!(r.appended, 0);
    assert!(r.patches.is_empty() && r.redirects.is_empty());
    assert_eq!(r.judged[0].fix, Fix::Adr);
    assert_eq!(r.sized, vec![Sized { chunk: ChunkId(1), size: 0 }]);
}

// spec: WP-271 ruling 3 (v1 thunks: L-113's fast path at the update's
// addresses, else `update-not-implemented (layout)`)
#[test]
fn replan_fast_path_fails_refuses() {
    let mut pl = one_page_world(0x40, 0, 0x100);
    assert!(fast_path(&pl, C).is_ok());
    let mut segs = pl.segments.as_slice().to_vec();
    segs[0].memsz = (1 << 27) - 4 - C + 1;
    pl.segments = Column::from_vec(segs.clone()).unwrap();
    let w = words(&pl, &[]);
    match full(&pl, &w) {
        Err(Refused::Layout(m)) => assert!(m.contains("L-113"), "{m}"),
        x => panic!("{x:?}"),
    }
    segs[0].memsz -= 1;
    pl.segments = Column::from_vec(segs).unwrap();
    assert!(full(&pl, &w).is_ok());
    // A branch whose target is out of reach at the update's addresses.
    let h = A64::M1.classify(283, Site { bytes: &[0; 4], offset: 0 }).unwrap();
    let site = BranchSite {
        branch: A64::M1.branch(&h).unwrap(),
        key: ThunkTarget::Sym { key: SymKey::global(SymbolId(0)), addend: 0 },
    };
    let b = [BranchAt { section: SectionId(0), offset: 0, record: 0, site }];
    let near = [Some(TEXT + 0x100)];
    let far = [Some(TEXT + (1 << 28))];
    let with = |t: &[Option<u64>]| {
        replan(&Input {
            arch: &FIX,
            placement: &pl,
            dirty: &full_dirty(&pl),
            words: &w,
            branches: Some(Branches { sites: &b, targets: t }),
            cap: C,
        })
    };
    assert!(with(&near).is_ok());
    assert!(matches!(with(&far), Err(Refused::Layout(_))));
}

// The dirty-only scan judges only the windows that meet a dirty range, and
// agrees with the full scan there (the lane's compare, `difference`).
#[test]
fn replan_dirty_only_against_full() {
    let pl = world(&[0x3040], 0, 0x100);
    let w = words(&pl, &[(TEXT + 0xFF8, true), (TEXT + 0x2FFC, true)]);
    let f = full(&pl, &w).unwrap();
    assert_eq!(f.judged.len(), 2);
    let d = run(&pl, &w, &[(TEXT + 0x2F00, TEXT + 0x3000)]).unwrap();
    assert_eq!(d.judged.len(), 1);
    assert!(difference(&d, &f).is_some(), "a missed window is a difference");
    w.asked.borrow_mut().clear();
    let d = run(&pl, &w, &[(TEXT + 0xF00, TEXT + 0x1004), (TEXT + 0x2F00, TEXT + 0x3000)]).unwrap();
    assert_eq!(difference(&d, &f), None);
    // Only the widened views were read, not the whole section.
    let asked: u64 = w.asked.borrow().iter().map(|(a, b)| b - a).sum();
    assert!(asked < 0x3040, "{asked:#x}");
}

// x86-64: no islands, so nothing changes (the replan returns the rows).
#[test]
fn replan_without_islands_changes_nothing() {
    let mut pl = world(&[0x2000], 0, 0);
    let chunks: Vec<Chunk> = pl.chunks.iter().map(|(_, c)| *c).take(1).collect();
    pl.chunks = Column::from_vec(chunks).unwrap();
    pl.sized = Box::default();
    let w = words(&pl, &[(TEXT + 0xFF8, true)]);
    let r = full(&pl, &w).unwrap();
    assert_eq!(r, Replan { holes: Vec::new(), ..Replan::default() });
    assert!(w.asked.borrow().is_empty(), "no word read");
}
