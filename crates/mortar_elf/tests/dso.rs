//! L0 tests of the shared-object reader (`spec/elf.md` §6): every accessor
//! on a written sample in both classes and every hash style, both entry
//! paths giving the same view, and each error case. Named `s6_N_...` for
//! the section of the spec they check.

mod support;

use mortar_elf::consts::{
    DF_1_NOW, DF_BIND_NOW, DF_STATIC_TLS, DT_DEBUG, DT_GNU_HASH, DT_HASH, DT_NEEDED, DT_NULL,
    DT_SONAME, DT_STRSZ, DT_STRTAB, DT_SYMENT, DT_SYMTAB, DT_VERDEF, DT_VERDEFNUM, DT_VERSYM,
    EM_386, EM_X86_64, ET_EXEC, ET_REL, GNU_PROPERTY_AARCH64_FEATURE_1_AND,
    GNU_PROPERTY_X86_FEATURE_1_AND, NT_GNU_PROPERTY_TYPE_0, PF_R, PF_W, PF_X, PN_XNUM, PT_DYNAMIC,
    PT_GNU_PROPERTY, PT_GNU_RELRO, PT_GNU_STACK, PT_LOAD, PT_NOTE, PT_NULL, PT_TLS, SHT_DYNAMIC,
    STB_LOCAL, VER_FLG_BASE, VER_FLG_WEAK,
};
use mortar_elf::{
    Class, DYNAMIC_STRINGS, Entry, Error, NotePart, SharedObject, SymbolIndex, SymbolSection,
    Version, VersionFault, VersionRecord,
};
use support::dso::{
    DATA_BIAS, Dso, DsoView, HASH_STYLES, HashStyle, PH_DATA, PH_DYNAMIC, PH_NOTE, PH_PROPERTY,
    PH_RELRO, PH_STACK, PH_TEXT, PH_TLS, SYM_BAR, SYM_BAZ, SYM_FOO_1, SYM_FOO_2, SYM_HID, SYM_PROT,
    SYM_SECTION, SYM_TLS, SYM_WEAKREF, SYMBOLS, SYMOFFSET, VerView, dso_sample, dso_view,
    strip_sections, walk_dso,
};
use support::{TARGETS, sample};

/// Every sample: three targets × three hash styles.
fn samples() -> impl Iterator<Item = Dso> {
    TARGETS.into_iter().flat_map(|(class, machine)| {
        HASH_STYLES.into_iter().map(move |hash| dso_sample(class, machine, hash))
    })
}

fn x86_64() -> Dso {
    dso_sample(Class::Elf64, EM_X86_64, HashStyle::Both)
}

fn i386() -> Dso {
    dso_sample(Class::Elf32, EM_386, HashStyle::Both)
}

/// The sample, and its section-stripped form, each parsed by both paths
/// where the path can see anything.
fn both_paths(bytes: &[u8]) -> [SharedObject<'_>; 2] {
    [
        SharedObject::parse_by(bytes, Entry::Sections).expect("parse"),
        SharedObject::parse_by(bytes, Entry::Dynamic).expect("parse"),
    ]
}

// ---- §6.1 identification ------------------------------------------------------

/// §6.1: an `ET_DYN` file of each machine and class parses; the entry path
/// is the section headers when the file has them and the dynamic table
/// when it has none, unless the caller says which.
#[test]
fn s6_1_parse_identifies_the_file_and_the_entry_path() {
    for dso in samples() {
        let object = SharedObject::parse(&dso.bytes).expect("parse");
        assert_eq!(object.class(), dso.class);
        assert_eq!(object.entry(), Entry::Sections);
        assert_eq!(object.data(), &dso.bytes[..]);
        assert_eq!(object.header().ehdr.e_type, 3);
        assert_eq!(object.section_table().len(), dso.shnum);
        let stripped = dso.stripped();
        let object = SharedObject::parse(&stripped).expect("parse");
        assert_eq!(object.entry(), Entry::Dynamic);
        assert!(object.section_table().is_empty());
        let forced = SharedObject::parse_by(&dso.bytes, Entry::Dynamic).expect("parse");
        assert_eq!(forced.entry(), Entry::Dynamic);
    }
}

/// §6.1: a relocatable or executable file is not a shared object; header
/// errors are the object reader's.
#[test]
fn s6_1_other_file_types_are_refused() {
    for (class, machine) in TARGETS {
        let object = sample(class, machine);
        assert_eq!(
            SharedObject::parse(&object.bytes).err(),
            Some(Error::NotSharedObject { e_type: ET_REL })
        );
        let mut dso = dso_sample(class, machine, HashStyle::Gnu);
        dso.set_ehdr(|e| e.e_type = ET_EXEC);
        assert_eq!(
            SharedObject::parse(&dso.bytes).err(),
            Some(Error::NotSharedObject { e_type: ET_EXEC })
        );
    }
    assert_eq!(SharedObject::parse(b"\x7fELG").err(), Some(Error::NotElf));
    let mut dso = x86_64();
    dso.set_ehdr(|e| e.e_machine = 40);
    assert_eq!(
        SharedObject::parse(&dso.bytes).err(),
        Some(Error::UnsupportedMachine { e_machine: 40 })
    );
}

// ---- §6.2 program headers -----------------------------------------------------

/// §6.2: every program header decodes as written, in both classes, with the
/// accessors over it.
#[test]
fn s6_2_program_headers_decode() {
    for dso in samples() {
        let object = SharedObject::parse(&dso.bytes).expect("parse");
        let segments = object.program_headers();
        assert_eq!(segments.len(), dso.phnum);
        assert_eq!(segments.class(), dso.class);
        let types: Vec<u32> = segments.iter().map(|s| s.p_type()).collect();
        assert_eq!(
            types,
            [
                PT_LOAD,
                PT_LOAD,
                PT_DYNAMIC,
                PT_NOTE,
                PT_GNU_PROPERTY,
                PT_TLS,
                PT_GNU_RELRO,
                PT_GNU_STACK
            ]
        );
        let text = segments.get(PH_TEXT).expect("text");
        assert_eq!(text.index(), PH_TEXT);
        assert_eq!(text.flags(), PF_R | PF_X);
        assert!(text.is_readable() && text.is_executable() && !text.is_writable());
        assert_eq!(text.alignment(), Ok(0x1000));
        assert_eq!(text.data().expect("data"), &dso.bytes[..text.header().p_filesz as usize]);
        let data = segments.get(PH_DATA).expect("data");
        assert!(data.is_writable());
        assert_eq!(data.header().p_vaddr, data.header().p_offset + DATA_BIAS);
        assert!(data.header().p_memsz > data.header().p_filesz);
        let dynamic = segments.unique(PT_DYNAMIC).expect("unique").expect("PT_DYNAMIC");
        assert_eq!(dynamic.index(), PH_DYNAMIC);
        assert_eq!(dynamic.header().p_offset as usize, dso.part(".dynamic").offset);
        let tls = segments.unique(PT_TLS).expect("unique").expect("PT_TLS");
        assert_eq!((tls.index(), tls.header().p_filesz, tls.header().p_memsz), (PH_TLS, 8, 16));
        assert_eq!(
            segments.unique(PT_GNU_RELRO).expect("unique").map(|s| s.index()),
            Some(PH_RELRO)
        );
        let stack = segments.get(PH_STACK).expect("stack");
        assert_eq!((stack.flags(), stack.alignment()), (PF_R | PF_W, Ok(16)));
        assert!(matches!(segments.unique(PT_NULL), Ok(None)));
        assert_eq!(
            segments.unique(PT_LOAD).err(),
            Some(Error::MultipleSegments { p_type: PT_LOAD, first: 0, second: 1 })
        );
        assert_eq!(
            segments.get(dso.phnum).err(),
            Some(Error::SegmentIndexOutOfRange { index: dso.phnum, count: dso.phnum })
        );
        assert_eq!(segments.iter().len(), dso.phnum as usize);
        assert_eq!((&segments).into_iter().count(), dso.phnum as usize);
    }
}

