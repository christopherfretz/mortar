//! The layout of a shared object (WP-118; `spec/dyn.md` D-100, D-101,
//! D-123; `spec/layout.md` L-64, L-67, L-68, L-102, L-103).
//!
//! The options come from a `-shared` [`Plan`] through
//! `Options::from_plan` (the join's path); the synthetic chunks are those
//! a library has (no `.interp`, no copy chunks, D-122). Each case is
//! placed, checked by `Layout::check` and by the independent [G]-rule
//! checker on a skeleton of the file, and compared with D-123: no
//! `PT_PHDR`, no `PT_INTERP`, base 0, four `PT_LOAD`s R, RX, RW (RELRO),
//! RW, each with `p_align` the max page and `p_offset ≡ p_vaddr`, the
//! RELRO end on a max-page boundary (4 KiB and AArch64's 64 KiB default),
//! `.got.plt` in RELRO exactly under `-z now` (D-120).
#![allow(clippy::cast_possible_truncation, clippy::too_many_lines)]

mod common;

use common::elf::{check_g, read_elf, write_skeleton};
use common::{Sec, World, sec};
use mortar_core::id::{ChunkId, SymbolId};
use mortar_core::layout::{ChunkKind, Synth};
use mortar_core::par::Pool;
use mortar_core::plan::{OutputKind, Plan};
use mortar_core::resolution::{Def, InternalDef};
use mortar_elf::Machine;
use mortar_elf::consts::{
    PF_R, PF_W, PF_X, PT_DYNAMIC, PT_GNU_RELRO, PT_GNU_STACK, PT_INTERP, PT_LOAD, PT_PHDR,
    SHF_ALLOC, SHF_EXECINSTR, SHF_INFO_LINK, SHF_WRITE, SHT_DYNAMIC, SHT_DYNSYM, SHT_GNU_HASH,
    SHT_GNU_VERNEED, SHT_GNU_VERSYM, SHT_INIT_ARRAY, SHT_NOBITS, SHT_PROGBITS, SHT_RELA,
    SHT_STRTAB,
};
use mortar_layout::chunk::SynthChunk;
use mortar_layout::{Options, Placed};

const A: u64 = SHF_ALLOC;
const AX: u64 = SHF_ALLOC | SHF_EXECINSTR;
const WA: u64 = SHF_ALLOC | SHF_WRITE;
const P: u32 = SHT_PROGBITS;

/// A library's sections: code, read-only data, a constructor, RELRO data,
/// data and bss.
fn files() -> Vec<Vec<Sec>> {
    vec![vec![
        sec(b".text", P, AX, 0x1_2340, 4),
        sec(b".rodata", P, A, 0x2021, 3),
        sec(b".init_array", SHT_INIT_ARRAY, WA, 8, 3),
        sec(b".data.rel.ro", P, WA, 0x1238, 4),
        sec(b".data", P, WA, 0x3456, 5),
        sec(b".bss", SHT_NOBITS, WA, 0x1_0000, 6),
    ]]
}

fn synth(name: &'static [u8], sh_type: u32, flags: u64, align: u8, size: u64) -> SynthChunk {
    SynthChunk { synth: Synth { name, address_dependent: false }, sh_type, flags, align, size }
}

/// The keys: 0 `_DYNAMIC`, 1 `_start`, 2 `f` (all undefined but the
/// first; a case defines what it needs).
const START: u32 = 1;
const F: u32 = 2;

/// A `-shared` line with `-z max-page-size`, `-z now`, `-z norelro` and
/// `-e` as given.
fn plan(max_page: Option<u64>, now: bool, relro: bool, entry: Option<&str>) -> Plan {
    let mut p = Plan { kind: OutputKind::Shared, ..Plan::default() };
    p.z.max_page_size = max_page.map(|m| format!("{m:#x}").into());
    p.z.now = Some(now);
    p.z.relro = Some(relro);
    p.entry = entry.map(Into::into);
    p
}

fn world(plan: &Plan) -> World {
    let mut w = World::new(
        Machine::AArch64,
        &files(),
        &[
            (b"_DYNAMIC", Def::Internal(InternalDef::Provide { conditional: false })),
            (b"_start", Def::Undefined { weak: false }),
            (b"f", Def::Undefined { weak: false }),
        ],
    );
    w.options = Options::from_plan(plan, &w.target).unwrap();
    // D-123, D-60: a library's dynamic chunks; no `.interp` (D-100), no
    // `.dynbss`/`.bss.rel.ro` (no copies, D-122).
    w.synthetic.extend([
        synth(b".gnu.hash", SHT_GNU_HASH, A, 3, 0x2c),
        synth(b".dynsym", SHT_DYNSYM, A, 3, 0xd8),
        synth(b".dynstr", SHT_STRTAB, A, 0, 0x6f),
        synth(b".gnu.version", SHT_GNU_VERSYM, A, 1, 0x12),
        synth(b".gnu.version_r", SHT_GNU_VERNEED, A, 3, 0x30),
        synth(b".rela.dyn", SHT_RELA, A, 3, 0x78),
        synth(b".rela.plt", SHT_RELA, A | SHF_INFO_LINK, 3, 0x48),
        synth(b".plt", P, AX, 4, 0x50),
        synth(b".got", P, WA, 3, 0x28),
        synth(b".got.plt", P, WA, 3, 0x30),
        synth(b".dynamic", SHT_DYNAMIC, WA, 3, 0x1b0),
    ]);
    w
}

