//! The stages on synthetic links, by rule id (`spec/layout.md`).
#![allow(clippy::many_single_char_names, clippy::cast_possible_truncation)]

mod common;

use common::{Sec, World, sec};
use mortar_core::id::{ChunkId, OutSecId, SymbolId};
use mortar_core::layout::{ChunkKind, DefValue};
use mortar_core::par::{Pool, check_schedules};
use mortar_core::resolution::{Def, InternalDef};
use mortar_elf::Machine;
use mortar_elf::consts::{
    PF_R, PF_W, PF_X, PT_GNU_RELRO, PT_GNU_STACK, PT_LOAD, PT_TLS, SHF_ALLOC, SHF_EXECINSTR,
    SHF_TLS, SHF_WRITE, SHT_INIT_ARRAY, SHT_NOBITS, SHT_PROGBITS,
};

const A: u64 = SHF_ALLOC;
const AX: u64 = SHF_ALLOC | SHF_EXECINSTR;
const WA: u64 = SHF_ALLOC | SHF_WRITE;
const WAT: u64 = WA | SHF_TLS;
const P: u32 = SHT_PROGBITS;
const PROVIDE: Def = Def::Internal(InternalDef::Provide { conditional: false });

fn hello_files() -> Vec<Vec<Sec>> {
    vec![
        vec![
            sec(b".text", P, AX, 0x40, 3),
            sec(b".data", P, WA, 0x10, 3),
            sec(b".bss", SHT_NOBITS, WA, 0x20, 4),
            sec(b".note.GNU-stack", P, 0, 0, 0),
        ],
        vec![
            sec(b".text.startup.main", P, AX, 0x30, 4),
            sec(b".text", P, AX, 0x100, 4),
            sec(b".rodata.str1.1", P, A, 0x21, 0),
            sec(b".init_array", SHT_INIT_ARRAY, WA, 8, 3),
            sec(b".tdata", P, WAT, 8, 3),
            sec(b".tbss", SHT_NOBITS, WAT, 0x10, 4),
            sec(b".comment", P, 0, 0x2a, 0),
        ],
    ]
}

fn hello() -> World {
    let mut w = World::new(
        Machine::AArch64,
        &hello_files(),
        &[
            (b"_start", Def::Undefined { weak: false }),
            (b"_end", PROVIDE),
            (b"__init_array_start", PROVIDE),
            (b"__bss_start", PROVIDE),
            (b"etext", PROVIDE),
            (b"__ehdr_start", PROVIDE),
        ],
    );
    w.define(0, 1, 1, 0);
    w
}

fn names(
    w: &World,
    a: &mortar_core::assignment::Assignment<'_>,
    placed: &mortar_layout::Placed,
) -> Vec<String> {
    let _ = w;
    placed
        .placement
        .order
        .iter()
        .map(|&c| {
            let chunk = &placed.placement.chunks[c];
            match chunk.kind {
                ChunkKind::FileHeader => "ehdr".to_owned(),
                ChunkKind::ProgramHeaders => "phdr".to_owned(),
                ChunkKind::SectionHeaders => "shdr".to_owned(),
                ChunkKind::Synthetic(s) => String::from_utf8_lossy(s.name).into_owned(),
                ChunkKind::Patches(p) => {
                    format!("island({})", String::from_utf8_lossy(a.outsecs[p.out].name))
                }
                k => String::from_utf8_lossy(a.outsecs[k.out().unwrap()].name).into_owned(),
            }
        })
        .collect()
}