/// §6.2: addresses map to file offsets through the `PT_LOAD` segments.
#[test]
fn s6_2_addresses_map_through_the_load_segments() {
    for dso in samples() {
        let object = SharedObject::parse(&dso.bytes).expect("parse");
        let segments = object.program_headers();
        let dynamic = dso.part(".dynamic");
        assert_eq!(segments.load_containing(dynamic.addr).map(|s| s.index()), Some(PH_DATA));
        assert_eq!(
            segments.file_range(dynamic.addr, dynamic.size as u64),
            Some(dynamic.offset..dynamic.offset + dynamic.size)
        );
        // The writable segment's memory runs past its file contents (.bss):
        // such an address is in the segment, but has no file bytes.
        let bss = dso.part(".bss");
        assert_eq!(segments.load_containing(bss.addr).map(|s| s.index()), Some(PH_DATA));
        assert_eq!(segments.file_range(bss.addr, 1), None);
        assert_eq!(segments.file_bytes_from(bss.addr), None);
        // A range must lie wholly in one segment's file contents.
        let text_end = segments.get(PH_TEXT).expect("text").header().p_filesz;
        assert_eq!(segments.file_range(text_end - 4, 8), None);
        assert_eq!(segments.file_range(u64::MAX, 2), None);
        let (offset, bytes) = segments.file_bytes_from(dynamic.addr).expect("mapped");
        assert_eq!(offset as usize, dynamic.offset);
        let data = *segments.get(PH_DATA).expect("data").header();
        assert_eq!(bytes.len() as u64, data.p_offset + data.p_filesz - offset);
        assert!(segments.load_containing(u64::MAX).is_none());
        assert!(segments.get(PH_DATA).expect("data").contains(data.p_vaddr + data.p_memsz - 1));
        assert!(!segments.get(PH_DATA).expect("data").contains(data.p_vaddr + data.p_memsz));
    }
}

/// §6.2: `e_phnum` = `PN_XNUM` takes the count from section 0's `sh_info`;
/// a file without sections cannot use the escape.
#[test]
fn s6_2_the_phnum_escape() {
    let mut dso = x86_64();
    dso.set_ehdr(|e| e.e_phnum = PN_XNUM);
    dso.set_shdr(0, |s| s.sh_info = 8);
    let object = SharedObject::parse(&dso.bytes).expect("parse");
    assert_eq!(object.program_headers().len(), 8);
    let stripped = strip_sections(&x86_64().bytes);
    let mut bytes = stripped.clone();
    bytes[56..58].copy_from_slice(&PN_XNUM.to_le_bytes());
    assert_eq!(SharedObject::parse(&bytes).err(), Some(Error::PhnumEscapeWithoutSections));
}

/// §6.2: errors in the program header table fail the parse; errors in one
/// segment are that segment's accessor's.
#[test]
fn s6_2_program_header_errors() {
    for dso in [x86_64(), i386()] {
        let phentsize = if dso.class == Class::Elf32 { 32 } else { 56 };
        let mut bad = dso.clone();
        bad.set_ehdr(|e| e.e_phentsize = 40);
        assert_eq!(
            SharedObject::parse(&bad.bytes).err(),
            Some(Error::BadPhentsize { e_phentsize: 40, expected: phentsize })
        );
        let mut bad = dso.clone();
        bad.set_ehdr(|e| e.e_phnum = 0xfff0);
        assert_eq!(
            SharedObject::parse(&bad.bytes).err(),
            Some(Error::ProgramHeadersOutOfBounds {
                offset: dso.phoff as u64,
                count: 0xfff0,
                file_len: dso.bytes.len() as u64
            })
        );
        let mut bad = dso.clone();
        bad.set_ehdr(|e| e.e_phoff = 0);
        assert_eq!(
            SharedObject::parse(&bad.bytes).err(),
            Some(Error::MissingProgramHeaderTable { e_phnum: 8 })
        );
        // No program headers at all: a file with nothing to find through
        // them.
        let mut bad = dso.clone();
        bad.set_ehdr(|e| {
            e.e_phoff = 0;
            e.e_phnum = 0;
        });
        let object = SharedObject::parse(&bad.bytes).expect("parse");
        assert!(object.program_headers().is_empty());
        assert!(matches!(object.gnu_property_note(), Ok(None)));

        let mut bad = dso.clone();
        bad.set_phdr(PH_DATA, |p| p.p_filesz = u64::from(u32::MAX));
        let object = SharedObject::parse(&bad.bytes).expect("parse");
        let data = object.program_headers().get(PH_DATA).expect("segment");
        assert!(matches!(data.data(), Err(Error::SegmentOutOfBounds { segment: PH_DATA, .. })));
        assert_eq!(data.data().err().and_then(|e| e.segment()), Some(PH_DATA));

        let mut bad = dso.clone();
        bad.set_phdr(PH_TEXT, |p| p.p_align = 3);
        let object = SharedObject::parse(&bad.bytes).expect("parse");
        let text = object.program_headers().get(PH_TEXT).expect("segment");
        assert_eq!(
            text.alignment(),
            Err(Error::BadSegmentAlignment { segment: PH_TEXT, align: 3 })
        );
        bad.set_phdr(PH_TEXT, |p| p.p_align = 0);
        let object = SharedObject::parse(&bad.bytes).expect("parse");
        assert_eq!(object.program_headers().get(PH_TEXT).expect("segment").alignment(), Ok(1));

        let mut bad = dso.clone();
        bad.set_phdr(PH_PROPERTY, |p| p.p_type = PT_DYNAMIC);
        let object = SharedObject::parse(&bad.bytes).expect("parse");
        assert!(object.dynamic().is_ok());
        // The section path does not read PT_DYNAMIC; the dynamic path does.
        let object = SharedObject::parse_by(&bad.bytes, Entry::Dynamic).expect("parse");
        assert_eq!(
            object.dynamic().err(),
            Some(Error::MultipleSegments {
                p_type: PT_DYNAMIC,
                first: PH_DYNAMIC,
                second: PH_PROPERTY
            })
        );
        assert_eq!(object.symbols().err(), object.dynamic().err());
    }
}

