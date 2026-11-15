//! L1 tests of `.symtab`, `.strtab` and `.symtab_shndx` (`spec/synth.md`
//! §7.1 to §7.3), the review findings that concern them, sub-range
//! writes (S-4, I-103) and determinism.

mod common;

use std::collections::BTreeMap;

use common::{Addrs, Secs, Syms, collector, text};
use mortar_core::layout::{Erratum, Patch, PatchKind, ThunkTarget};
use mortar_core::par::{Pool, check_schedules};
use mortar_core::plan::Discard;
use mortar_core::{ChunkId, FileId, SectionId, Span, SymKey, SymbolId};
use mortar_elf::Class;
use mortar_elf::consts::{
    STB_GLOBAL, STB_WEAK, STT_FILE, STT_FUNC, STT_GNU_IFUNC, STT_NOTYPE, STT_OBJECT, STT_SECTION,
    STT_TLS, STV_HIDDEN, STV_PROTECTED,
};
use mortar_synth::out::Window;
use mortar_synth::symtab::{
    GlobalIn, InternalValue, LocalIn, PatchFacts, SymCtx, SymDef, SymtabPlan, fixed_table,
};

fn local(name: &'static [u8], typ: u8, def: SymDef) -> LocalIn<'static> {
    LocalIn { name, info: typ, other: 0, size: 4, def, referenced: false }
}

fn in_sec(section: u32, value: u64) -> SymDef {
    SymDef::Section { section: SectionId(section), value }
}

fn global(
    name: &'static [u8],
    typ: u8,
    binding: u8,
    other: u8,
    def: SymDef,
    file: u32,
) -> GlobalIn<'static> {
    GlobalIn { name, typ, binding, other, size: 8, def, file: FileId(file), referenced: true }
}

/// The program of the order tests.
fn program() -> (Syms, Secs, Addrs) {
    let syms = Syms {
        files: vec![
            vec![],
            vec![
                local(b"a.c", STT_FILE, SymDef::Absolute(0)),
                local(b".Ltmp", STT_NOTYPE, in_sec(1, 4)),
                local(b"s1", STT_FUNC, in_sec(1, 0)),
                local(b"", STT_SECTION, in_sec(1, 0)),
                local(b"dead", STT_FUNC, in_sec(2, 0)),
                local(b"$x", STT_NOTYPE, in_sec(1, 0)),
                local(b"tl", STT_TLS, in_sec(900, 8)),
            ],
            vec![local(b"s2", STT_OBJECT, in_sec(3, 0))],
        ],
        globals: vec![
            Some(global(b"main", STT_FUNC, STB_GLOBAL, 0, in_sec(1, 0x10), 1)),
            Some(global(b"hid", STT_OBJECT, STB_GLOBAL, STV_HIDDEN, in_sec(3, 4), 2)),
            Some(global(
                b"_GLOBAL_OFFSET_TABLE_",
                STT_NOTYPE,
                STB_GLOBAL,
                STV_HIDDEN,
                SymDef::Internal,
                0,
            )),
            Some(GlobalIn {
                referenced: false,
                ..global(b"unref", STT_NOTYPE, STB_WEAK, 0, SymDef::Undefined, 1)
            }),
            Some(global(b"weak_ref", STT_NOTYPE, STB_WEAK, 0, SymDef::Undefined, 1)),
            Some(global(b"com", STT_NOTYPE, STB_GLOBAL, 0, SymDef::Common, 2)),
            Some(global(b"ifn", STT_GNU_IFUNC, STB_GLOBAL, 0, in_sec(1, 0x40), 1)),
            None,
            Some(global(b"_end", STT_NOTYPE, STB_GLOBAL, 0, SymDef::Internal, 0)),
            Some(global(b"__start_s", STT_NOTYPE, STB_GLOBAL, STV_PROTECTED, SymDef::Internal, 0)),
            Some(global(b"in_dead", STT_FUNC, STB_GLOBAL, 0, in_sec(2, 0), 1)),
        ],
    };
    let secs = Secs { dead: vec![SectionId(2)], eh: vec![] };
    let addrs = Addrs {
        internal: BTreeMap::from([
            (2, InternalValue::At { shndx: 1, addr: 0x10000 }),
            (8, InternalValue::At { shndx: 1, addr: 0x20000 }),
            (9, InternalValue::At { shndx: 1, addr: 0x10300 }),
        ]),
        high: None,
    };
    (syms, secs, addrs)
}