// spec: layout.md §7, §10 L-60, §11 L-65..L-70, model §4.4 (a static hello is valid)
#[test]
fn hello_is_valid() {
    let w = hello();
    let (a, placed) = w.run(&Pool::sequential()).unwrap();
    w.check(&a, &placed).unwrap();
    assert_eq!(
        names(&w, &a, &placed),
        [
            "ehdr",
            "phdr",
            ".rodata",
            ".text",
            "island(.text)",
            ".tdata",
            ".tbss",
            ".init_array",
            ".relro_padding",
            ".data",
            ".bss",
            ".comment",
            "shdr"
        ]
    );
    assert!(placed.warnings.is_empty(), "{:?}", placed.warnings);
    let p = &placed.placement;
    let segs: Vec<(u32, u32)> = p.segments.iter().map(|(_, s)| (s.p_type, s.flags)).collect();
    assert_eq!(
        segs,
        [
            (PT_LOAD, PF_R),
            (PT_LOAD, PF_R | PF_X),
            (PT_LOAD, PF_R | PF_W),
            (PT_LOAD, PF_R | PF_W),
            (PT_TLS, PF_R),
            (PT_GNU_STACK, PF_R | PF_W),
            (PT_GNU_RELRO, PF_R),
        ]
    );
    // L-68: the first PT_LOAD maps the file header at the base.
    let (_, first) = p.segments.iter().next().unwrap();
    assert_eq!((first.addr, first.offset), (0x40_0000, 0));
    // L-102: _start is the first byte of crt1's .text.
    let text = p.place[w.sid(1, 1)].get().unwrap();
    assert_eq!(p.entry, Some(p.chunks[text.0].addr.unwrap() + text.1));
    // L-64: the RELRO end is a multiple of maxpage.
    let (_, relro) = p.segments.iter().find(|(_, s)| s.p_type == PT_GNU_RELRO).unwrap();
    assert_eq!((relro.addr + relro.memsz) % 0x1_0000, 0);
}

// spec: layout.md §13.1 L-91, L-92, L-95 (the linker-defined values)
#[test]
fn l91_values() {
    let w = hello();
    let (a, placed) = w.run(&Pool::sequential()).unwrap();
    let p = &placed.placement;
    let value = |s: u32| p.defined.iter().find(|d| d.symbol == SymbolId(s)).unwrap().value;
    let out_chunk = |name: &[u8]| {
        let o = a.outsecs.iter().find(|(_, o)| o.name == name).unwrap().0;
        p.order.iter().copied().find(|&c| p.chunks[c].kind.out() == Some(o)).unwrap()
    };
    let bss = out_chunk(b".bss");
    assert_eq!(value(1), DefValue::At { chunk: bss, offset: 0x20 }, "_end");
    assert_eq!(value(2), DefValue::At { chunk: out_chunk(b".init_array"), offset: 0 });
    assert_eq!(value(3), DefValue::At { chunk: bss, offset: 0 }, "__bss_start");
    let island = p
        .order
        .iter()
        .copied()
        .find(|&c| matches!(p.chunks[c].kind, ChunkKind::Patches(_)))
        .unwrap();
    // etext: the end of the last exec chunk, the (empty) island.
    assert_eq!(value(4), DefValue::At { chunk: island, offset: 0 });
    assert_eq!(value(5), DefValue::At { chunk: ChunkId(0), offset: 0 });
}

// spec: layout.md §7 (determinism), model §6.3 (every schedule gives one result)
#[test]
fn determinism_schedules() {
    let w = hello();
    let got = check_schedules(|pool| {
        let (a, placed) = w.run(pool).unwrap();
        (a.outsecs.iter().map(|(_, o)| o.name.to_vec()).collect::<Vec<_>>(), placed.placement)
    });
    assert!(!got.1.chunks.is_empty());
}

// spec: layout.md §9 L-53 (.text categories before the rest)
#[test]
fn l53_text_order() {
    let w = hello();
    let (a, placed) = w.run(&Pool::sequential()).unwrap();
    let p = &placed.placement;
    let at = |f, i| {
        let (c, off) = p.place[w.sid(f, i)].get().unwrap();
        p.chunks[c].addr.unwrap() + off
    };
    // .text.startup.main (file 2) before crt1's .text (file 1).
    assert!(at(2, 1) < at(1, 1));
    assert!(at(1, 1) < at(2, 2));
    let _ = (a, OutSecId(0));
}

