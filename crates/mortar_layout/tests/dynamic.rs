//! The layout of a dynamic executable (WP-049, `spec/dyn.md` D-1, D-42,
//! D-60, D-61; `spec/layout.md` L-60, L-64, L-67, L-68).
//!
//! The oracle is the spec's table, alone: a fixture link with every
//! dynamic table as a synthetic chunk and three copy allocations is placed
//! for each output kind (`Exec`, `Pie`) x `-z now` x `-z norelro` x
//! machine, and the section order, the program headers (order, flags,
//! extents), the base, the congruences, the copy offsets and `_DYNAMIC`
//! are compared with D-60/D-61; `Layout::check` (with the copy slots) and
//! the independent [G]-rule checker (`common::elf::check_g`) run on each.
#![allow(clippy::cast_possible_truncation, clippy::too_many_lines)]

mod common;

use common::elf::{check_g, read_elf, write_skeleton};
use common::{Sec, World, sec};
use mortar_core::assignment::Assignment;
use mortar_core::check::{CheckContext, Violation};
use mortar_core::column::Column;
use mortar_core::demands::{DemandFlags, Demands};
use mortar_core::id::{ChunkId, FileId, Span, SymEntry, SymbolId};
use mortar_core::layout::{
    AllocKey, ChunkKind, CopyAlloc, CopyPlace, DefValue, Layout, Slots, Synth,
};
use mortar_core::par::Pool;
use mortar_core::resolution::{Def, InternalDef, RefFlags};
use mortar_elf::consts::{
    PF_R, PF_W, PF_X, PT_DYNAMIC, PT_GNU_RELRO, PT_GNU_STACK, PT_INTERP, PT_LOAD, PT_NOTE,
    PT_PHDR, PT_TLS, SHF_ALLOC, SHF_EXECINSTR, SHF_INFO_LINK, SHF_TLS, SHF_WRITE, SHT_DYNAMIC,
    SHT_DYNSYM, SHT_GNU_HASH, SHT_GNU_VERNEED, SHT_GNU_VERSYM, SHT_INIT_ARRAY, SHT_NOBITS,
    SHT_NOTE, SHT_PROGBITS, SHT_RELA, SHT_STRTAB,
};
use mortar_elf::{Machine, SymbolIndex};
use mortar_layout::chunk::{CopyRow, SynthChunk};
use mortar_layout::headers::HeaderShapes;
use mortar_layout::{Placed, place_with_copies};

const A: u64 = SHF_ALLOC;
const AX: u64 = SHF_ALLOC | SHF_EXECINSTR;
const WA: u64 = SHF_ALLOC | SHF_WRITE;
const P: u32 = SHT_PROGBITS;
const SHARED: u32 = 3;

fn files() -> Vec<Vec<Sec>> {
    vec![
        vec![
            sec(b".text", P, AX, 0x40, 3),
            sec(b".data", P, WA, 0x10, 3),
            sec(b".bss", SHT_NOBITS, WA, 0x20, 4),
            sec(b".note.ABI-tag", SHT_NOTE, A, 0x20, 2),
        ],
        vec![
            sec(b".text", P, AX, 0x1_2340, 4),
            sec(b".rodata", P, A, 0x2021, 3),
            sec(b".init_array", SHT_INIT_ARRAY, WA, 8, 3),
            sec(b".data.rel.ro", P, WA, 0x1238, 4),
            sec(b".tdata", P, WA | SHF_TLS, 8, 3),
            sec(b".data", P, WA, 0x3456, 5),
            sec(b".bss", SHT_NOBITS, WA, 0x1_0000, 6),
        ],
        // The shared object the copies come from (no sections).
        vec![],
    ]
}

fn synth(name: &'static [u8], sh_type: u32, flags: u64, align: u8, size: u64) -> SynthChunk {
    SynthChunk {
        synth: Synth { name, address_dependent: false },
        sh_type,
        flags,
        align,
        size,
    }
}