const FACTS: PatchFacts = PatchFacts { thunk_size: 12, mapping: true };

fn plan(cx: &SymCtx<'_>, patches: &[Patch]) -> Result<SymtabPlan, String> {
    let diag = collector();
    let p = SymtabPlan::new(cx, Class::Elf64, patches, FACTS, &Pool::sequential(), &diag);
    let r = diag.finish();
    p.map_err(|_| text(&r))
}

#[derive(Debug, PartialEq, Eq)]
struct Entry {
    name: String,
    info: u8,
    other: u8,
    shndx: u16,
    value: u64,
    size: u64,
}

fn write_all(p: &SymtabPlan, cx: &SymCtx<'_>, addrs: &Addrs) -> Result<(Vec<u8>, Vec<u8>), String> {
    let mut sym = vec![0xaa; p.symtab_size() as usize];
    let mut str = vec![0xaa; p.strtab_size() as usize];
    p.write_symtab(cx, addrs, &mut Window::new(0, &mut sym))
        .map_err(|e| format!("{:?}", e.into_diagnostic()))?;
    p.write_strtab(cx, &mut Window::new(0, &mut str))
        .map_err(|e| format!("{:?}", e.into_diagnostic()))?;
    Ok((sym, str))
}

fn entries(sym: &[u8], str: &[u8]) -> Vec<Entry> {
    sym.chunks(24)
        .map(|e| {
            let at = u32::from_le_bytes(e[0..4].try_into().unwrap()) as usize;
            let end = at + str[at..].iter().position(|&b| b == 0).unwrap();
            Entry {
                name: String::from_utf8_lossy(&str[at..end]).into_owned(),
                info: e[4],
                other: e[5],
                shndx: u16::from_le_bytes(e[6..8].try_into().unwrap()),
                value: u64::from_le_bytes(e[8..16].try_into().unwrap()),
                size: u64::from_le_bytes(e[16..24].try_into().unwrap()),
            }
        })
        .collect()
}

fn names(es: &[Entry]) -> Vec<&str> {
    es.iter().map(|e| e.name.as_str()).collect()
}

fn cx<'a>(syms: &'a Syms, secs: &'a Secs, discard: Option<Discard>) -> SymCtx<'a> {
    SymCtx { syms, sections: secs, eh: None, discard }
}

#[test]
fn s74_s75_order_and_sh_info() {
    let (syms, secs, addrs) = program();
    let c = cx(&syms, &secs, None);
    let p = plan(&c, &[]).unwrap();
    let (sym, str) = write_all(&p, &c, &addrs).unwrap();
    let es = entries(&sym, &str);
    assert_eq!(
        names(&es),
        [
            "",
            "_GLOBAL_OFFSET_TABLE_",
            "a.c",
            "s1",
            "$x",
            "tl",
            "s2",
            "hid",
            "main",
            "weak_ref",
            "com",
            "ifn",
            "_end",
            "__start_s"
        ]
    );
    assert_eq!(p.sh_info(), 8, "the null entry and seven locals");
    assert_eq!(p.entries() as usize, es.len());
}

#[test]
fn s88_symtab_locals_per_file() {
    let (syms, secs, _) = program();
    let p = plan(&cx(&syms, &secs, None), &[]).unwrap();
    let spans: Vec<Span> = (0..3).map(|f| p.symtab_locals()[FileId(f)]).collect();
    assert_eq!(
        spans,
        [Span { start: 1, len: 1 }, Span { start: 2, len: 4 }, Span { start: 6, len: 2 }]
    );
}

#[test]
fn s76_s79_dead_sections_and_section_symbols_are_dropped() {
    let (syms, secs, addrs) = program();
    let c = cx(&syms, &secs, None);
    let es = {
        let p = plan(&c, &[]).unwrap();
        let (sym, str) = write_all(&p, &c, &addrs).unwrap();
        entries(&sym, &str)
    };
    assert!(!names(&es).contains(&"dead") && !names(&es).contains(&"in_dead"));
    assert!(es.iter().all(|e| e.info & 0xf != STT_SECTION));
}