#[test]
#[ignore = "prints the hello layout"]
fn print_hello() {
    let w = hello();
    let (a, placed) = w.run(&Pool::sequential()).unwrap();
    let p = &placed.placement;
    for (n, &c) in names(&w, &a, &placed).iter().zip(p.order.iter()) {
        let ch = &p.chunks[c];
        println!(
            "{n:16} addr={:?} off={:#x} size={:#x} align={} shndx={:?}",
            ch.addr.map(|a| format!("{a:#x}")),
            ch.offset,
            ch.size,
            ch.align,
            ch.shndx
        );
    }
    for (_, s) in p.segments.iter() {
        println!(
            "type={:#x} flags={} off={:#x} va={:#x} fs={:#x} ms={:#x} al={}",
            s.p_type, s.flags, s.offset, s.addr, s.filesz, s.memsz, s.align
        );
    }
    println!("{:?} {:?}", p.tls, p.entry);
}

fn refusal_of(files: &[Vec<Sec>], machine: Machine) -> Option<String> {
    let w = World::new(machine, files, &[]);
    let e = w.stage_a(&Pool::sequential()).err()?;
    e[0].1.refusal().map(|(class, name)| format!("{class:?}:{name}"))
}

// spec: layout.md §8.1 L-36..L-40 (every M1 refusal of stage A, by name)
#[test]
fn l36_l40_refusals() {
    use mortar_elf::consts::SHF_X86_64_LARGE;
    let one = |s: Sec| vec![vec![sec(b".text", P, AX, 4, 2), s]];
    let x86 = Machine::X86_64;
    let cases: [(Sec, Machine, &str); 6] = [
        (sec(b".ldata", P, WA | SHF_X86_64_LARGE, 8, 3), x86, "SectionFlag:SHF_X86_64_LARGE"),
        (
            sec(b".gnu.linkonce.t.f", P, AX, 8, 2),
            Machine::AArch64,
            "Feature:.gnu.linkonce sections",
        ),
        (sec(b".zdebug_info", P, 0, 8, 0), Machine::AArch64, "CompressionType:.zdebug"),
        (sec(b".got", P, WA, 8, 3), Machine::AArch64, "Feature:input section .got"),
        (sec(b".ctors", P, WA, 8, 3), Machine::AArch64, "Feature:.ctors section"),
        (sec(b".dtors.00100", P, WA, 8, 3), x86, "Feature:.ctors section"),
    ];
    for (s, m, want) in cases {
        assert_eq!(
            refusal_of(&one(s), m).as_deref(),
            Some(want),
            "{:?}",
            String::from_utf8_lossy(s.name)
        );
    }
    // An empty .ctors is not refused (L-40), and SHF_X86_64_LARGE means
    // nothing on AArch64 (it is a processor flag).
    assert_eq!(refusal_of(&one(sec(b".ctors", P, WA, 0, 3)), Machine::AArch64), None);
}

// spec: layout.md §8.1 L-33..L-35 (consumed sections go nowhere)
#[test]
fn l33_l35_consumed() {
    use mortar_core::assignment::OutOf;
    let files = vec![vec![
        sec(b".text", P, AX, 4, 2),
        sec(b".gnu.warning.f", P, 0, 8, 0),
        sec(b".note.gnu.property", mortar_elf::consts::SHT_NOTE, A, 0x20, 3),
    ]];
    let w = World::new(Machine::AArch64, &files, &[]);
    let a = w.stage_a(&Pool::sequential()).unwrap();
    assert_eq!(a.out_of[w.sid(1, 2)], OutOf::Consumed);
    assert_eq!(a.out_of[w.sid(1, 3)], OutOf::Consumed);
    assert!(matches!(a.out_of[w.sid(1, 1)], OutOf::Out(_)));
}

// spec: layout.md §8.3 L-47 (TLS and non-TLS in one output section: section-conflict)
#[test]
fn l47_section_conflict() {
    let files = vec![
        vec![sec(b".text", P, AX, 4, 2), sec(b".tdata", P, WAT, 8, 3)],
        vec![sec(b".tdata", P, WA, 8, 3)],
    ];
    let w = World::new(Machine::AArch64, &files, &[]);
    let e = w.run(&Pool::sequential()).unwrap_err();
    assert_eq!(e[0].id(), "section-conflict");
    assert!(e[0].is_error());
}