// ---- §6.3 the two entry paths -------------------------------------------------

/// §6.3: the dynamic table alone gives the view the section headers give,
/// in every target and hash style, and so does the section-stripped file.
#[test]
fn s6_3_both_entry_paths_give_one_view() {
    for dso in samples() {
        let by_sections = dso_view(&dso.bytes, Some(Entry::Sections)).expect("sections");
        let by_dynamic = dso_view(&dso.bytes, Some(Entry::Dynamic)).expect("dynamic");
        assert_eq!(by_sections, by_dynamic);
        let stripped = dso_view(&dso.stripped(), None).expect("stripped");
        assert_eq!(by_sections, stripped);
        assert_eq!(by_sections.symbols.len(), SYMBOLS as usize);
    }
}

/// §6.3: when the file has section headers, they are what is read: a
/// damaged dynamic-table address does not change the view, and damaged
/// section headers do (and the dynamic path does not see them).
#[test]
fn s6_3_section_headers_win_when_present() {
    for dso in [x86_64(), i386()] {
        let good = dso_view(&dso.bytes, None).expect("view");
        let mut bad = dso.clone();
        let symtab = bad.dyn_index(DT_SYMTAB);
        bad.set_dyn(symtab, |d| d.d_val = 0x7fff_0000);
        let view = dso_view(&bad.bytes, None).expect("view");
        assert_ne!(view.dynamic, good.dynamic);
        assert_eq!(DsoView { dynamic: good.dynamic.clone(), ..view }, good);
        assert_eq!(
            SharedObject::parse_by(&bad.bytes, Entry::Dynamic).expect("parse").symbols().err(),
            Some(Error::AddressNotMapped {
                d_tag: DT_SYMTAB,
                addr: 0x7fff_0000,
                size: u64::from(SYMBOLS) * support::sym_size(dso.class)
            })
        );

        let mut bad = dso.clone();
        let dynsym = bad.part(".dynsym").index;
        bad.set_shdr(dynsym, |s| s.sh_size = u64::MAX / 2);
        assert!(SharedObject::parse(&bad.bytes).expect("parse").symbols().is_err());
        assert_eq!(dso_view(&bad.bytes, Some(Entry::Dynamic)).as_ref(), Ok(&good));
    }
}

/// §6.3: through the section headers, a file without them has no parts.
#[test]
fn s6_3_a_file_without_sections_has_no_parts_by_sections() {
    let stripped = x86_64().stripped();
    let object = SharedObject::parse_by(&stripped, Entry::Sections).expect("parse");
    assert_eq!(object.entry(), Entry::Sections);
    assert!(object.dynamic().expect("dynamic").is_none());
    assert!(object.symbols().expect("symbols").is_none());
    assert!(object.versym().expect("versym").is_none());
    assert!(object.version_definitions().expect("verdef").is_none());
    assert!(object.version_needs().expect("verneed").is_none());
    // The program headers are read on both paths.
    assert!(object.gnu_property_note().expect("note").is_some());
}

/// §6.3: a file with neither a `PT_DYNAMIC` nor sections has no parts.
#[test]
fn s6_3_no_dynamic_segment_no_parts() {
    let mut dso = x86_64();
    dso.set_phdr(PH_DYNAMIC, |p| p.p_type = PT_NULL);
    let stripped = dso.stripped();
    let object = SharedObject::parse(&stripped).expect("parse");
    assert!(matches!(object.dynamic(), Ok(None)));
    assert!(object.symbols().expect("symbols").is_none());
    assert!(object.versions().expect("versions").versym().is_none());
}

// ---- §6.4 the dynamic table ---------------------------------------------------

/// §6.4: the entries up to the first `DT_NULL`, and the typed accessors.
#[test]
fn s6_4_the_dynamic_table() {
    for dso in samples() {
        for object in both_paths(&dso.bytes) {
            let table = object.dynamic().expect("dynamic").expect("a table");
            assert_eq!(table.offset() as usize, dso.part(".dynamic").offset);
            // Three trailing DT_NULLs are written; the table ends at the
            // first.
            assert_eq!(table.len(), dso.dynamic_entries - 2);
            assert_eq!(table.iter().next_back().map(|d| d.d_tag), Some(DT_NULL));
            assert_eq!(table.entries().len(), table.len());
            assert!(!table.is_empty());
            let needed: Vec<&[u8]> = table.needed().map(|n| n.expect("name")).collect();
            assert_eq!(needed, [&b"libc.so.6"[..], b"libm.so.6"]);
            assert_eq!(table.soname(), Ok(Some(&b"libt.so"[..])));
            assert_eq!(table.rpath(), Ok(Some(&b"/opt/rpath"[..])));
            assert_eq!(table.runpath(), Ok(Some(&b"/opt/runpath"[..])));
            assert_eq!(table.flags(), Ok(DF_BIND_NOW | DF_STATIC_TLS));
            assert_eq!(table.flags_1(), Ok(DF_1_NOW));
            assert_eq!(table.value(DT_DEBUG), Ok(None));
            assert_eq!(table.value(DT_SYMTAB), Ok(Some(dso.part(".dynsym").addr)));
            assert_eq!(table.value(DT_STRTAB), Ok(Some(dso.part(".dynstr").addr)));
            assert_eq!(table.value(DT_SYMENT), Ok(Some(support::sym_size(dso.class))));
            assert_eq!(
                table.value(DT_NEEDED),
                Err(Error::DuplicateDynamicTag { d_tag: DT_NEEDED, first: 0, second: 1 })
            );
            assert_eq!(table.values(DT_NEEDED).count(), 2);
            assert_eq!(table.string(DT_DEBUG), Ok(None));
            let strings = table.strings().expect("strings").expect("a string table");
            let size = strings.bytes().len() as u64;
            assert!(matches!(
                table.string_at(DT_NEEDED, size),
                Err(Error::StringOutOfBounds { offset, .. }) if u64::from(offset) == size
            ));
            assert!(matches!(
                table.string_at(DT_NEEDED, u64::MAX),
                Err(Error::StringOutOfBounds { offset: u32::MAX, .. })
            ));
            assert_eq!((&table).into_iter().count(), table.len());
        }
    }
}