#[test]
fn s77_l91_demoted_and_linker_defined_bindings() {
    let (syms, secs, addrs) = program();
    let c = cx(&syms, &secs, None);
    let p = plan(&c, &[]).unwrap();
    let (sym, str) = write_all(&p, &c, &addrs).unwrap();
    let es = entries(&sym, &str);
    let by = |n: &str| es.iter().find(|e| e.name == n).unwrap();
    assert_eq!((by("hid").info >> 4, by("hid").other), (0, STV_HIDDEN), "local, hidden kept");
    assert_eq!(
        (by("_GLOBAL_OFFSET_TABLE_").info >> 4, by("_GLOBAL_OFFSET_TABLE_").other),
        (0, STV_HIDDEN)
    );
    assert_eq!((by("_end").info >> 4, by("_end").other), (STB_GLOBAL, 0));
    assert_eq!((by("__start_s").info >> 4, by("__start_s").other), (STB_GLOBAL, STV_PROTECTED));
    assert_eq!(by("weak_ref").info >> 4, STB_WEAK);
}

#[test]
fn s78_default_drops_dot_l_and_keeps_file_and_mapping() {
    let (syms, secs, _) = program();
    let none = plan(&cx(&syms, &secs, Some(Discard::None)), &[]).unwrap();
    let default = plan(&cx(&syms, &secs, None), &[]).unwrap();
    let locals = plan(&cx(&syms, &secs, Some(Discard::Locals)), &[]).unwrap();
    assert_eq!(none.entries(), default.entries() + 1, "--discard-none keeps .Ltmp");
    assert_eq!(locals.entries(), default.entries(), "-X is the default");
}

#[test]
fn s78_discard_all_keeps_demoted_and_the_linker_block() {
    let (syms, secs, addrs) = program();
    let c = cx(&syms, &secs, Some(Discard::All));
    let p = plan(&c, &[]).unwrap();
    let (sym, str) = write_all(&p, &c, &addrs).unwrap();
    let es = entries(&sym, &str);
    assert_eq!(&names(&es)[..4], ["", "_GLOBAL_OFFSET_TABLE_", "hid", "main"]);
    assert_eq!(p.sh_info(), 3);
}

#[test]
fn s81_values() {
    let (syms, secs, addrs) = program();
    let c = cx(&syms, &secs, None);
    let p = plan(&c, &[]).unwrap();
    let (sym, str) = write_all(&p, &c, &addrs).unwrap();
    let es = entries(&sym, &str);
    let by = |n: &str| es.iter().find(|e| e.name == n).unwrap();
    assert_eq!((by("main").shndx, by("main").value), (1, 0x10110));
    assert_eq!((by("tl").shndx, by("tl").value), (2, 8), "TLS: the offset in the template");
    assert_eq!(
        (by("com").info & 0xf, by("com").value),
        (STT_OBJECT, 0x1f050),
        "common: STT_OBJECT in .bss"
    );
    assert_eq!((by("weak_ref").shndx, by("weak_ref").value), (0, 0));
    assert_eq!((by("a.c").shndx, by("a.c").info), (0xfff1, STT_FILE), "STT_FILE is SHN_ABS");
    assert_eq!(by("_end").value, 0x20000, "S-122: the end of a section is in it");
}

#[test]
fn s81_finding1_ifunc_is_kept_at_its_resolver() {
    let (syms, secs, addrs) = program();
    let c = cx(&syms, &secs, None);
    let p = plan(&c, &[]).unwrap();
    let (sym, str) = write_all(&p, &c, &addrs).unwrap();
    let es = entries(&sym, &str);
    let f = es.iter().find(|e| e.name == "ifn").unwrap();
    assert_eq!((f.info & 0xf, f.value, f.size), (STT_GNU_IFUNC, 0x10140, 8));
}