/// The copy allocations: (index, place, size, log2 align, least alias).
/// `.dynbss` holds 0 (aliases 1, 2) then 2 (alias 4): offsets 0 and 8,
/// size 0xc, alignment 8; `.bss.rel.ro` holds 1 (alias 3) at 0, size 0x18.
fn copy_rows() -> Vec<CopyRow> {
    vec![
        CopyRow { index: 0, place: CopyPlace::DynBss, size: 8, align: 3, first: SymbolId(1) },
        CopyRow { index: 1, place: CopyPlace::RelRo, size: 0x18, align: 3, first: SymbolId(3) },
        CopyRow { index: 2, place: CopyPlace::DynBss, size: 4, align: 2, first: SymbolId(4) },
    ]
}

fn copy_slots() -> Slots {
    let at = |value, align, alias, place| CopyAlloc {
        file: FileId(SHARED),
        value,
        place,
        align,
        alias: SymbolId(alias),
    };
    Slots {
        copies: Box::new([
            at(0x100, 8, 1, CopyPlace::DynBss),
            at(0x200, 8, 3, CopyPlace::RelRo),
            at(0x300, 4, 4, CopyPlace::DynBss),
        ]),
        copy_of: Box::new([(SymbolId(1), 0), (SymbolId(2), 0), (SymbolId(3), 1), (SymbolId(4), 2)]),
        dynsym: Box::new([SymbolId(1), SymbolId(2), SymbolId(3), SymbolId(4)]),
        ..Slots::default()
    }
}

#[derive(Debug, Clone, Copy)]
struct Kind {
    pie: bool,
    now: bool,
    relro: bool,
    interp: bool,
}

fn world(machine: Machine, k: Kind) -> World {
    let shared = |i| Def::Shared(SymEntry { file: FileId(SHARED), index: SymbolIndex(i) });
    let mut w = World::new(
        machine,
        &files(),
        &[
            (b"_DYNAMIC", Def::Internal(InternalDef::Provide { conditional: false })),
            (b"environ", shared(1)),
            (b"__environ", shared(2)),
            (b"_ZTIi", shared(3)),
            (b"stdout", shared(4)),
        ],
    );
    for s in 1..=4 {
        w.resolution.props[SymbolId(s)].refs = RefFlags::PREEMPTIBLE;
    }
    w.options.pie = k.pie;
    w.options.now = k.now;
    w.options.relro = k.relro;
    if k.interp {
        w.synthetic.push(synth(b".interp", P, A, 0, 0x1b));
    }
    w.synthetic.extend([
        synth(b".gnu.hash", SHT_GNU_HASH, A, 3, 0x1c),
        synth(b".dynsym", SHT_DYNSYM, A, 3, 0x78),
        synth(b".dynstr", SHT_STRTAB, A, 0, 0x5d),
        synth(b".gnu.version", SHT_GNU_VERSYM, A, 1, 0xa),
        synth(b".gnu.version_r", SHT_GNU_VERNEED, A, 3, 0x30),
        synth(b".rela.dyn", SHT_RELA, A, 3, 0x60),
        synth(b".rela.plt", SHT_RELA, A | SHF_INFO_LINK, 3, 0x48),
        synth(b".plt", P, AX, 4, 0x50),
        synth(b".got", P, WA, 3, 0x20),
        synth(b".got.plt", P, WA, 3, 0x30),
        synth(b".dynamic", SHT_DYNAMIC, WA, 3, 0x1d0),
        synth(b".dynbss", SHT_NOBITS, WA, 0, 0xc),
        synth(b".bss.rel.ro", SHT_NOBITS, WA, 0, 0x18),
    ]);
    w
}

fn place(w: &World) -> (Assignment<'static>, Placed) {
    let pool = Pool::sequential();
    let mut a = w.stage_a(&pool).unwrap();
    let entries = w.entries.clone();
    let placed = place_with_copies(
        &w.link(),
        &copy_rows(),
        &mut a,
        None,
        move |e| entries.iter().find(|(x, _)| *x == e).map(|(_, d)| *d),
        &pool,
        None,
    )
    .unwrap();
    (a, placed)
}