/// §6.4: the dynamic path's string table is not a section: its errors say
/// so, and name no section.
#[test]
fn s6_4_dynamic_strings_are_not_a_section() {
    let dso = x86_64();
    let object = SharedObject::parse_by(&dso.bytes, Entry::Dynamic).expect("parse");
    let strings =
        object.dynamic().expect("dynamic").expect("table").strings().expect("s").expect("s");
    assert_eq!(strings.section(), DYNAMIC_STRINGS);
    let error = strings.get(10_000).expect_err("out of bounds");
    assert_eq!(error.section(), None);
    assert!(error.to_string().contains("DT_STRTAB"), "{error}");
    let object = SharedObject::parse_by(&dso.bytes, Entry::Sections).expect("parse");
    let strings =
        object.dynamic().expect("dynamic").expect("table").strings().expect("s").expect("s");
    assert_eq!(strings.section(), dso.part(".dynstr").index);
    assert_eq!(strings.get(10_000).expect_err("out of bounds").section(), Some(strings.section()));
}

/// §6.4: errors of the dynamic table are its accessors' errors; the file
/// still parses.
#[test]
fn s6_4_dynamic_table_errors() {
    for dso in [x86_64(), i386()] {
        let entries = dso.dynamic_entries as u64;
        // No DT_NULL.
        let mut bad = dso.clone();
        for i in 0..dso.dynamic_entries {
            bad.set_dyn(i, |d| {
                if d.d_tag == DT_NULL {
                    d.d_tag = DT_DEBUG;
                }
            });
        }
        for object in both_paths(&bad.bytes) {
            assert_eq!(object.dynamic().err(), Some(Error::DynamicNotTerminated { entries }));
        }
        // The dynamic path: DT_STRSZ missing, DT_STRTAB not mapped.
        let mut bad = dso.clone();
        let strsz = bad.dyn_index(DT_STRSZ);
        bad.set_dyn(strsz, |d| d.d_tag = DT_DEBUG);
        let object = SharedObject::parse_by(&bad.bytes, Entry::Dynamic).expect("parse");
        let missing = Error::MissingDynamicTag { d_tag: DT_STRSZ, needed_by: DT_STRTAB };
        let table = object.dynamic().expect("the table is still there").expect("table");
        assert_eq!(table.strings().err(), Some(missing));
        assert_eq!(table.soname().err(), Some(missing));
        assert_eq!(object.symbols().err(), Some(missing));
        let mut bad = dso.clone();
        let strtab = bad.dyn_index(DT_STRTAB);
        bad.set_dyn(strtab, |d| d.d_val = 0x7fff_0000);
        let object = SharedObject::parse_by(&bad.bytes, Entry::Dynamic).expect("parse");
        assert!(matches!(
            object.dynamic().expect("table").expect("table").strings(),
            Err(Error::AddressNotMapped { d_tag: DT_STRTAB, addr: 0x7fff_0000, .. })
        ));
        // No DT_STRTAB: no string table, and the string-valued tags say so.
        let mut bad = dso.clone();
        bad.set_dyn(strtab, |d| d.d_tag = DT_DEBUG);
        let object = SharedObject::parse_by(&bad.bytes, Entry::Dynamic).expect("parse");
        let table = object.dynamic().expect("table").expect("table");
        assert!(matches!(table.strings(), Ok(None)));
        assert_eq!(
            table.soname(),
            Err(Error::MissingDynamicTag { d_tag: DT_STRTAB, needed_by: DT_SONAME })
        );
        assert_eq!(
            table.needed().next(),
            Some(Err(Error::MissingDynamicTag { d_tag: DT_STRTAB, needed_by: DT_NEEDED }))
        );
        assert_eq!(
            object.symbols().err(),
            Some(Error::MissingDynamicTag { d_tag: DT_STRTAB, needed_by: DT_SYMTAB })
        );
        // The section path: entry size, and a second SHT_DYNAMIC.
        let dynamic = dso.part(".dynamic").index;
        let mut bad = dso.clone();
        bad.set_shdr(dynamic, |s| s.sh_entsize = 3);
        let object = SharedObject::parse(&bad.bytes).expect("parse");
        assert!(
            matches!(object.dynamic(), Err(Error::BadEntsize { section, entsize: 3, .. }) if section == dynamic)
        );
        let mut bad = dso.clone();
        let data = dso.part(".data").index;
        bad.set_shdr(data, |s| s.sh_type = SHT_DYNAMIC);
        let object = SharedObject::parse(&bad.bytes).expect("parse");
        assert_eq!(
            object.dynamic().err(),
            Some(Error::MultipleSections { sh_type: SHT_DYNAMIC, first: dynamic, second: data })
        );
        // The other parts do not depend on the dynamic section.
        assert!(object.symbols().expect("symbols").is_some());
    }
}

// ---- §6.5 dynamic symbols -----------------------------------------------------

/// §6.5: the dynamic symbols, their names and sections, and the split
/// between locals and globals, by both paths.
#[test]
fn s6_5_dynamic_symbols() {
    for dso in samples() {
        for object in both_paths(&dso.bytes) {
            let symbols = object.symbols().expect("symbols").expect("a table");
            assert_eq!(symbols.len(), SYMBOLS);
            assert_eq!(symbols.first_global(), SymbolIndex(SYM_BAZ));
            assert_eq!(symbols.locals().len(), 2);
            assert_eq!(symbols.globals().len(), SYMBOLS as usize - 2);
            assert!(!symbols.has_extended_indexes());
            let names: Vec<&[u8]> =
                symbols.iter().map(|(_, sym)| symbols.name(&sym).expect("name")).collect();
            assert_eq!(
                names,
                [
                    &b""[..],
                    b"",
                    b"baz",
                    b"weakref",
                    b"foo",
                    b"foo",
                    b"bar",
                    b"hid",
                    b"tls",
                    b"prot"
                ]
            );
            let section = |index: u32| {
                let sym = symbols.get(SymbolIndex(index)).expect("symbol");
                symbols.section_of(SymbolIndex(index), &sym).expect("section")
            };
            let rodata = mortar_elf::SectionIndex(dso.part(".rodata").index);
            assert_eq!(section(SYM_SECTION), SymbolSection::Section(rodata));
            assert_eq!(section(SYM_BAZ), SymbolSection::Undefined);
            assert_eq!(section(SYM_FOO_2), SymbolSection::Section(rodata));
            let section_symbol = symbols.get(SymbolIndex(SYM_SECTION)).expect("symbol");
            assert_eq!(section_symbol.binding(), STB_LOCAL);
            let bar = symbols.get(SymbolIndex(SYM_BAR)).expect("bar");
            assert_eq!((bar.st_value, bar.st_size), (dso.part(".data").addr, 16));
            assert!(symbols.get(SymbolIndex(SYMBOLS)).is_err());
        }
    }
}