#[test]
fn s83_finding15_a_file_without_stt_file_gets_none() {
    let (syms, secs, addrs) = program();
    let c = cx(&syms, &secs, None);
    let p = plan(&c, &[]).unwrap();
    let (sym, str) = write_all(&p, &c, &addrs).unwrap();
    let files = entries(&sym, &str).iter().filter(|e| e.info & 0xf == STT_FILE).count();
    assert_eq!(files, 1);
}

#[test]
fn s80_thunk_and_patch_symbols() {
    let (syms, secs, addrs) = program();
    let main = SymKey::global(SymbolId(0));
    let patches = [
        Patch {
            chunk: ChunkId(9),
            offset: 0,
            kind: PatchKind::Thunk(ThunkTarget::Sym { key: main, addend: 0x10 }),
        },
        Patch {
            chunk: ChunkId(9),
            offset: 16,
            kind: PatchKind::Thunk(ThunkTarget::Sym { key: main, addend: -8 }),
        },
        Patch { chunk: ChunkId(9), offset: 32, kind: PatchKind::Thunk(ThunkTarget::Plt(main)) },
        Patch {
            chunk: ChunkId(9),
            offset: 48,
            kind: PatchKind::Erratum {
                erratum: Erratum::E843419,
                section: SectionId(1),
                offset: 0xff8,
            },
        },
    ];
    let c = cx(&syms, &secs, None);
    let p = plan(&c, &patches).unwrap();
    let (sym, str) = write_all(&p, &c, &addrs).unwrap();
    let es = entries(&sym, &str);
    assert_eq!(
        &names(&es)[2..10],
        [
            "$x",
            "__main+0x10_veneer",
            "$x",
            "__main-0x8_veneer",
            "$x",
            "__main@plt_veneer",
            "$x",
            "__erratum_843419_veneer"
        ]
    );
    assert_eq!((es[3].info, es[3].size, es[3].value), (STT_FUNC, 12, 0x1e000));
    assert_eq!(es[9].size, 8);
    assert_eq!(p.sh_info(), 16, "null, the linker block of 1 + 8, and 4 + 2 file locals");
}

#[test]
fn s86_strtab_fixed_table_then_names() {
    let (syms, secs, addrs) = program();
    let c = cx(&syms, &secs, None);
    let p = plan(&c, &[]).unwrap();
    let (_, str) = write_all(&p, &c, &addrs).unwrap();
    assert!(str.starts_with(fixed_table()));
    assert_eq!(
        &str[fixed_table().len()..],
        b"_GLOBAL_OFFSET_TABLE_\0a.c\0s1\0tl\0s2\0hid\0main\0weak_ref\0com\0ifn\0_end\0__start_s\0"
    );
    assert_eq!(p.strtab_size() as usize, str.len());
}

#[test]
fn s122_a_value_outside_its_section_is_an_internal_error() {
    let (syms, secs, mut addrs) = program();
    addrs.internal.insert(8, InternalValue::At { shndx: 1, addr: 0x20001 });
    let c = cx(&syms, &secs, None);
    let p = plan(&c, &[]).unwrap();
    let e = write_all(&p, &c, &addrs).unwrap_err();
    assert!(
        e.contains("symbol _end: value 0x20001 is outside section 1 (spec/synth.md S-122)"),
        "{e}"
    );
}

#[test]
fn s124_a_linker_symbol_without_a_value_is_an_internal_error() {
    let (syms, secs, mut addrs) = program();
    addrs.internal.remove(&8);
    let c = cx(&syms, &secs, None);
    let p = plan(&c, &[]).unwrap();
    let e = write_all(&p, &c, &addrs).unwrap_err();
    assert!(e.contains("linker-defined symbol _end has no value (spec/synth.md S-124)"), "{e}");
}