/// `Layout::check` with the copy slots and demands.
fn check(w: &World, a: &Assignment<'static>, placed: &Placed) -> Result<(), Violation> {
    let layout = Layout {
        folds: w.folds.clone(),
        merge: w.merge.clone(),
        slots: Slots {
            symtab_locals: Column::filled(w.resolution.file_live.len(), Span::default()),
            ..copy_slots()
        },
        placement: placed.placement.clone(),
    };
    let mut demands = Demands::empty(w.symbols.len());
    for s in 1..=4 {
        demands.global[SymbolId(s)] = DemandFlags::COPY;
    }
    let shapes = HeaderShapes(&w.headers);
    let cx = CheckContext {
        class: w.target.class,
        page_size: w.options.max_page,
        needs_entry: placed.placement.entry.is_some(),
        segment_rules: placed.rules,
        resolution: &w.resolution,
        assignment: a,
        split: &w.split,
        liveness: &w.liveness,
        demands: &demands,
        shapes: &shapes,
    };
    layout.validate(&cx)
}

/// The allocated chunks in address order, by name (islands left out).
fn names(a: &Assignment<'_>, placed: &Placed) -> Vec<String> {
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

/// D-60's order for `k`.
fn expected(k: Kind) -> Vec<&'static str> {
    let mut v = vec!["ehdr", "phdr"];
    if k.interp {
        v.push(".interp");
    }
    v.extend([
        ".note.ABI-tag",
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
        ".tdata",
        ".init_array",
        ".data.rel.ro",
        ".dynamic",
        ".got",
    ]);
    if k.now {
        v.push(".got.plt");
    }
    if k.relro {
        v.extend([".bss.rel.ro", ".relro_padding"]);
    }
    if !k.now {
        v.push(".got.plt");
    }
    v.extend([".data", ".dynbss", ".bss"]);
    if !k.relro {
        // L-69: no PROGBITS after NOBITS in one PT_LOAD; lld's place.
        v.push(".bss.rel.ro");
    }
    v
}

fn chunk_named(placed: &Placed, name: &[u8]) -> ChunkId {
    let p = &placed.placement;
    *p.order
        .iter()
        .find(|&&c| matches!(p.chunks[c].kind, ChunkKind::Synthetic(s) if s.name == name))
        .unwrap()
}

fn kinds() -> Vec<Kind> {
    let mut out = Vec::new();
    for pie in [false, true] {
        for now in [false, true] {
            for relro in [true, false] {
                out.push(Kind { pie, now, relro, interp: true });
            }
        }
    }
    // An interpreter-less position-independent layout (static-pie's
    // shape): no PT_PHDR, no PT_INTERP (D-61).
    out.push(Kind { pie: true, now: true, relro: true, interp: false });
    out
}