/// §6.5: the section path reads `.dynsym` and checks its section indexes;
/// the dynamic path cannot check them (there are no sections to count).
#[test]
fn s6_5_section_indexes_are_checked_only_against_sections() {
    let dso = x86_64();
    let mut bad = dso.clone();
    let at = bad.part(".dynsym").offset + SYM_FOO_2 as usize * 24 + 6;
    bad.bytes[at..at + 2].copy_from_slice(&0x0200u16.to_le_bytes());
    let object = SharedObject::parse(&bad.bytes).expect("parse");
    let symbols = object.symbols().expect("symbols").expect("table");
    let sym = symbols.get(SymbolIndex(SYM_FOO_2)).expect("foo");
    assert_eq!(
        symbols.section_of(SymbolIndex(SYM_FOO_2), &sym),
        Err(Error::SymbolSectionOutOfRange { symbol: SYM_FOO_2, index: 0x200, count: dso.shnum })
    );
    let object = SharedObject::parse_by(&bad.bytes, Entry::Dynamic).expect("parse");
    let symbols = object.symbols().expect("symbols").expect("table");
    assert_eq!(
        symbols.section_of(SymbolIndex(SYM_FOO_2), &sym),
        Ok(SymbolSection::Section(mortar_elf::SectionIndex(0x200)))
    );
}

/// §6.5: `DT_SYMENT`, when present, is the class's symbol size; the
/// dynamic path needs `DT_STRTAB` and a mapped `DT_SYMTAB`.
#[test]
fn s6_5_dynamic_symbol_errors() {
    for dso in [x86_64(), i386()] {
        let size = support::sym_size(dso.class);
        let mut bad = dso.clone();
        let syment = bad.dyn_index(DT_SYMENT);
        bad.set_dyn(syment, |d| d.d_val = 99);
        let object = SharedObject::parse_by(&bad.bytes, Entry::Dynamic).expect("parse");
        assert_eq!(
            object.symbols().err(),
            Some(Error::BadDynamicEntsize { d_tag: DT_SYMENT, value: 99, expected: size })
        );
        // Without DT_SYMENT, the class's size.
        bad.set_dyn(syment, |d| d.d_tag = DT_DEBUG);
        let object = SharedObject::parse_by(&bad.bytes, Entry::Dynamic).expect("parse");
        assert_eq!(object.symbols().expect("symbols").expect("table").len(), SYMBOLS);
        // Without DT_SYMTAB, no symbols; and so no version table either.
        let mut bad = dso.clone();
        let symtab = bad.dyn_index(DT_SYMTAB);
        bad.set_dyn(symtab, |d| d.d_tag = DT_DEBUG);
        let object = SharedObject::parse_by(&bad.bytes, Entry::Dynamic).expect("parse");
        assert!(matches!(object.symbols(), Ok(None)));
        assert_eq!(
            object.versym().err(),
            Some(Error::MissingDynamicTag { d_tag: DT_SYMTAB, needed_by: DT_VERSYM })
        );
    }
}

// ---- §6.6 hash tables ---------------------------------------------------------

/// §6.6: without sections the count comes from the hash tables: either
/// one gives it; both must agree; neither is an error.
#[test]
fn s6_6_the_symbol_count_from_the_hash_tables() {
    for (class, machine) in TARGETS {
        for hash in HASH_STYLES {
            let dso = dso_sample(class, machine, hash);
            let stripped = dso.stripped();
            let object = SharedObject::parse(&stripped).expect("parse");
            assert_eq!(object.symbols().expect("symbols").expect("table").len(), SYMBOLS);
        }
        let dso = dso_sample(class, machine, HashStyle::Both);
        // nchain one short: the tables disagree.
        let mut bad = dso.clone();
        let at = bad.part(".hash").offset + 4;
        bad.bytes[at..at + 4].copy_from_slice(&(SYMBOLS - 1).to_le_bytes());
        let object = SharedObject::parse_by(&bad.bytes, Entry::Dynamic).expect("parse");
        assert_eq!(
            object.symbols().err(),
            Some(Error::SymbolCountMismatch {
                hash: u64::from(SYMBOLS) - 1,
                gnu_hash: u64::from(SYMBOLS)
            })
        );
        // The section path does not read the hash tables.
        assert!(SharedObject::parse(&bad.bytes).expect("parse").symbols().is_ok());
        // Neither table.
        let mut bad = dso.clone();
        for tag in [DT_HASH, DT_GNU_HASH] {
            let index = bad.dyn_index(tag);
            bad.set_dyn(index, |d| d.d_tag = DT_DEBUG);
        }
        let object = SharedObject::parse_by(&bad.bytes, Entry::Dynamic).expect("parse");
        assert_eq!(object.symbols().err(), Some(Error::NoSymbolCount));
        // A table outside the file's mapped contents, or not well formed.
        let mut bad = dso.clone();
        let index = bad.dyn_index(DT_GNU_HASH);
        bad.set_dyn(index, |d| d.d_val = 0x7fff_0000);
        let object = SharedObject::parse_by(&bad.bytes, Entry::Dynamic).expect("parse");
        assert_eq!(
            object.symbols().err(),
            Some(Error::BadHashTable { d_tag: DT_GNU_HASH, addr: 0x7fff_0000 })
        );
        // A GNU table that hashes no symbol gives only a lower bound:
        // DT_HASH must state the count, and must meet the bound.
        for (style, symoffset, expected) in [
            (HashStyle::Gnu, SYMOFFSET, Err(Error::UnhashedSymbolCount { symoffset: SYMOFFSET })),
            (HashStyle::Both, SYMOFFSET, Ok(SYMBOLS)),
            (
                HashStyle::Both,
                SYMBOLS + 1,
                Err(Error::SymbolCountMismatch {
                    hash: u64::from(SYMBOLS),
                    gnu_hash: u64::from(SYMBOLS) + 1,
                }),
            ),
        ] {
            let mut empty = dso_sample(class, machine, style);
            let gnu = empty.part(".gnu.hash");
            let word = |at: usize| u32::from_le_bytes(empty.bytes[at..at + 4].try_into().unwrap());
            let (nbuckets, bloom) = (word(gnu.offset) as usize, word(gnu.offset + 8) as usize);
            let buckets = gnu.offset + 16 + bloom * class.word_size();
            empty.bytes[buckets..buckets + 4 * nbuckets].fill(0);
            empty.bytes[gnu.offset + 4..gnu.offset + 8].copy_from_slice(&symoffset.to_le_bytes());
            let object = SharedObject::parse_by(&empty.bytes, Entry::Dynamic).expect("parse");
            let count = object.symbols().map(|table| table.expect("table").len());
            assert_eq!(count, expected, "{style:?} symoffset {symoffset}");
        }
        let mut bad = dso.clone();
        let gnu = bad.part(".gnu.hash");
        let bucket = gnu.offset + 16 + class.word_size();
        bad.bytes[bucket..bucket + 4].copy_from_slice(&1u32.to_le_bytes());
        let object = SharedObject::parse_by(&bad.bytes, Entry::Dynamic).expect("parse");
        assert_eq!(
            object.symbols().err(),
            Some(Error::BadHashTable { d_tag: DT_GNU_HASH, addr: gnu.addr })
        );
    }
}