#[test]
fn s85_large_section_indices_use_shn_xindex() {
    let (mut syms, secs, mut addrs) = program();
    syms.files[2].push(local(b"far", STT_OBJECT, in_sec(800, 4)));
    addrs.high = Some(0xff10);
    let c = cx(&syms, &secs, None);
    let p = plan(&c, &[]).unwrap();
    let (sym, str) = write_all(&p, &c, &addrs).unwrap();
    let es = entries(&sym, &str);
    let at = es.iter().position(|e| e.name == "far").unwrap();
    assert_eq!(es[at].shndx, 0xffff);
    let mut shndx = vec![0xaa; p.shndx_size() as usize];
    p.write_shndx(&c, &addrs, &mut Window::new(0, &mut shndx)).unwrap();
    let w = |i: usize| u32::from_le_bytes(shndx[4 * i..4 * i + 4].try_into().unwrap());
    assert_eq!(w(at), 0xff10);
    assert_eq!(w(1), 0, "a small index is 0 in the extension");
    assert!(SymtabPlan::needs_shndx(0xff01) && !SymtabPlan::needs_shndx(0xff00));
}

/// A program of `n` locals per file over `files` files and `g` globals.
fn big(files: u32, n: u32, g: u32) -> Syms {
    const NAMES: [&[u8]; 6] = [b"alpha", b".Lx", b"beta_long_name", b"$x", b"g", b"zeta"];
    let mut s = Syms { files: vec![vec![]], globals: Vec::new() };
    for f in 1..=files {
        s.files.push(
            (0..n)
                .map(|i| {
                    local(
                        NAMES[((i + f) % 6) as usize],
                        STT_FUNC,
                        in_sec(1 + (i % 50), u64::from(i % 7)),
                    )
                })
                .collect(),
        );
    }
    for i in 0..g {
        let other = if i % 5 == 0 { STV_HIDDEN } else { 0 };
        s.globals.push(Some(global(
            NAMES[(i % 6) as usize],
            STT_FUNC,
            STB_GLOBAL,
            other,
            in_sec(1 + i % 60, 0),
            1 + i % files,
        )));
    }
    s
}

#[test]
fn s4_i104_sub_range_writes_equal_the_whole() {
    let syms = big(3, 2500, 4100);
    let secs = Secs::default();
    let addrs = Addrs::default();
    let c = cx(&syms, &secs, None);
    let p = plan(&c, &[]).unwrap();
    let (sym, str) = write_all(&p, &c, &addrs).unwrap();
    for step in [24 * 7 + 5, 4096, 24 * 1024, 100_000] {
        let mut s2 = vec![0xaa; sym.len()];
        for (k, part) in s2.chunks_mut(step).enumerate() {
            p.write_symtab(&c, &addrs, &mut Window::new((k * step) as u64, part)).unwrap();
        }
        assert!(s2 == sym, ".symtab split by {step}");
        let mut t2 = vec![0xaa; str.len()];
        for (k, part) in t2.chunks_mut(step).enumerate() {
            p.write_strtab(&c, &mut Window::new((k * step) as u64, part)).unwrap();
        }
        assert!(t2 == str, ".strtab split by {step}");
    }
}

#[test]
fn symtab_does_not_depend_on_the_schedule() {
    let syms = big(5, 700, 3000);
    let secs = Secs::default();
    let addrs = Addrs::default();
    check_schedules(|pool| {
        let c = cx(&syms, &secs, None);
        let diag = collector();
        let p = SymtabPlan::new(&c, Class::Elf64, &[], FACTS, pool, &diag).unwrap();
        let (sym, str) = write_all(&p, &c, &addrs).unwrap();
        (p.symtab_locals().clone(), sym, str)
    });
}

#[test]
fn elf32_entries_are_16_bytes() {
    let (syms, secs, addrs) = program();
    let c = cx(&syms, &secs, None);
    let diag = collector();
    let p = SymtabPlan::new(&c, Class::Elf32, &[], FACTS, &Pool::sequential(), &diag).unwrap();
    assert_eq!(p.symtab_size(), 16 * u64::from(p.entries()));
    let mut sym = vec![0; p.symtab_size() as usize];
    p.write_symtab(&c, &addrs, &mut Window::new(0, &mut sym)).unwrap();
    // `main` is entry 8: st_value at +4, st_info at +12.
    assert_eq!(u32::from_le_bytes(sym[8 * 16 + 4..8 * 16 + 8].try_into().unwrap()), 0x10110);
    assert_eq!(sym[8 * 16 + 12], STB_GLOBAL << 4 | STT_FUNC);
}