// spec: layout.md §9.1 L-54 (init_array: numbered by priority, then the unnumbered, in input order)
#[test]
fn l54_init_array_order() {
    let ia = |n: &'static [u8]| sec(n, SHT_INIT_ARRAY, WA, 8, 3);
    let files = vec![
        vec![sec(b".text", P, AX, 4, 2), ia(b".init_array"), ia(b".init_array.00200")],
        vec![ia(b".init_array.00100"), ia(b".init_array"), ia(b".init_array.00200")],
    ];
    let w = World::new(Machine::AArch64, &files, &[]);
    let (_, placed) = w.run(&Pool::sequential()).unwrap();
    let p = &placed.placement;
    let off = |f, i| p.place[w.sid(f, i)].get().unwrap().1;
    let mut by_off = [(1, 2), (1, 3), (2, 1), (2, 2), (2, 3)];
    by_off.sort_by_key(|&(f, i)| off(f, i));
    assert_eq!(by_off, [(2, 1), (1, 3), (2, 3), (1, 2), (2, 2)]);
}

// spec: layout.md §9.2 L-57 (SHF_LINK_ORDER follows the linked sections' order)
#[test]
fn l57_link_order() {
    use mortar_elf::consts::SHF_LINK_ORDER;
    let mut m1 = sec(b"__patchable", P, A | SHF_LINK_ORDER, 8, 3);
    m1.link = 2;
    let mut m2 = sec(b"__patchable", P, A | SHF_LINK_ORDER, 8, 3);
    m2.link = 1;
    let files = vec![vec![
        sec(b".text.unlikely.a", P, AX, 4, 2),
        sec(b".text.startup.b", P, AX, 4, 2),
        m1,
        m2,
    ]];
    let w = World::new(Machine::AArch64, &files, &[]);
    let (_, placed) = w.run(&Pool::sequential()).unwrap();
    let p = &placed.placement;
    let off = |i| p.place[w.sid(1, i)].get().unwrap().1;
    // .text.unlikely.a (index 1) comes first in .text, so m2 (linked to 1) first.
    assert!(off(4) < off(3));
}

// spec: layout.md §11.4 L-76, L-77 (.tbss-only TLS: the template and the next chunk)
#[test]
fn l77_tbss_only() {
    let files = vec![vec![
        sec(b".text", P, AX, 4, 2),
        sec(b".tbss", SHT_NOBITS, WAT, 0x30, 6),
        sec(b".data", P, WA, 8, 3),
    ]];
    let w = World::new(Machine::AArch64, &files, &[]);
    let (a, placed) = w.run(&Pool::sequential()).unwrap();
    w.check(&a, &placed).unwrap();
    let tls = placed.placement.tls.unwrap();
    assert_eq!((tls.size, tls.align), (0x30, 6));
    assert_eq!(tls.start % 64, 0);
}

// spec: layout.md §11.2 L-69, L-70 (large alignments: p_align and congruence)
#[test]
fn l70_large_alignment() {
    let files = vec![vec![
        sec(b".text", P, AX, 4, 2),
        sec(b".data.big", P, WA, 0x10, 21),
        sec(b".bss", SHT_NOBITS, WA, 0x10, 3),
    ]];
    let w = World::new(Machine::AArch64, &files, &[]);
    let (a, placed) = w.run(&Pool::sequential()).unwrap();
    w.check(&a, &placed).unwrap();
    let p = &placed.placement;
    let (c, _) = p.place[w.sid(1, 2)].get().unwrap();
    assert_eq!(p.chunks[c].addr.unwrap() % (1 << 21), 0);
    let rw =
        p.segments.iter().find(|(_, s)| s.p_type == PT_LOAD && s.flags == PF_R | PF_W).unwrap().1;
    assert_eq!(rw.align, 21);
    assert_eq!(rw.addr % (1 << 21), rw.offset % (1 << 21));
}