// ---- §6.7 symbol versions -----------------------------------------------------

fn ver(index: u16, hidden: bool, kind: char, name: Option<&str>, file: Option<&str>) -> VerView {
    VerView {
        index,
        hidden,
        kind,
        name: name.map(str::to_owned),
        file: file.map(str::to_owned),
        default: kind == 'D' && !hidden,
    }
}

/// §6.7: which version every dynamic symbol carries, and whether it is the
/// default, by both paths.
#[test]
fn s6_7_the_version_of_each_symbol() {
    let expected = [
        ver(0, false, 'L', None, None),
        ver(0, false, 'L', None, None),
        ver(4, false, 'N', Some("GLIBC_2.2"), Some("libc.so.6")),
        ver(5, false, 'N', Some("M_1"), Some("libm.so.6")),
        ver(2, true, 'D', Some("VER_1"), None),
        ver(3, false, 'D', Some("VER_2"), None),
        ver(1, false, 'G', None, None),
        ver(3, true, 'D', Some("VER_2"), None),
        ver(2, false, 'D', Some("VER_1"), None),
        ver(1, false, 'G', None, None),
    ];
    for dso in samples() {
        for entry in [Entry::Sections, Entry::Dynamic] {
            let view = dso_view(&dso.bytes, Some(entry)).expect("view");
            let versions: Vec<VerView> =
                view.symbols.iter().map(|s| s.version.clone().expect("a version")).collect();
            assert_eq!(versions, expected);
            assert!(versions[SYM_FOO_2 as usize].default && !versions[SYM_FOO_1 as usize].default);
            assert!(!versions[SYM_HID as usize].default);
            assert!(versions[SYM_TLS as usize].default);
            let _ = (SYM_WEAKREF, SYM_PROT);
        }
    }
}

/// §6.7: the definition and need chains, record by record.
#[test]
fn s6_7_the_definition_and_need_chains() {
    for dso in samples() {
        for object in both_paths(&dso.bytes) {
            let base = dso.part(".gnu.version_d").offset as u64;
            let definitions = object.version_definitions().expect("verdef").expect("chain");
            assert_eq!(definitions.declared(), 3);
            let definitions: Vec<_> = definitions.map(|d| d.expect("definition")).collect();
            assert_eq!(definitions.len(), 3);
            let offsets: Vec<u64> =
                definitions.iter().map(mortar_elf::VersionDefinition::offset).collect();
            assert_eq!(offsets, [base, base + 28, base + 56]);
            assert!(definitions[0].is_base() && !definitions[1].is_base());
            assert_eq!(definitions[0].flags(), VER_FLG_BASE);
            assert_eq!(definitions[0].name(), Ok(&b"libt.so"[..]));
            assert_eq!(
                definitions.iter().map(mortar_elf::VersionDefinition::index).collect::<Vec<_>>(),
                [1, 2, 3]
            );
            let names: Vec<&[u8]> =
                definitions[2].names().expect("names").map(|n| n.expect("n")).collect();
            assert_eq!(names, [&b"VER_2"[..], b"VER_1"]);
            assert_eq!(definitions[2].record().vd_hash, support::dso::elf_hash(b"VER_2"));
            assert!(!definitions[1].is_weak());

            let needs = object.version_needs().expect("verneed").expect("chain");
            assert_eq!(needs.declared(), 2);
            let needs: Vec<_> = needs.map(|n| n.expect("need")).collect();
            assert_eq!(needs[0].file(), Ok(&b"libc.so.6"[..]));
            assert_eq!(needs[1].file(), Ok(&b"libm.so.6"[..]));
            assert_eq!(needs[1].offset(), dso.part(".gnu.version_r").offset as u64 + 32);
            let entries: Vec<_> =
                needs[1].entries().expect("entries").map(|e| e.expect("e")).collect();
            assert_eq!(entries.len(), 1);
            assert_eq!((entries[0].index(), entries[0].flags()), (5, VER_FLG_WEAK));
            assert!(entries[0].is_weak());
            assert_eq!(entries[0].name(), Ok(&b"M_1"[..]));
            assert_eq!(entries[0].offset(), needs[1].offset() + 16);

            let versions = object.versions().expect("versions");
            assert_eq!(versions.versym().map(|t| t.len()), Some(SYMBOLS as usize));
            assert!(matches!(versions.version(SymbolIndex(0), 0), Ok(Version::Local)));
            assert!(matches!(versions.version(SymbolIndex(0), 1), Ok(Version::Global)));
            assert!(
                matches!(versions.version(SymbolIndex(0), 3), Ok(Version::Defined(d)) if d.index() == 3)
            );
            assert!(matches!(
                versions.version(SymbolIndex(0), 4),
                Ok(Version::Needed { need, entry }) if need.file() == Ok(b"libc.so.6") && entry.index() == 4
            ));
            assert_eq!(
                versions.of(SymbolIndex(SYMBOLS)).err(),
                Some(Error::SymbolIndexOutOfRange { index: SYMBOLS, count: SYMBOLS })
            );
            assert_eq!(object.versym().expect("versym").map(|t| t.len()), Some(SYMBOLS as usize));
        }
    }
}

/// §6.7: an object without versions: every symbol has `None`.
#[test]
fn s6_7_no_version_table_no_versions() {
    let dso = x86_64();
    let mut bad = dso.clone();
    let versym = bad.dyn_index(DT_VERSYM);
    bad.set_dyn(versym, |d| d.d_tag = DT_DEBUG);
    let stripped = bad.stripped();
    let object = SharedObject::parse(&stripped).expect("parse");
    assert!(object.versym().expect("versym").is_none());
    assert_eq!(
        object.versions().expect("versions").of(SymbolIndex(4)).expect("of").map(|v| v.versym),
        None
    );
}