fn chunk_named(
    a: &mortar_core::assignment::Assignment<'_>,
    placed: &Placed,
    name: &[u8],
) -> ChunkId {
    let p = &placed.placement;
    *p.order.iter().find(|&&c| p.chunks[c].name(a) == Some(name)).unwrap()
}

/// The allocated chunks in address order, by name.
fn names(a: &mortar_core::assignment::Assignment<'_>, placed: &Placed) -> Vec<String> {
    let p = &placed.placement;
    p.order
        .iter()
        .map(|&c| &p.chunks[c])
        .filter(|c| c.addr.is_some() && !matches!(c.kind, ChunkKind::Patches(_)))
        .map(|c| match c.kind {
            ChunkKind::FileHeader => "ehdr".to_owned(),
            ChunkKind::ProgramHeaders => "phdr".to_owned(),
            _ => String::from_utf8_lossy(c.name(a).unwrap()).into_owned(),
        })
        .collect()
}

// spec: dyn.md D-100 (no PT_INTERP, no PT_PHDR), D-123 (segments, RELRO
// end, p_align, congruence), D-120 (.got.plt outside RELRO when lazy),
// layout.md L-68 (base 0)
#[test]
#[allow(clippy::many_single_char_names)] // the world, assignment, placement and segments of one case
fn d123_shared_segments() {
    let mut cases = 0;
    for max_page in [None, Some(0x1000), Some(0x1_0000)] {
        for now in [false, true] {
            for relro in [true, false] {
                let what = format!("max-page {max_page:x?} now {now} relro {relro}");
                let w = world(&plan(max_page, now, relro, None));
                // AArch64's default max page is 64 KiB (§F.9).
                let page = max_page.unwrap_or(0x1_0000);
                assert_eq!(w.options.max_page, page, "{what}");
                assert!(w.options.pie, "{what}");
                let (a, placed) = w.run(&Pool::sequential()).unwrap();
                w.check(&a, &placed).unwrap_or_else(|v| panic!("{what}: {v:?}"));
                assert!(placed.warnings.is_empty(), "{what}: {:?}", placed.warnings);
                let p = &placed.placement;

                // D-60's order without `.interp`.
                let mut want = vec![
                    "ehdr",
                    "phdr",
                    ".gnu.hash",
                    ".dynsym",
                    ".dynstr",
                    ".gnu.version",
                    ".gnu.version_r",
                    ".rela.dyn",
                    ".rela.plt",
                    ".rodata",
                    ".plt",
                    ".text",
                    ".init_array",
                    ".data.rel.ro",
                    ".dynamic",
                    ".got",
                ];
                if now {
                    want.push(".got.plt");
                }
                if relro {
                    want.push(".pad.relro");
                }
                if !now {
                    want.push(".got.plt");
                }
                want.extend([".data", ".bss"]);
                assert_eq!(names(&a, &placed), want, "{what}");

                // D-100, D-61: no PT_PHDR, no PT_INTERP.
                let types: Vec<u32> = p.segments.iter().map(|(_, s)| s.p_type).collect();
                assert!(!types.contains(&PT_PHDR) && !types.contains(&PT_INTERP), "{what}");
                let mut want = vec![PT_LOAD; if relro { 4 } else { 3 }];
                want.extend([PT_DYNAMIC, PT_GNU_STACK]);
                if relro {
                    want.push(PT_GNU_RELRO);
                }
                assert_eq!(types, want, "{what}");

                // D-123: R, RX, RW (RELRO), RW; base 0 (L-68); p_align the
                // max page; p_offset ≡ p_vaddr.
                let loads: Vec<_> =
                    p.segments.iter().map(|(_, s)| *s).filter(|s| s.p_type == PT_LOAD).collect();
                let flags: Vec<u32> = loads.iter().map(|s| s.flags).collect();
                let mut want = vec![PF_R, PF_R | PF_X, PF_R | PF_W];
                if relro {
                    want.push(PF_R | PF_W);
                }
                assert_eq!(flags, want, "{what}");
                assert_eq!((loads[0].addr, loads[0].offset), (0, 0), "{what}");
                for s in &loads {
                    let al = 1u64 << s.align;
                    assert_eq!(al, page, "{what}: p_align");
                    assert_eq!(s.offset % al, s.addr % al, "{what}: p_offset ≡ p_vaddr");
                }

                // D-123, L-64: the RELRO run is the third PT_LOAD's start
                // and ends on a max-page boundary; `.got.plt` is in it
                // exactly under `-z now` (D-120).
                if relro {
                    let r = p.segments.iter().map(|(_, s)| *s).find(|s| s.p_type == PT_GNU_RELRO);
                    let r = r.unwrap();
                    assert_eq!(r.addr, loads[2].addr, "{what}");
                    assert_eq!((r.addr + r.memsz) % page, 0, "{what}: RELRO end");
                    assert!(loads[3].addr >= r.addr + r.memsz, "{what}: RW after RELRO");
                    let got_plt = &p.chunks[chunk_named(&a, &placed, b".got.plt")];
                    assert_eq!(got_plt.relro, now, "{what}");
                    for n in [&b".dynamic"[..], b".got", b".data.rel.ro"] {
                        let c = &p.chunks[chunk_named(&a, &placed, n)];
                        let at = c.addr.unwrap();
                        assert!(at >= r.addr && at + c.size <= r.addr + r.memsz, "{what}");
                    }
                }

                // The [G] rules, from the file alone.
                let bytes = write_skeleton(p, &a, Machine::AArch64, true);
                let g = check_g(&read_elf(&bytes));
                assert!(g.is_empty(), "{what}: {g:?}");
                cases += 1;
            }
        }
    }
    assert_eq!(cases, 12);
}