// spec: layout.md §11.2 L-71 (separate-code: the RX offset and the next segment's on maxpage)
#[test]
fn l71_separate_code() {
    let mut w = hello();
    w.options.separate_code = true;
    let (a, placed) = w.run(&Pool::sequential()).unwrap();
    w.check(&a, &placed).unwrap();
    let loads: Vec<_> = placed
        .placement
        .segments
        .iter()
        .filter(|(_, s)| s.p_type == PT_LOAD)
        .map(|(_, s)| *s)
        .collect();
    let rx = loads.iter().position(|s| s.flags & PF_X != 0).unwrap();
    assert_eq!(loads[rx].offset % 0x1_0000, 0);
    assert_eq!(loads[rx + 1].offset % 0x1_0000, 0);
}

// spec: layout.md §11.2 L-68 (base 0 is refused)
#[test]
fn l68_base_zero() {
    let mut w = hello();
    w.target.image_base = 0x8000;
    let e = w.run(&Pool::sequential()).unwrap_err();
    assert_eq!(e[0].refusal().map(|r| r.1), Some("non-PIE image base 0"));
}

// spec: layout.md §11.2 L-73 (output-too-large)
#[test]
fn l73_too_large() {
    let files =
        vec![vec![sec(b".text", P, AX, 4, 2), sec(b".bss", SHT_NOBITS, WA, 0xfff0_0000, 3)]];
    let w = World::new(Machine::I386, &files, &[]);
    let e = w.run(&Pool::sequential()).unwrap_err();
    assert_eq!(e[0].id(), "output-too-large");
}

// spec: layout.md §11.6 L-84 (exec note: RWX stack and one warning)
#[test]
fn l84_executable_stack() {
    use mortar_layout::stack::StackNote;
    let mut w = hello();
    w.stack_notes = vec![("a.o".into(), StackNote::NonExec), ("b.o".into(), StackNote::Exec)];
    let (a, placed) = w.run(&Pool::sequential()).unwrap();
    w.check(&a, &placed).unwrap();
    let stack = placed.placement.segments.iter().find(|(_, s)| s.p_type == PT_GNU_STACK).unwrap().1;
    assert_eq!(stack.flags, PF_R | PF_W | PF_X);
    assert_eq!(placed.warnings.len(), 1);
    assert_eq!(
        placed.warnings[0].message(),
        "b.o: requires executable stack (because the .note.GNU-stack section is executable)"
    );
    w.options.execstack = Some(false);
    let (_, placed) = w.run(&Pool::sequential()).unwrap();
    assert!(placed.warnings.is_empty());
}

// spec: layout.md §13.6 L-103 (an undefined entry: entry-not-found, e_entry 0)
#[test]
fn l103_entry_not_found() {
    let w = World::new(
        Machine::AArch64,
        &hello_files(),
        &[(b"_start", Def::Undefined { weak: false })],
    );
    let (a, placed) = w.run(&Pool::sequential()).unwrap();
    w.check(&a, &placed).unwrap();
    assert_eq!(placed.placement.entry, None);
    assert_eq!(placed.warnings.len(), 1);
    assert_eq!(placed.warnings[0].id(), "entry-not-found");
    assert!(!placed.warnings[0].is_error());
}

// spec: layout.md §13.6 L-102 (a number given to -e), L-104 (an entry outside code is refused)
#[test]
fn l102_l104_entry() {
    use mortar_layout::target::EntryName;
    let mut w = hello();
    let (_, placed) = w.run(&Pool::sequential()).unwrap();
    let text_addr = placed.placement.entry.unwrap();
    w.options.entry = EntryName::Symbol(format!("{:#x}", text_addr + 4).into_bytes());
    let (_, placed) = w.run(&Pool::sequential()).unwrap();
    assert_eq!(placed.placement.entry, Some(text_addr + 4));
    // A data symbol as the entry.
    let mut w = hello();
    w.define(0, 1, 2, 0);
    let e = w.run(&Pool::sequential()).unwrap_err();
    assert_eq!(e[0].refusal().map(|r| r.1), Some("entry point outside code"));
}