/// §6.7: errors of the version table and chains.
#[test]
#[allow(clippy::too_many_lines)] // one case after another
fn s6_7_version_errors() {
    for dso in [x86_64(), i386()] {
        let versym = dso.part(".gnu.version").index;
        let dynsym = dso.part(".dynsym").index;
        // The version table must belong to .dynsym and match its count.
        let mut bad = dso.clone();
        bad.set_shdr(versym, |s| s.sh_link = dso.part(".dynstr").index);
        let object = SharedObject::parse(&bad.bytes).expect("parse");
        assert_eq!(
            object.versym().err(),
            Some(Error::VersymLink { section: versym, link: dso.part(".dynstr").index })
        );
        assert_eq!(object.versions().err(), object.versym().err());
        let mut bad = dso.clone();
        bad.set_shdr(versym, |s| s.sh_size = 18);
        let object = SharedObject::parse(&bad.bytes).expect("parse");
        assert_eq!(
            object.versym().err(),
            Some(Error::VersymCountMismatch { entries: 9, symbols: SYMBOLS })
        );
        let _ = dynsym;

        let verdef = dso.part(".gnu.version_d");
        let poke16 = |bytes: &mut Vec<u8>, at: usize, value: u16| {
            bytes[at..at + 2].copy_from_slice(&value.to_le_bytes());
        };
        let poke32 = |bytes: &mut Vec<u8>, at: usize, value: u32| {
            bytes[at..at + 4].copy_from_slice(&value.to_le_bytes());
        };
        for entry in [Entry::Sections, Entry::Dynamic] {
            let error_of = |bytes: &[u8]| -> Option<Error> {
                let object = SharedObject::parse_by(bytes, entry).expect("parse");
                let definitions = match object.version_definitions() {
                    Ok(definitions) => definitions.expect("chain"),
                    Err(error) => return Some(error),
                };
                for definition in definitions {
                    let definition = match definition {
                        Ok(definition) => definition,
                        Err(error) => return Some(error),
                    };
                    if let Err(error) = definition.name() {
                        return Some(error);
                    }
                    for name in definition.names().ok()? {
                        if let Err(error) = name {
                            return Some(error);
                        }
                    }
                }
                None
            };
            let bad_version = |record, offset: usize, fault| {
                Some(Error::BadVersion { record, offset: offset as u64, fault })
            };
            // vd_version 2.
            let mut bad = dso.clone();
            poke16(&mut bad.bytes, verdef.offset + 28, 2);
            assert_eq!(
                error_of(&bad.bytes),
                bad_version(
                    VersionRecord::Definition,
                    verdef.offset + 28,
                    VersionFault::Revision(2)
                )
            );
            // vd_next 0 before the last.
            let mut bad = dso.clone();
            poke32(&mut bad.bytes, verdef.offset + 16, 0);
            assert_eq!(
                error_of(&bad.bytes),
                bad_version(VersionRecord::Definition, verdef.offset, VersionFault::ChainEnds)
            );
            // vd_next past the end.
            let mut bad = dso.clone();
            poke32(&mut bad.bytes, verdef.offset + 16, u32::MAX);
            assert!(matches!(
                error_of(&bad.bytes),
                Some(Error::BadVersion {
                    record: VersionRecord::Definition,
                    fault: VersionFault::Truncated,
                    ..
                })
            ));
            // vd_cnt 0: no name.
            let mut bad = dso.clone();
            poke16(&mut bad.bytes, verdef.offset + 28 + 6, 0);
            assert_eq!(
                error_of(&bad.bytes),
                bad_version(VersionRecord::Definition, verdef.offset + 28, VersionFault::NoName)
            );
            // vd_cnt more than fit.
            let mut bad = dso.clone();
            poke16(&mut bad.bytes, verdef.offset + 28 + 6, 0xffff);
            assert_eq!(
                error_of(&bad.bytes),
                bad_version(
                    VersionRecord::DefinitionName,
                    verdef.offset + 48,
                    VersionFault::TooMany(0xffff)
                )
            );
            // vda_next 0 before the last name.
            let mut bad = dso.clone();
            poke32(&mut bad.bytes, verdef.offset + 56 + 24, 0);
            assert_eq!(
                error_of(&bad.bytes),
                bad_version(
                    VersionRecord::DefinitionName,
                    verdef.offset + 76,
                    VersionFault::ChainEnds
                )
            );
            // A name outside the string table.
            let mut bad = dso.clone();
            poke32(&mut bad.bytes, verdef.offset + 20, 10_000);
            assert!(matches!(
                error_of(&bad.bytes),
                Some(Error::StringOutOfBounds { offset: 10_000, .. })
            ));
        }
        // More definitions than the bytes can hold.
        let mut bad = dso.clone();
        bad.set_shdr(verdef.index, |s| s.sh_info = 1000);
        let object = SharedObject::parse(&bad.bytes).expect("parse");
        assert_eq!(
            object.version_definitions().err(),
            Some(Error::BadVersion {
                record: VersionRecord::Definition,
                offset: verdef.offset as u64,
                fault: VersionFault::TooMany(1000)
            })
        );
        let mut bad = dso.clone();
        let num = bad.dyn_index(DT_VERDEFNUM);
        bad.set_dyn(num, |d| d.d_tag = DT_DEBUG);
        let object = SharedObject::parse_by(&bad.bytes, Entry::Dynamic).expect("parse");
        assert_eq!(
            object.version_definitions().err(),
            Some(Error::MissingDynamicTag { d_tag: DT_VERDEFNUM, needed_by: DT_VERDEF })
        );
        assert_eq!(object.versions().err(), object.version_definitions().err());

        // vn_version 0, and vna_next 0 needs no next (the last entry).
        let verneed = dso.part(".gnu.version_r");
        let mut bad = dso.clone();
        poke16(&mut bad.bytes, verneed.offset, 0);
        let object = SharedObject::parse(&bad.bytes).expect("parse");
        let first = object.version_needs().expect("needs").expect("chain").next();
        assert_eq!(
            first.map(Result::err),
            Some(Some(Error::BadVersion {
                record: VersionRecord::Need,
                offset: verneed.offset as u64,
                fault: VersionFault::Revision(0)
            }))
        );
        let mut bad = dso.clone();
        poke16(&mut bad.bytes, verneed.offset + 2, 0xff);
        let object = SharedObject::parse(&bad.bytes).expect("parse");
        let need = object
            .version_needs()
            .expect("needs")
            .expect("chain")
            .next()
            .expect("one")
            .expect("ok");
        assert_eq!(
            need.entries().err(),
            Some(Error::BadVersion {
                record: VersionRecord::NeedEntry,
                offset: verneed.offset as u64 + 16,
                fault: VersionFault::TooMany(0xff)
            })
        );

        // A version index no definition or need has, and one two have.
        let versym_at = dso.part(".gnu.version").offset;
        let mut bad = dso.clone();
        poke16(&mut bad.bytes, versym_at + 2 * SYM_BAR as usize, 9);
        for object in both_paths(&bad.bytes) {
            assert_eq!(
                object.versions().expect("versions").of(SymbolIndex(SYM_BAR)).err(),
                Some(Error::UnknownVersionIndex { symbol: SYM_BAR, index: 9 })
            );
        }
        let mut bad = dso.clone();
        poke16(&mut bad.bytes, verneed.offset + 32 + 16 + 6, 3);
        for object in both_paths(&bad.bytes) {
            assert_eq!(
                object.versions().expect("versions").of(SymbolIndex(SYM_FOO_2)).err(),
                Some(Error::AmbiguousVersionIndex { index: 3 })
            );
            // Other symbols are unaffected.
            assert!(object.versions().expect("versions").of(SymbolIndex(SYM_BAZ)).is_ok());
        }
    }
}