// spec: dyn.md D-60 (section order), D-61 (segments), D-1/L-68 (base),
// D-42 (copy chunks), L-64 (RELRO end), §13.1 `_DYNAMIC`
#[test]
fn d60_d61_table() {
    let mut cases = 0;
    for machine in [Machine::AArch64, Machine::X86_64] {
        for k in kinds() {
            let what = format!("{machine:?} {k:?}");
            let w = world(machine, k);
            let (a, placed) = place(&w);
            check(&w, &a, &placed).unwrap_or_else(|v| panic!("{what}: {v:?}"));
            let p = &placed.placement;
            assert_eq!(names(&a, &placed), expected(k), "{what}");

            // D-61: the program headers in order.
            let types: Vec<u32> = p.segments.iter().map(|(_, s)| s.p_type).collect();
            let mut want = Vec::new();
            if k.interp {
                want.extend([PT_PHDR, PT_INTERP]);
            }
            want.extend([PT_LOAD; 3]);
            if k.relro {
                want.push(PT_LOAD);
            }
            want.extend([PT_DYNAMIC, PT_NOTE, PT_TLS, PT_GNU_STACK]);
            if k.relro {
                want.push(PT_GNU_RELRO);
            }
            assert_eq!(types, want, "{what}");
            let loads: Vec<_> =
                p.segments.iter().map(|(_, s)| *s).filter(|s| s.p_type == PT_LOAD).collect();
            let flags: Vec<u32> = loads.iter().map(|s| s.flags).collect();
            let mut want = vec![PF_R, PF_R | PF_X, PF_R | PF_W];
            if k.relro {
                want.push(PF_R | PF_W);
            }
            assert_eq!(flags, want, "{what}");

            // D-1/L-68: base 0 exactly for a PIE.
            assert_eq!(loads[0].addr == 0, k.pie, "{what}");
            assert_eq!(loads[0].offset, 0, "{what}");
            for s in &loads {
                // Segment alignment is log2.
                let al = 1u64 << s.align;
                assert_eq!(s.offset % al, s.addr % al, "{what}: p_offset = p_vaddr");
                assert_eq!(al, w.options.max_page, "{what}");
            }

            // PT_PHDR, PT_INTERP, PT_DYNAMIC: their chunks exactly.
            let seg = |t| p.segments.iter().map(|(_, s)| *s).find(|s| s.p_type == t);
            let covers = |t, c: ChunkId, flags| {
                let s = seg(t).unwrap();
                let ch = &p.chunks[c];
                assert_eq!(
                    (s.offset, s.addr, s.filesz, s.memsz, s.flags),
                    (ch.offset, ch.addr.unwrap(), ch.size, ch.size, flags),
                    "{what}: segment {t}"
                );
            };
            let phdr = *p
                .order
                .iter()
                .find(|&&c| matches!(p.chunks[c].kind, ChunkKind::ProgramHeaders))
                .unwrap();
            if k.interp {
                covers(PT_PHDR, phdr, PF_R);
                covers(PT_INTERP, chunk_named(&placed, b".interp"), PF_R);
            }
            let dynamic = chunk_named(&placed, b".dynamic");
            covers(PT_DYNAMIC, dynamic, PF_R | PF_W);

            // L-64: the RELRO run ends on a maxpage boundary at
            // .relro_padding's end; .dynamic, .got, (now) .got.plt and
            // .bss.rel.ro inside it.
            if k.relro {
                let r = seg(PT_GNU_RELRO).unwrap();
                assert_eq!((r.addr + r.memsz) % w.options.max_page, 0, "{what}");
                assert_eq!(r.addr, loads[2].addr, "{what}");
                for n in [&b".dynamic"[..], b".got", b".bss.rel.ro"]
                    .into_iter()
                    .chain(k.now.then_some(&b".got.plt"[..]))
                {
                    let c = &p.chunks[chunk_named(&placed, n)];
                    let at = c.addr.unwrap();
                    assert!(at >= r.addr && at + c.size <= r.addr + r.memsz, "{what}");
                    assert!(c.relro, "{what}");
                }
                let got_plt = &p.chunks[chunk_named(&placed, b".got.plt")];
                assert_eq!(got_plt.relro, k.now, "{what}");
            }

            // D-42: the copy allocations.
            let dynbss = chunk_named(&placed, b".dynbss");
            let relro_copy = chunk_named(&placed, b".bss.rel.ro");
            let place_of = |i| {
                let sp = p.sym_place.iter().find(|s| s.key == AllocKey::Copy(i)).unwrap();
                (sp.chunk, sp.offset, sp.size, sp.align)
            };
            assert_eq!(place_of(0), (dynbss, 0, 8, 3), "{what}");
            assert_eq!(place_of(2), (dynbss, 8, 4, 2), "{what}");
            assert_eq!(place_of(1), (relro_copy, 0, 0x18, 3), "{what}");
            // L-5: .dynbss leads .bss, so it carries .bss's alignment.
            assert_eq!(p.chunks[dynbss].align, 6, "{what}");
            // .dynbss is the first chunk of .bss's section.
            let at = p.order.iter().position(|&c| c == dynbss).unwrap();
            let next = &p.chunks[p.order[at + 1]];
            assert_eq!(p.chunks[dynbss].shndx, next.shndx, "{what}: .dynbss joins .bss");
            assert_ne!(p.chunks[p.order[at - 1]].shndx, next.shndx, "{what}: .dynbss first");

            // §13.1: _DYNAMIC is the start of .dynamic.
            let v = p.defined.iter().find(|d| d.symbol == SymbolId(0)).unwrap().value;
            assert_eq!(v, DefValue::At { chunk: dynamic, offset: 0 }, "{what}");

            // The [G] rules, from the file alone.
            let bytes = write_skeleton(p, &a, machine, true);
            let g = check_g(&read_elf(&bytes));
            assert!(g.is_empty(), "{what}: {g:?}");
            cases += 1;
        }
    }
    assert_eq!(cases, 18);
}