/// WP-118's "measure first": the RELRO end of a library at AArch64's
/// default 64 KiB max page, in the same shape as the references' (record
/// §F.9: GNU ld 0x20000, mold 0x30000, lld 0x21000 for a small library).
/// mortar's end is the next 64 KiB boundary after the RELRO chunks, the
/// RW segment starts there, and the file is not padded to it (D-123: the
/// RELRO padding is NOBITS-like, L-64).
// spec: dyn.md D-123 (RELRO end on the max page), §F.9
#[test]
fn d123_relro_end_at_64k() {
    let w = world(&plan(None, false, true, None));
    let (a, placed) = w.run(&Pool::sequential()).unwrap();
    w.check(&a, &placed).unwrap();
    let p = &placed.placement;
    let seg = |t| p.segments.iter().map(|(_, s)| *s).find(|s| s.p_type == t).unwrap();
    let r = seg(PT_GNU_RELRO);
    let end = r.addr + r.memsz;
    assert_eq!(end % 0x1_0000, 0);
    // The end is the least boundary at or after the last RELRO chunk.
    let got = &p.chunks[chunk_named(&a, &placed, b".got")];
    let last = got.addr.unwrap() + got.size;
    assert_eq!(end, last.next_multiple_of(0x1_0000));
    // The RELRO run's file bytes end with its last chunk in the file, not
    // at the boundary.
    assert_eq!(r.offset + r.filesz, got.offset + got.size);
    eprintln!(
        "d123_relro_end_at_64k: RELRO {:#x}..{end:#x} (file {:#x}..{:#x})",
        r.addr,
        r.offset,
        r.offset + r.filesz
    );
}

/// The entry of a shared object (D-101): `_start` when defined, else 0
/// silently; `-e f`: `f`'s address, or `entry-not-found` and 0.
// spec: dyn.md D-101, layout.md L-102, L-103 (as D-101 replaces its last
// sentence)
#[test]
fn d101_shared_entry() {
    let pool = Pool::sequential();
    let entry = |w: &World| {
        let (a, placed) = w.run(&pool).unwrap();
        w.check(&a, &placed).unwrap();
        let text = placed.placement.place[w.sid(1, 1)].get().unwrap();
        let text = placed.placement.chunks[text.0].addr.unwrap() + text.1;
        let warnings: Vec<&str> =
            placed.warnings.iter().map(mortar_layout::LayoutDiag::id).collect();
        (placed.placement.entry.map(|e| e - text), warnings.join(","))
    };
    // No `_start`: 0, nothing reported.
    let w = world(&plan(None, false, true, None));
    assert_eq!(entry(&w), (None, String::new()));
    // `_start` defined: its address.
    let mut w = world(&plan(None, false, true, None));
    w.define(START, 1, 1, 0x10);
    assert_eq!(entry(&w), (Some(0x10), String::new()));
    // `-e f`, `f` defined: its address (not `_start`'s).
    let mut w = world(&plan(None, false, true, Some("f")));
    w.define(START, 1, 1, 0x10);
    w.define(F, 1, 1, 0x20);
    assert_eq!(entry(&w), (Some(0x20), String::new()));
    // `-e f`, `f` undefined: L-103's warning, 0.
    let w = world(&plan(None, false, true, Some("f")));
    assert_eq!(entry(&w), (None, "entry-not-found".to_owned()));
    assert_eq!(SymbolId(START).0, 1);
}