// spec: layout.md §13.3 L-98 (_TLS_MODULE_BASE_ without TLS is refused)
#[test]
fn l98_tls_module_base() {
    let files = vec![vec![sec(b".text", P, AX, 4, 2)]];
    let w = World::new(Machine::AArch64, &files, &[(b"_TLS_MODULE_BASE_", PROVIDE)]);
    let e = w.run(&Pool::sequential()).unwrap_err();
    assert_eq!(e[0].refusal().map(|r| r.1), Some("_TLS_MODULE_BASE_ without TLS"));
    let w = World::new(Machine::AArch64, &hello_files(), &[(b"_TLS_MODULE_BASE_", PROVIDE)]);
    let (_, placed) = w.run(&Pool::sequential()).unwrap();
    let p = &placed.placement;
    let DefValue::At { chunk, offset } = p.defined[0].value else { panic!() };
    assert_eq!(p.chunks[chunk].addr.unwrap() + offset, p.tls.unwrap().start);
}

// spec: layout.md §13.1 L-93 (absent arrays: both bounds at the base), resolve.md §8.1 (withdrawn __start_)
#[test]
fn l93_absent_and_withdrawn() {
    let cond = Def::Internal(InternalDef::Provide { conditional: true });
    let files = vec![vec![sec(b".text", P, AX, 4, 2), sec(b"my_sec", P, A, 8, 3)]];
    let w = World::new(
        Machine::AArch64,
        &files,
        &[
            (b"__preinit_array_start", PROVIDE),
            (b"__preinit_array_end", PROVIDE),
            (b"__start_my_sec", cond),
            (b"__stop_my_sec", cond),
            (b"__start_nothing", cond),
        ],
    );
    let (a, placed) = w.run(&Pool::sequential()).unwrap();
    w.check(&a, &placed).unwrap();
    assert_eq!(&a.withdrawn[..], &[SymbolId(4)]);
    let p = &placed.placement;
    let v = |s| p.defined.iter().find(|d| d.symbol == SymbolId(s)).unwrap().value;
    assert_eq!(v(0), DefValue::At { chunk: ChunkId(0), offset: 0 });
    assert_eq!(v(1), v(0));
    let (c, _) = p.place[w.sid(1, 2)].get().unwrap();
    assert_eq!(v(2), DefValue::At { chunk: c, offset: 0 });
    assert_eq!(v(3), DefValue::At { chunk: c, offset: 8 });
}

// spec: layout.md §12.2 L-88 (commons at the end of .bss), L-89 (a TLS common is refused)
#[test]
fn l88_commons() {
    use mortar_layout::chunk::Common;
    let mut w = hello();
    w.commons = vec![Common { symbol: SymbolId(1), size: 0x40, align: 5, tls: false }];
    w.resolution.def[SymbolId(1)] = Def::Common(mortar_core::id::SymEntry {
        file: mortar_core::id::FileId(2),
        index: mortar_elf::SymbolIndex(9),
    });
    w.resolution.common =
        vec![(SymbolId(1), mortar_core::resolution::CommonShape { size: 0x40, align: 5 })].into();
    let (a, placed) = w.run(&Pool::sequential()).unwrap();
    w.check(&a, &placed).unwrap();
    let sp = placed.placement.sym_place[0];
    assert_eq!(sp.offset % 32, 0);
    assert!(sp.offset >= 0x20);
    w.commons[0].tls = true;
    let e = w.run(&Pool::sequential()).unwrap_err();
    assert_eq!(e[0].refusal().map(|r| r.1), Some("TLS common symbol"));
}

// spec: layout.md §20.2 (-z norelro: rank 7 joins RW, no padding, no PT_GNU_RELRO)
#[test]
fn norelro() {
    let mut w = hello();
    w.options.relro = false;
    let (a, placed) = w.run(&Pool::sequential()).unwrap();
    w.check(&a, &placed).unwrap();
    let p = &placed.placement;
    assert!(!names(&w, &a, &placed).iter().any(|n| n == ".relro_padding"));
    assert!(!p.segments.iter().any(|(_, s)| s.p_type == PT_GNU_RELRO));
    let rw = p.segments.iter().filter(|(_, s)| s.p_type == PT_LOAD && s.flags == PF_R | PF_W).count();
    assert_eq!(rw, 1);
}