// ---- §6.8 notes and properties ------------------------------------------------

/// §6.8: the program property note of `PT_GNU_PROPERTY`, and the notes of
/// `PT_NOTE`, with the segment's alignment.
#[test]
fn s6_8_the_property_note() {
    for dso in samples() {
        for object in both_paths(&dso.bytes) {
            let note = object.gnu_property_note().expect("note").expect("a note");
            assert_eq!(note.n_type, NT_GNU_PROPERTY_TYPE_0);
            assert_eq!(note.name(), b"GNU");
            let properties: Vec<_> =
                note.gnu_properties().expect("properties").map(|p| p.expect("property")).collect();
            assert_eq!(properties.len(), 1);
            let expected = if object.machine() == mortar_elf::Machine::AArch64 {
                GNU_PROPERTY_AARCH64_FEATURE_1_AND
            } else {
                GNU_PROPERTY_X86_FEATURE_1_AND
            };
            assert_eq!(properties[0].pr_type, expected);
            assert_eq!(properties[0].as_u32(), Some(3));
            let notes =
                object.program_headers().get(PH_NOTE).expect("note").notes().expect("notes");
            assert_eq!(notes.count(), 1);
        }
    }
}

/// §6.8: the note errors of a segment.
#[test]
fn s6_8_note_errors() {
    for dso in [x86_64(), i386()] {
        let object = SharedObject::parse(&dso.bytes).expect("parse");
        assert_eq!(
            object.program_headers().get(PH_TEXT).expect("text").notes().err(),
            Some(Error::WrongSegmentType { segment: PH_TEXT, p_type: PT_LOAD, expected: PT_NOTE })
        );
        let mut bad = dso.clone();
        bad.set_phdr(PH_PROPERTY, |p| p.p_align = 16);
        let object = SharedObject::parse(&bad.bytes).expect("parse");
        assert_eq!(
            object.gnu_property_note().err(),
            Some(Error::SegmentNoteAlignment { segment: PH_PROPERTY, align: 16 })
        );
        let mut bad = dso.clone();
        bad.set_phdr(PH_PROPERTY, |p| p.p_filesz = 14);
        let object = SharedObject::parse(&bad.bytes).expect("parse");
        let error = object.gnu_property_note().expect_err("truncated");
        assert!(
            matches!(
                error,
                Error::BadSegmentNote { segment: PH_PROPERTY, offset: 0, part: NotePart::Name }
            ),
            "{error:?}"
        );
        assert_eq!(error.segment(), Some(PH_PROPERTY));
        // No PT_GNU_PROPERTY, no property note (even with a PT_NOTE).
        let mut bad = dso.clone();
        bad.set_phdr(PH_PROPERTY, |p| p.p_type = PT_NULL);
        assert!(matches!(
            SharedObject::parse(&bad.bytes).expect("parse").gnu_property_note(),
            Ok(None)
        ));
    }
}

// ---- §6.9 copy-relocation facts -----------------------------------------------

/// §6.9: the segment a symbol's value is in, whether it is writable, and
/// its alignment.
#[test]
fn s6_9_the_segment_of_a_symbol() {
    for dso in samples() {
        for object in both_paths(&dso.bytes) {
            let symbols = object.symbols().expect("symbols").expect("table");
            let segments = object.program_headers();
            let of = |index: u32| {
                let sym = symbols.get(SymbolIndex(index)).expect("symbol");
                segments.load_containing(sym.st_value).expect("in a segment")
            };
            let bar = of(SYM_BAR);
            assert_eq!(bar.index(), PH_DATA);
            assert!(bar.is_writable());
            assert_eq!(bar.alignment(), Ok(0x1000));
            let prot = of(SYM_PROT);
            assert_eq!(prot.index(), PH_TEXT);
            assert!(!prot.is_writable());
        }
    }
}

// ---- §6.11 errors -------------------------------------------------------------

/// §6.11: a walk of every accessor of every sample finds no error, and of
/// the section-stripped samples neither.
#[test]
fn s6_11_every_accessor_of_the_samples_is_clean() {
    for dso in samples() {
        for (bytes, entry) in [
            (dso.bytes.clone(), Some(Entry::Sections)),
            (dso.bytes.clone(), Some(Entry::Dynamic)),
            (dso.stripped(), None),
        ] {
            let walk = walk_dso(&bytes, entry).expect("parse");
            assert_eq!(walk.symbols, u64::from(SYMBOLS));
            assert_eq!(walk.versions, u64::from(SYMBOLS));
            assert_eq!((walk.definitions, walk.needs, walk.notes), (3, 2, 2));
            // The only errors are the asked-for ones: unique(PT_LOAD), the
            // notes of the six non-note segments, and value() of each
            // DT_NEEDED, which repeats.
            let expected = 1 + 6 + 2;
            assert_eq!(walk.errors, expected, "{:?}", walk.first_error);
        }
    }
}

/// §6.11: the segment errors name their segment and no section.
#[test]
fn s6_11_errors_name_their_place() {
    let error = Error::SegmentOutOfBounds { segment: 3, offset: 1, size: 2, file_len: 3 };
    assert_eq!((error.segment(), error.section()), (Some(3), None));
    let error = Error::VersymLink { section: 5, link: 2 };
    assert_eq!((error.segment(), error.section()), (None, Some(5)));
    let error = Error::NoSymbolCount;
    assert_eq!((error.segment(), error.section()), (None, None));
    for error in [
        Error::NotSharedObject { e_type: 1 },
        Error::UnhashedSymbolCount { symoffset: 4 },
        Error::MissingDynamicTag { d_tag: DT_STRSZ, needed_by: DT_STRTAB },
        Error::DuplicateDynamicTag { d_tag: 0x6fff_0000, first: 0, second: 1 },
        Error::MultipleSegments { p_type: PT_DYNAMIC, first: 0, second: 1 },
        Error::BadVersion {
            record: VersionRecord::NeedEntry,
            offset: 8,
            fault: VersionFault::TooMany(3),
        },
    ] {
        let text = error.to_string();
        assert!(!text.is_empty() && !text.contains('\n'), "{text}");
    }
    assert!(
        Error::MissingDynamicTag { d_tag: DT_STRSZ, needed_by: DT_STRTAB }
            .to_string()
            .contains("DT_STRSZ")
    );
}