// spec: dyn.md D-62 (layout is a pure function): placing again from the
// previous placement gives the same placement.
#[test]
fn d62_again() {
    for k in kinds() {
        let w = world(Machine::X86_64, k);
        let (a, placed) = place(&w);
        let pool = Pool::sequential();
        let mut a2 = w.stage_a(&pool).unwrap();
        let again = place_with_copies(
            &w.link(),
            &copy_rows(),
            &mut a2,
            None,
            |_| None,
            &pool,
            Some(&placed),
        )
        .unwrap();
        assert_eq!(again.placement, placed.placement, "{k:?}");
        assert_eq!(a2.outsecs.len(), a.outsecs.len());
    }
}

// spec: dyn.md D-42 (a copy chunk whose size is not its allocations'
// extent, or copies with no chunk, is a defect of the caller)
#[test]
fn d42_copy_chunk_size() {
    let mut w = world(Machine::AArch64, kinds()[0]);
    w.synthetic.retain(|s| s.synth.name != b".dynbss");
    let pool = Pool::sequential();
    let mut a = w.stage_a(&pool).unwrap();
    let e = place_with_copies(&w.link(), &copy_rows(), &mut a, None, |_| None, &pool, None)
        .unwrap_err();
    assert!(format!("{e:?}").contains("copy allocations with no .dynbss chunk"), "{e:?}");
    let mut w = world(Machine::AArch64, kinds()[0]);
    for s in &mut w.synthetic {
        if s.synth.name == b".bss.rel.ro" {
            s.size = 0x20;
        }
    }
    let mut a = w.stage_a(&pool).unwrap();
    let e = place_with_copies(&w.link(), &copy_rows(), &mut a, None, |_| None, &pool, None)
        .unwrap_err();
    assert!(format!("{e:?}").contains(".bss.rel.ro has size 0x20"), "{e:?}");
}

// spec: layout.md L-39, dyn.md R-L2 (an input .interp is refused by name)
#[test]
fn input_interp_refused() {
    let mut f = files();
    f[0].push(sec(b".interp", P, A, 0x1b, 0));
    let w = World::new(Machine::X86_64, &f, &[]);
    let e = w.stage_a(&Pool::sequential()).unwrap_err();
    assert!(format!("{e:?}").contains(".interp"), "{e:?}");
}

// spec: layout.md L-5, dyn.md D-42 (the leading .dynbss carries .bss's
// alignment: the section's sh_addr is its address)
#[test]
fn l5_dynbss_aligned_as_bss() {
    let w = world(Machine::AArch64, kinds()[0]);
    let (a, mut placed) = place(&w);
    let dynbss = chunk_named(&placed, b".dynbss");
    assert_eq!(placed.placement.chunks[dynbss].align, 6);
    placed.placement.chunks[dynbss].align = 3;
    let v = check(&w, &a, &placed).unwrap_err();
    assert!(v.what.contains("less aligned than its section"), "{v:?}");
}
