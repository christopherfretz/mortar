//! L0: the behavior `spec/elf.md` §2 to §4 states, section by section, on
//! files built in memory (`support::Builder`) for both classes. Each test
//! names the section it checks. Agreement with binutils and LLVM on real
//! objects is `tools.rs`; malformed input at large is `hostile.rs`.

mod support;

use mortar_elf::consts::{
    self, EM_386, EM_AARCH64, EM_RISCV, EM_X86_64, ET_CORE, ET_DYN, ET_EXEC, GRP_COMDAT,
    NT_GNU_BUILD_ID, NT_GNU_PROPERTY_TYPE_0, SHF_ALLOC, SHF_COMPRESSED, SHF_MERGE, SHF_STRINGS,
    SHN_XINDEX, SHT_GROUP, SHT_NOBITS, SHT_NOTE, SHT_PROGBITS, SHT_REL, SHT_RELA, SHT_RELR,
    SHT_STRTAB, SHT_SYMTAB, SHT_SYMTAB_SHNDX,
};
use mortar_elf::{
    Class, Error, FileHeader, Machine, NotePart, Object, Rel, Rela, SectionIndex, Shdr, Sym,
    SymbolIndex, SymbolSection, Table, Word, reloc,
};
use support::{
    Builder, Built, TARGETS, enc_note, enc_property, enc_rel, enc_rela, enc_sym, rel_size,
    rela_size, sample, sym_size, walk, words,
};

fn parse(built: &Built) -> Object<'_> {
    Object::parse(&built.bytes).expect("the file parses")
}

fn section<'a>(built: &'a Built, name: &str) -> mortar_elf::Section<'a> {
    parse(built)
        .sections()
        .find(|s| s.name() == Ok(name.as_bytes()))
        .unwrap_or_else(|| panic!("no section {name}"))
}

fn index_of(built: &Built, name: &str) -> u32 {
    section(built, name).index().0
}

// ---- §2 --------------------------------------------------------------------

/// §2.2: values are named by the specification's names; a value the
/// module does not name is `None`; the processor-specific section types
/// are named per machine.
#[test]
fn s2_2_values_have_their_specification_names() {
    assert_eq!(consts::et_name(ET_DYN), Some("ET_DYN"));
    assert_eq!(consts::et_name(0xfe00), None);
    assert_eq!(consts::em_name(EM_386), Some("EM_386"));
    assert_eq!(consts::em_name(EM_AARCH64), Some("EM_AARCH64"));
    assert_eq!(consts::em_name(0x1234), None);
    assert_eq!(consts::sht_name(EM_AARCH64, SHT_RELR), Some("SHT_RELR"));
    assert_eq!(consts::sht_name(EM_X86_64, 0x7000_0001), Some("SHT_X86_64_UNWIND"));
    assert_eq!(consts::sht_name(EM_AARCH64, 0x7000_0001), None);
    assert_eq!(consts::sht_name(EM_AARCH64, 0x7000_0003), Some("SHT_AARCH64_ATTRIBUTES"));
    assert_eq!(consts::sht_name(EM_386, 0x6fff_4c01), Some("SHT_LLVM_LINKER_OPTIONS"));
    assert_eq!(consts::stb_name(10), Some("STB_GNU_UNIQUE"));
    assert_eq!(consts::stt_name(10), Some("STT_GNU_IFUNC"));
    assert_eq!(consts::stv_name(3), Some("STV_PROTECTED"));
    assert_eq!(consts::stb_name(7), None);
    assert_eq!(consts::elfcompress_name(2), Some("ELFCOMPRESS_ZSTD"));
    assert_eq!(consts::pt_name(0x6474_e553), Some("PT_GNU_PROPERTY"));
    assert_eq!(consts::dt_name(14), Some("DT_SONAME"));
    assert_eq!(consts::nt_gnu_name(5), Some("NT_GNU_PROPERTY_TYPE_0"));
}

/// §2.3: relocation types are named per machine; the same number means a
/// different thing on each. (`tools.rs` checks every number against
/// binutils and LLVM.)
#[test]
fn s2_3_relocation_types_are_named_per_machine() {
    assert_eq!(reloc::name(EM_AARCH64, 283), Some("R_AARCH64_CALL26"));
    assert_eq!(reloc::name(EM_X86_64, 4), Some("R_X86_64_PLT32"));
    assert_eq!(reloc::name(EM_386, 4), Some("R_386_PLT32"));
    assert_eq!(reloc::name(EM_386, 43), Some("R_386_GOT32X"));
    assert_eq!(reloc::name(EM_AARCH64, 4), None);
    assert_eq!(reloc::name(EM_RISCV, 1), None);
    assert_eq!(reloc::aarch64::R_AARCH64_ABS64, 257);
    assert_eq!(reloc::x86_64::R_X86_64_REX_GOTPCRELX, 42);
    assert_eq!(reloc::i386::R_386_TLS_GD, 18);
    assert_eq!(Machine::I386.reloc_name(1), Some("R_386_32"));
    assert_eq!(Machine::X86_64.section_type_name(0x7000_0001), Some("SHT_X86_64_UNWIND"));
}

// ---- §3.1 ------------------------------------------------------------------

/// §3.1: the three targets are accepted, each in its class, and the
/// object reports class and machine.
#[test]
fn s3_1_the_three_targets_are_accepted() {
    for ((class, e_machine), machine) in
        TARGETS.into_iter().zip([Machine::AArch64, Machine::X86_64, Machine::I386])
    {
        let built = sample(class, e_machine);
        let object = parse(&built);
        assert_eq!((object.class(), object.machine()), (class, machine));
        assert_eq!(machine.class(), class);
        assert_eq!(machine.e_machine(), e_machine);
        assert_eq!(object.header().e_machine, e_machine);
        assert_eq!(object.header().e_shoff, built.shoff as u64);
        assert_eq!(object.data().len(), built.bytes.len());
    }
}

/// §3.1: what is not a supported relocatable object is identified, each
/// kind by its own error, checked in the order magic, class, encoding,
/// length, version, machine, class of the machine, file type; and the
/// message names the refused thing.
#[test]
fn s3_1_everything_else_is_refused_by_name() {
    let good = sample(Class::Elf64, EM_AARCH64);
    let with = |edit: &dyn Fn(&mut Vec<u8>)| {
        let mut bytes = good.bytes.clone();
        edit(&mut bytes);
        Object::parse(&bytes).expect_err("refused")
    };
    assert_eq!(Object::parse(b"").unwrap_err(), Error::NotElf);
    assert_eq!(Object::parse(b"\x7fEL").unwrap_err(), Error::NotElf);
    assert_eq!(Object::parse(b"!<arch>\n").unwrap_err(), Error::NotElf);
    assert_eq!(
        Object::parse(b"\x7fELF\x02\x01").unwrap_err(),
        Error::TruncatedHeader { len: 6, expected: 16 }
    );
    assert_eq!(
        Object::parse(&good.bytes[..63]).unwrap_err(),
        Error::TruncatedHeader { len: 63, expected: 64 }
    );
    let small = sample(Class::Elf32, EM_386);
    assert_eq!(
        Object::parse(&small.bytes[..51]).unwrap_err(),
        Error::TruncatedHeader { len: 51, expected: 52 }
    );
    assert_eq!(with(&|b| b[4] = 0), Error::InvalidClass { class: 0 });
    assert_eq!(with(&|b| b[4] = 3), Error::InvalidClass { class: 3 });
    assert_eq!(with(&|b| b[5] = 2), Error::BigEndian);
    assert!(Error::BigEndian.to_string().contains("big-endian"));
    assert_eq!(with(&|b| b[5] = 0), Error::InvalidEncoding { data: 0 });
    // Class and encoding come before the length: a short big-endian file
    // is big-endian.
    assert_eq!(
        Object::parse(b"\x7fELF\x01\x02\x01\0\0\0\0\0\0\0\0\0").unwrap_err(),
        Error::BigEndian
    );
    assert_eq!(with(&|b| b[6] = 0), Error::UnsupportedVersion { ei_version: 0, e_version: 1 });
    assert_eq!(with(&|b| b[20] = 2), Error::UnsupportedVersion { ei_version: 1, e_version: 2 });

    let machine = |class: Class, e_machine: u16, e_type: u16| {
        let mut built = sample(class, e_machine);
        built.set_ehdr(|e| e.e_type = e_type);
        Object::parse(&built.bytes).map(|_| ())
    };
    let riscv = machine(Class::Elf64, EM_RISCV, 1).unwrap_err();
    assert_eq!(riscv, Error::UnsupportedMachine { e_machine: EM_RISCV });
    assert_eq!(riscv.to_string(), "unsupported machine EM_RISCV (243)");
    assert_eq!(
        machine(Class::Elf64, 0x1234, 1).unwrap_err().to_string(),
        "unsupported machine 4660"
    );
    let x32 = machine(Class::Elf32, EM_X86_64, 1).unwrap_err();
    assert_eq!(x32, Error::UnsupportedClass { class: Class::Elf32, e_machine: EM_X86_64 });
    assert!(x32.to_string().contains("x32"), "{x32}");
    let ilp32 = machine(Class::Elf32, EM_AARCH64, 1).unwrap_err();
    assert_eq!(ilp32, Error::UnsupportedClass { class: Class::Elf32, e_machine: EM_AARCH64 });
    assert!(ilp32.to_string().contains("ILP32"), "{ilp32}");
    let wide = machine(Class::Elf64, EM_386, 1).unwrap_err();
    assert_eq!(wide, Error::UnsupportedClass { class: Class::Elf64, e_machine: EM_386 });
    assert_eq!(wide.to_string(), "ELF64 is not a class of EM_386");
    // The machine comes before the file type.
    assert_eq!(
        machine(Class::Elf64, EM_RISCV, ET_DYN).unwrap_err(),
        Error::UnsupportedMachine { e_machine: EM_RISCV }
    );
    for (e_type, name) in
        [(ET_DYN, "ET_DYN"), (ET_EXEC, "ET_EXEC"), (ET_CORE, "ET_CORE"), (0, "ET_NONE")]
    {
        for (class, e_machine) in TARGETS {
            let error = machine(class, e_machine, e_type).unwrap_err();
            assert_eq!(error, Error::NotRelocatable { e_type });
            assert!(error.to_string().ends_with(name), "{error}");
        }
    }
    assert_eq!(
        machine(Class::Elf64, EM_X86_64, 0xff00).unwrap_err().to_string(),
        "not a relocatable object: e_type is 0xff00"
    );
}

/// §3.1: the file header reader alone accepts any little-endian ELF file
/// of either class, whatever its type and machine (the shared-object
/// reader starts from it), and exposes every field.
#[test]
fn s3_1_the_file_header_reads_any_little_endian_file() {
    for (class, e_machine) in [(Class::Elf64, EM_RISCV), (Class::Elf32, EM_X86_64)] {
        let mut built = sample(class, e_machine);
        built.set_ehdr(|e| {
            e.e_type = ET_DYN;
            e.e_entry = 0x1234;
            e.e_phoff = 0x40;
            e.e_flags = 5;
            e.e_phentsize = 56;
            e.e_phnum = 3;
            e.ei_osabi = 3;
            e.ei_abiversion = 1;
        });
        let header = FileHeader::parse(&built.bytes).expect("header");
        assert_eq!(header.class, class);
        assert_eq!((header.e_type, header.e_machine), (ET_DYN, e_machine));
        assert_eq!((header.e_entry, header.e_phoff, header.e_flags), (0x1234, 0x40, 5));
        assert_eq!((header.e_phentsize, header.e_phnum), (56, 3));
        assert_eq!((header.ei_osabi, header.ei_abiversion, header.ei_version), (3, 1, 1));
        assert_eq!(header.e_ehsize, if class == Class::Elf32 { 52 } else { 64 });
        assert_eq!(header.e_shnum, built.count as u16);
        assert!(Machine::of(&header).is_err());
        assert!(Object::parse(&built.bytes).is_err());
    }
}

// ---- §3.2 ------------------------------------------------------------------

/// §3.2: sections are numbered from 0 in table order; a section view
/// exposes every header field as stored and its name; an index past the
/// table is an error that names index and count.
#[test]
fn s3_2_sections_are_indexed_and_iterated() {
    for (class, e_machine) in TARGETS {
        let built = sample(class, e_machine);
        let object = parse(&built);
        assert_eq!(object.section_count(), 13);
        let names: Vec<_> = object
            .sections()
            .map(|s| String::from_utf8_lossy(s.name().unwrap()).into_owned())
            .collect();
        assert_eq!(
            names,
            [
                "",
                ".group",
                ".text.f",
                ".rela.text.f",
                ".rel.data",
                ".data",
                ".bss",
                ".rodata.str1.1",
                ".note.gnu.property",
                ".debug_info",
                ".symtab",
                ".strtab",
                ".shstrtab"
            ]
        );
        let rela = object.section(SectionIndex(3)).unwrap();
        let header = rela.header();
        assert_eq!(rela.index(), SectionIndex(3));
        assert_eq!((header.sh_type, header.sh_link, header.sh_info), (SHT_RELA, 10, 2));
        assert_eq!(header.sh_flags, 0x240);
        assert_eq!(header.sh_entsize, rela_size(class));
        assert_eq!(header.sh_size, 2 * rela_size(class));
        assert_eq!(header.sh_addralign, class.word_size() as u64);
        assert_eq!((rela.sh_type(), rela.flags(), rela.size()), (SHT_RELA, 0x240, header.sh_size));
        assert_eq!(object.section_table().headers().get(3), Some(*header));
        assert_eq!(object.section_table().names().section(), 12);
        assert_eq!(
            object.section(SectionIndex(13)).unwrap_err(),
            Error::SectionIndexOutOfRange { index: 13, count: 13 }
        );
        // Section 0 is a section like any other to the table.
        let null = object.section(SectionIndex(0)).unwrap();
        assert_eq!((null.sh_type(), null.name()), (0, Ok(&b""[..])));
    }
}

/// §3.2: a file without a section header table (`e_shoff` 0) has no
/// sections; `e_shnum` or `e_shstrndx` set without a table is an error.
#[test]
fn s3_2_a_file_may_have_no_sections() {
    for (class, e_machine) in TARGETS {
        let mut built = Builder::new(class, e_machine).build();
        built.set_ehdr(|e| {
            e.e_shoff = 0;
            e.e_shnum = 0;
            e.e_shstrndx = 0;
        });
        let object = parse(&built);
        assert_eq!(object.section_count(), 0);
        assert_eq!(object.sections().count(), 0);
        assert!(object.section_table().is_empty());
        assert_eq!(object.symbols().map(|s| s.is_some()), Ok(false));
        built.set_ehdr(|e| e.e_shnum = 3);
        assert_eq!(
            Object::parse(&built.bytes).unwrap_err(),
            Error::MissingSectionTable { e_shnum: 3 }
        );
        built.set_ehdr(|e| {
            e.e_shnum = 0;
            e.e_shstrndx = 2;
        });
        assert_eq!(
            Object::parse(&built.bytes).unwrap_err(),
            Error::BadShstrndx { index: 2, count: 0 }
        );
    }
}

/// §3.2: the table must be of the class's entry size and lie within the
/// file; the section name string table index must be a `SHT_STRTAB`
/// section within the file; 0 means no names.
#[test]
fn s3_2_the_table_and_the_name_table_are_checked_eagerly() {
    for (class, e_machine) in TARGETS {
        let good = sample(class, e_machine);
        let entsize = if class == Class::Elf32 { 40 } else { 64 };
        let file_len = good.bytes.len() as u64;
        let with = |edit: &dyn Fn(&mut Built)| {
            let mut built = good.clone();
            edit(&mut built);
            Object::parse(&built.bytes).map(|_| ())
        };
        assert_eq!(
            with(&|b| b.set_ehdr(|e| e.e_shentsize = 23)),
            Err(Error::BadShentsize { e_shentsize: 23, expected: entsize })
        );
        assert_eq!(
            with(&|b| b.set_ehdr(|e| e.e_shnum = 14)),
            Err(Error::SectionTableOutOfBounds { offset: good.shoff as u64, count: 14, file_len })
        );
        assert_eq!(
            with(&|b| b.set_ehdr(|e| e.e_shoff = file_len + 1)),
            Err(Error::SectionTableOutOfBounds { offset: file_len + 1, count: 13, file_len })
        );
        let top = if class == Class::Elf32 { u64::from(u32::MAX) } else { u64::MAX };
        assert_eq!(
            with(&|b| b.set_ehdr(|e| e.e_shoff = top - 7)),
            Err(Error::SectionTableOutOfBounds { offset: top - 7, count: 13, file_len })
        );
        assert_eq!(
            with(&|b| b.set_ehdr(|e| e.e_shstrndx = 13)),
            Err(Error::BadShstrndx { index: 13, count: 13 })
        );
        assert_eq!(
            with(&|b| b.set_ehdr(|e| e.e_shstrndx = 0xfff1)),
            Err(Error::BadShstrndx { index: 0xfff1, count: 13 })
        );
        assert_eq!(
            with(&|b| b.set_ehdr(|e| e.e_shstrndx = 5)),
            Err(Error::WrongSectionType {
                section: 5,
                sh_type: SHT_PROGBITS,
                expected: SHT_STRTAB
            })
        );
        let moved = with(&|b| b.set_shdr(12, |s| s.sh_offset = file_len));
        assert!(matches!(moved, Err(Error::SectionOutOfBounds { section: 12, .. })), "{moved:?}");
        // No name table: only the empty name exists.
        let mut nameless = good.clone();
        nameless.set_ehdr(|e| e.e_shstrndx = 0);
        let object = parse(&nameless);
        assert_eq!(object.section(SectionIndex(0)).unwrap().name(), Ok(&b""[..]));
        let named = object.section(SectionIndex(2)).unwrap();
        assert!(matches!(named.name(), Err(Error::StringOutOfBounds { section: 0, size: 0, .. })));
    }
}

/// A file with `count` sections in all (null section and `.shstrtab`
/// included): empty `SHT_PROGBITS` sections named `.s<index>`, and a symbol
/// table whose last symbol is defined in the last of them.
fn many_sections(class: Class, e_machine: u16, count: u32) -> Built {
    let mut b = Builder::new(class, e_machine);
    let filler = count - 5;
    for i in 1..=filler {
        b.progbits(&format!(".s{i}"), SHT_PROGBITS, SHF_ALLOC, Vec::new());
    }
    let last = filler;
    let symtab = b.next_index();
    let escaped = last >= 0xff00;
    let symbols: Vec<u8> = [
        Sym::default(),
        Sym { st_name: 1, st_info: 0x11, st_shndx: 1, ..Sym::default() },
        Sym {
            st_name: 3,
            st_info: 0x11,
            st_shndx: if escaped { SHN_XINDEX } else { last as u16 },
            ..Sym::default()
        },
    ]
    .iter()
    .flat_map(|s| enc_sym(class, s))
    .collect();
    b.add(
        ".symtab",
        Shdr {
            sh_type: SHT_SYMTAB,
            sh_link: symtab + 2,
            sh_info: 1,
            sh_addralign: 8,
            sh_entsize: sym_size(class),
            ..Shdr::default()
        },
        symbols,
    );
    b.add(
        ".symtab_shndx",
        Shdr {
            sh_type: SHT_SYMTAB_SHNDX,
            sh_link: symtab,
            sh_addralign: 4,
            sh_entsize: 4,
            ..Shdr::default()
        },
        words(&[0, 0, if escaped { last } else { 0 }]),
    );
    b.progbits(".strtab", SHT_STRTAB, 0, b"\0a\0z\0".to_vec());
    let built = b.build();
    assert_eq!(built.count, count);
    built
}

/// §3.2, §3.5: with 0xff00 sections or more the count is in `sh_size` of
/// header 0 (`e_shnum` 0), a name table index from 0xff00 up is in
/// `sh_link` of header 0 (`e_shstrndx` `SHN_XINDEX`), and a symbol's
/// section index from 0xff00 up is in the `SHT_SYMTAB_SHNDX` section.
/// Checked at the boundary and above it, for both classes.
#[test]
fn s3_2_large_section_counts_use_the_escapes() {
    for (class, e_machine) in [(Class::Elf64, EM_AARCH64), (Class::Elf32, EM_386)] {
        for count in [0xfeff_u32, 0xff00, 0xff01, 0xff06, 70_000] {
            let built = many_sections(class, e_machine, count);
            let object = parse(&built);
            let header = object.header();
            if count >= 0xff00 {
                assert_eq!(header.e_shnum, 0, "count {count}");
            } else {
                assert_eq!(u32::from(header.e_shnum), count);
            }
            // The name table is the last section: index count - 1.
            assert_eq!(header.e_shstrndx == SHN_XINDEX, count > 0xff00, "count {count}");
            assert_eq!(object.section_count(), count);
            assert_eq!(object.sections().len(), count as usize);
            assert_eq!(object.section_table().names().section(), count - 1);
            let last = count - 5;
            let section = object.section(SectionIndex(last)).unwrap();
            assert_eq!(section.name(), Ok(format!(".s{last}").as_bytes()));
            assert_eq!(
                object.section(SectionIndex(count - 1)).unwrap().name(),
                Ok(&b".shstrtab"[..])
            );
            assert!(object.section(SectionIndex(count)).is_err());

            let symbols = object.symbols().unwrap().expect("a symbol table");
            assert!(symbols.has_extended_indexes());
            let z = symbols.get(SymbolIndex(2)).unwrap();
            assert_eq!(symbols.name(&z), Ok(&b"z"[..]));
            assert_eq!(z.st_shndx == SHN_XINDEX, last >= 0xff00, "count {count}");
            assert_eq!(
                symbols.section_of(SymbolIndex(2), &z),
                Ok(SymbolSection::Section(SectionIndex(last)))
            );
            let a = symbols.get(SymbolIndex(1)).unwrap();
            assert_eq!(
                symbols.section_of(SymbolIndex(1), &a),
                Ok(SymbolSection::Section(SectionIndex(1)))
            );
        }
    }
}

/// §3.2: with `e_shnum` 0 the count is `sh_size` of header 0 whatever its
/// value; a count of 0 there is a file without sections; a count that
/// does not fit the file, or 32 bits, is an error and nothing is
/// allocated or walked for it.
#[test]
fn s3_2_the_count_escape_is_taken_at_its_word() {
    let good = sample(Class::Elf64, EM_AARCH64);
    let file_len = good.bytes.len() as u64;
    let offset = good.shoff as u64;
    let escaped = |count: u64, shstrndx: u16| {
        let mut built = good.clone();
        built.set_shdr(0, |s| s.sh_size = count);
        built.set_ehdr(|e| {
            e.e_shnum = 0;
            e.e_shstrndx = shstrndx;
        });
        built
    };
    assert_eq!(parse(&escaped(13, 12)).section_count(), 13);
    let fewer = escaped(5, 0);
    assert_eq!(parse(&fewer).section_count(), 5);
    assert_eq!(parse(&escaped(0, 0)).section_count(), 0);
    assert_eq!(
        Object::parse(&escaped(0, 12).bytes).unwrap_err(),
        Error::BadShstrndx { index: 12, count: 0 }
    );
    assert_eq!(
        Object::parse(&escaped(14, 12).bytes).unwrap_err(),
        Error::SectionTableOutOfBounds { offset, count: 14, file_len }
    );
    assert_eq!(
        Object::parse(&escaped(1 << 60, 12).bytes).unwrap_err(),
        Error::TooManySections { count: 1 << 60 }
    );
    assert_eq!(
        Object::parse(&escaped(u64::from(u32::MAX), 12).bytes).unwrap_err(),
        Error::SectionTableOutOfBounds { offset, count: u64::from(u32::MAX), file_len }
    );
    // The name table escape with a table too short to hold header 0.
    let mut empty = escaped(0, SHN_XINDEX);
    assert_eq!(
        Object::parse(&empty.bytes).unwrap_err(),
        Error::BadShstrndx { index: 0xffff, count: 0 }
    );
    // The count escape with no room for header 0.
    empty.set_ehdr(|e| e.e_shoff = file_len - 8);
    assert_eq!(
        Object::parse(&empty.bytes).unwrap_err(),
        Error::SectionTableOutOfBounds { offset: file_len - 8, count: 1, file_len }
    );
    // The name table escape below 0xff00 is taken at its word too.
    let mut low = good.clone();
    low.set_shdr(0, |s| s.sh_link = 12);
    low.set_ehdr(|e| e.e_shstrndx = SHN_XINDEX);
    assert_eq!(parse(&low).section(SectionIndex(1)).unwrap().name(), Ok(&b".group"[..]));
}

// ---- §3.3, §3.4 ------------------------------------------------------------

/// §3.3: a section name outside the name table, or not terminated, is an
/// error of the accessor that names the name table's section, and does
/// not make the object unreadable.
#[test]
fn s3_3_bad_section_names_are_errors_of_the_name() {
    let mut built = sample(Class::Elf64, EM_AARCH64);
    built.set_shdr(5, |s| s.sh_name = 0x1000);
    let object = parse(&built);
    let size = object.section_table().names().bytes().len() as u64;
    assert_eq!(
        object.section(SectionIndex(5)).unwrap().name(),
        Err(Error::StringOutOfBounds { section: 12, offset: 0x1000, size })
    );
    assert_eq!(object.section(SectionIndex(6)).unwrap().name(), Ok(&b".bss"[..]));
    // Cut the name table's terminator off.
    built.set_shdr(12, |s| s.sh_size -= 1);
    let last = parse(&built).section(SectionIndex(12)).unwrap();
    assert!(matches!(last.name(), Err(Error::StringNotTerminated { section: 12, .. })));
    // A string table read through its section.
    let strings = section(&built, ".strtab").strings().unwrap();
    assert_eq!(strings.get(1), Ok(&b"f"[..]));
    assert_eq!(strings.section(), 11);
    assert_eq!(
        section(&built, ".text.f").strings().unwrap_err(),
        Error::WrongSectionType { section: 2, sh_type: SHT_PROGBITS, expected: SHT_STRTAB }
    );
}

/// §3.4: the contents are `sh_size` bytes at `sh_offset`; `SHT_NOBITS`
/// and `SHT_NULL` sections have none, whatever their offset and size; a
/// range past the end of the file, or one that overflows, is an error.
#[test]
fn s3_4_contents_are_bounds_checked() {
    for (class, e_machine) in TARGETS {
        let mut built = sample(class, e_machine);
        let file_len = built.bytes.len() as u64;
        let object = parse(&built);
        let text = object.section(SectionIndex(2)).unwrap();
        assert_eq!(text.data(), Ok(&[0x1f, 0x20, 0x03, 0xd5, 0xc0, 0x03, 0x5f, 0xd6][..]));
        let range = text.file_range().unwrap().unwrap();
        assert_eq!(range.len(), 8);
        assert_eq!(&built.bytes[range], text.data().unwrap());
        let bss = object.section(SectionIndex(6)).unwrap();
        assert_eq!((bss.sh_type(), bss.size()), (SHT_NOBITS, 0x1000));
        assert_eq!((bss.file_range(), bss.data()), (Ok(None), Ok(&[][..])));
        assert_eq!(object.section(SectionIndex(0)).unwrap().file_range(), Ok(None));

        // A NOBITS section may claim anything: it has no bytes.
        built.set_shdr(6, |s| {
            s.sh_offset = u64::from(u32::MAX);
            s.sh_size = u64::from(u32::MAX);
        });
        assert_eq!(parse(&built).section(SectionIndex(6)).unwrap().data(), Ok(&[][..]));
        // An empty section at the very end of the file is fine.
        built.set_shdr(5, |s| {
            s.sh_offset = file_len;
            s.sh_size = 0;
        });
        assert_eq!(parse(&built).section(SectionIndex(5)).unwrap().data(), Ok(&[][..]));
        built.set_shdr(5, |s| s.sh_size = 1);
        assert_eq!(
            parse(&built).section(SectionIndex(5)).unwrap().data(),
            Err(Error::SectionOutOfBounds { section: 5, offset: file_len, size: 1, file_len })
        );
        let top = if class == Class::Elf32 { u64::from(u32::MAX) } else { u64::MAX };
        built.set_shdr(5, |s| {
            s.sh_offset = top;
            s.sh_size = top;
        });
        assert_eq!(
            parse(&built).section(SectionIndex(5)).unwrap().file_range(),
            Err(Error::SectionOutOfBounds { section: 5, offset: top, size: top, file_len })
        );
    }
}

/// §3.4: the alignment is `sh_addralign` with 0 read as 1; a value that
/// is not a power of two is an error. `sh_link` and `sh_info` read as
/// section indexes are checked against the section count.
#[test]
fn s3_4_alignment_and_links_are_checked() {
    fn get(built: &Built, index: u32) -> mortar_elf::Section<'_> {
        parse(built).section(SectionIndex(index)).unwrap()
    }
    let mut built = sample(Class::Elf32, EM_386);
    assert_eq!(get(&built, 0).alignment(), Ok(1));
    assert_eq!(get(&built, 6).alignment(), Ok(16));
    built.set_shdr(6, |s| s.sh_addralign = 24);
    assert_eq!(get(&built, 6).alignment(), Err(Error::BadAlignment { section: 6, align: 24 }));
    built.set_shdr(6, |s| s.sh_addralign = 1 << 31);
    assert_eq!(get(&built, 6).alignment(), Ok(1 << 31));
    assert_eq!(get(&built, 3).link(), Ok(SectionIndex(10)));
    assert_eq!(get(&built, 3).info_section(), Ok(SectionIndex(2)));
    built.set_shdr(3, |s| {
        s.sh_link = 13;
        s.sh_info = u32::MAX;
    });
    assert_eq!(get(&built, 3).link(), Err(Error::BadLink { section: 3, link: 13, count: 13 }));
    assert_eq!(
        get(&built, 3).info_section(),
        Err(Error::BadInfo { section: 3, info: u32::MAX, count: 13 })
    );
}

// ---- §3.5 ------------------------------------------------------------------

/// §3.5: every field of a symbol in both layouts (the classes order the
/// fields differently), names, the local/global split at `sh_info`, and
/// the section of each kind of symbol.
#[test]
fn s3_5_symbols_are_decoded_in_both_classes() {
    for (class, e_machine) in TARGETS {
        let built = sample(class, e_machine);
        let object = parse(&built);
        let symbols = object.symbols().unwrap().expect("a symbol table");
        assert_eq!(symbols.section(), SectionIndex(10));
        assert_eq!((symbols.len(), symbols.first_global()), (7, SymbolIndex(4)));
        assert!(!symbols.has_extended_indexes());
        assert_eq!(symbols.strings().section(), 11);
        let all: Vec<(SymbolIndex, Sym)> = symbols.iter().collect();
        assert_eq!(all.len(), 7);
        assert_eq!(all[0], (SymbolIndex(0), Sym::default()));
        let (index, local) = all[1];
        assert_eq!(index, SymbolIndex(1));
        assert_eq!(
            local,
            Sym { st_name: 3, st_info: 1, st_other: 0, st_shndx: 5, st_value: 8, st_size: 4 }
        );
        assert_eq!(symbols.name(&local), Ok(&b"local"[..]));
        assert_eq!((local.binding(), local.sym_type(), local.visibility()), (0, 1, 0));
        let f = all[4].1;
        assert_eq!((f.binding(), f.sym_type(), f.visibility(), f.st_other), (2, 2, 2, 2));
        assert_eq!((symbols.name(&f), f.st_size), (Ok(&b"f"[..]), 8));
        let section_of = |i: usize| symbols.section_of(all[i].0, &all[i].1);
        assert_eq!(section_of(0), Ok(SymbolSection::Undefined));
        assert_eq!(section_of(1), Ok(SymbolSection::Section(SectionIndex(5))));
        assert_eq!(section_of(2), Ok(SymbolSection::Section(SectionIndex(2))));
        assert_eq!(section_of(3), Ok(SymbolSection::Absolute));
        assert_eq!(section_of(5), Ok(SymbolSection::Undefined));
        assert_eq!(section_of(6), Ok(SymbolSection::Common));
        assert_eq!((all[6].1.st_value, all[6].1.st_size), (8, 64));
        let locals: Vec<u32> = symbols.locals().map(|(i, _)| i.0).collect();
        let globals: Vec<u32> = symbols.globals().map(|(i, _)| i.0).collect();
        assert_eq!((locals, globals), (vec![0, 1, 2, 3], vec![4, 5, 6]));
        assert_eq!(symbols.records().get(6), Some(all[6].1));
        assert_eq!(symbols.get(SymbolIndex(6)), Ok(all[6].1));
        assert_eq!(
            symbols.get(SymbolIndex(7)),
            Err(Error::SymbolIndexOutOfRange { index: 7, count: 7 })
        );
    }
}

/// §3.5: the largest field values survive widening in both classes.
#[test]
fn s3_5_symbol_fields_are_widened_not_truncated() {
    for (class, e_machine) in TARGETS {
        let max = if class == Class::Elf32 { u64::from(u32::MAX) } else { u64::MAX };
        let symbol = Sym {
            st_name: 1,
            st_info: 0xa6,
            st_other: 0x83,
            st_shndx: 0xff02,
            st_value: max,
            st_size: max - 1,
        };
        let mut b = Builder::new(class, e_machine);
        let mut data = enc_sym(class, &Sym::default());
        data.extend(enc_sym(class, &symbol));
        b.add(
            ".symtab",
            Shdr {
                sh_type: SHT_SYMTAB,
                sh_link: 2,
                sh_info: 1,
                sh_entsize: sym_size(class),
                ..Shdr::default()
            },
            data,
        );
        b.progbits(".strtab", SHT_STRTAB, 0, b"\0x\0".to_vec());
        let built = b.build();
        let symbols = parse(&built).symbols().unwrap().unwrap();
        let read = symbols.get(SymbolIndex(1)).unwrap();
        assert_eq!(read, symbol);
        assert_eq!((read.binding(), read.sym_type(), read.visibility()), (10, 6, 3));
        assert_eq!(read.st_other & consts::STO_AARCH64_VARIANT_PCS, 0x80);
        assert_eq!(
            symbols.section_of(SymbolIndex(1), &read),
            Ok(SymbolSection::Reserved(consts::SHN_X86_64_LCOMMON))
        );
    }
}

/// §3.5: the symbol table's shape is checked when it is asked for: entry
/// size and size of the class, the string table link, `sh_info`, one
/// table per file, the extended index table's length.
#[test]
fn s3_5_a_malformed_symbol_table_is_an_error() {
    for (class, e_machine) in TARGETS {
        let good = sample(class, e_machine);
        let entsize = sym_size(class);
        let symbols = |edit: &dyn Fn(&mut Built)| {
            let mut built = good.clone();
            edit(&mut built);
            parse(&built).symbols().map(|s| s.map(|s| s.len()))
        };
        assert_eq!(symbols(&|_| {}), Ok(Some(7)));
        assert_eq!(
            symbols(&|b| b.set_shdr(10, |s| s.sh_entsize = 23)),
            Err(Error::BadEntsize { section: 10, entsize: 23, expected: entsize })
        );
        // The other class's entry size is wrong too.
        let other = 40 - entsize;
        assert_eq!(
            symbols(&|b| b.set_shdr(10, |s| s.sh_entsize = other)),
            Err(Error::BadEntsize { section: 10, entsize: other, expected: entsize })
        );
        assert_eq!(
            symbols(&|b| b.set_shdr(10, |s| s.sh_size -= 1)),
            Err(Error::BadSize { section: 10, size: 7 * entsize - 1, entsize })
        );
        assert_eq!(
            symbols(&|b| b.set_shdr(10, |s| s.sh_info = 8)),
            Err(Error::BadFirstGlobal { section: 10, sh_info: 8, count: 7 })
        );
        assert_eq!(symbols(&|b| b.set_shdr(10, |s| s.sh_info = 7)), Ok(Some(7)));
        assert_eq!(
            symbols(&|b| b.set_shdr(10, |s| s.sh_link = 99)),
            Err(Error::BadLink { section: 10, link: 99, count: 13 })
        );
        assert_eq!(
            symbols(&|b| b.set_shdr(10, |s| s.sh_link = 5)),
            Err(Error::WrongSectionType {
                section: 5,
                sh_type: SHT_PROGBITS,
                expected: SHT_STRTAB
            })
        );
        assert_eq!(
            symbols(&|b| b.set_shdr(5, |s| s.sh_type = SHT_SYMTAB)),
            Err(Error::MultipleSymbolTables { first: 5, second: 10 })
        );
        assert_eq!(symbols(&|b| b.set_shdr(10, |s| s.sh_type = SHT_PROGBITS)), Ok(None));
        let out = symbols(&|b| b.set_shdr(10, |s| s.sh_offset = 1 << 31));
        assert!(matches!(out, Err(Error::SectionOutOfBounds { section: 10, .. })), "{out:?}");
        // An explicit table of the wrong type.
        let object = parse(&good);
        assert_eq!(
            object.section_table().symbol_table(SectionIndex(5), None).unwrap_err(),
            Error::WrongSectionType { section: 5, sh_type: SHT_PROGBITS, expected: SHT_SYMTAB }
        );
        assert_eq!(
            object.section_table().find_symbol_table(consts::SHT_DYNSYM).map(|s| s.is_some()),
            Ok(false)
        );
    }
}

/// §3.5: `SHN_XINDEX` needs the extended index table, which must have one
/// entry per symbol and be unique; an extended index of 0 is undefined; a
/// section index that is not a section is an error that names the symbol.
#[test]
fn s3_5_symbol_section_indexes_are_checked() {
    for (class, e_machine) in TARGETS {
        // `.data` (5) becomes the extended index table of `.symtab` (10).
        let escaped = |entries: &[u32], st_shndx: u16| {
            let mut b = Builder::new(class, e_machine);
            let symbols: Vec<u8> =
                [Sym::default(), Sym { st_info: 0x10, st_shndx, ..Sym::default() }]
                    .iter()
                    .flat_map(|s| enc_sym(class, s))
                    .collect();
            b.add(
                ".symtab",
                Shdr {
                    sh_type: SHT_SYMTAB,
                    sh_link: 2,
                    sh_info: 1,
                    sh_entsize: sym_size(class),
                    ..Shdr::default()
                },
                symbols,
            );
            b.progbits(".strtab", SHT_STRTAB, 0, vec![0]);
            b.add(
                ".symtab_shndx",
                Shdr { sh_type: SHT_SYMTAB_SHNDX, sh_link: 1, sh_entsize: 4, ..Shdr::default() },
                words(entries),
            );
            b.build()
        };
        let resolve = |built: &Built| {
            let symbols = parse(built).symbols()?.unwrap();
            let symbol = symbols.get(SymbolIndex(1))?;
            symbols.section_of(SymbolIndex(1), &symbol)
        };
        assert_eq!(
            resolve(&escaped(&[0, 2], SHN_XINDEX)),
            Ok(SymbolSection::Section(SectionIndex(2)))
        );
        assert_eq!(resolve(&escaped(&[0, 0], SHN_XINDEX)), Ok(SymbolSection::Undefined));
        // Entries of symbols that do not use the escape are ignored.
        assert_eq!(resolve(&escaped(&[7, 9], 3)), Ok(SymbolSection::Section(SectionIndex(3))));
        assert_eq!(
            resolve(&escaped(&[0, 5], SHN_XINDEX)),
            Err(Error::SymbolSectionOutOfRange { symbol: 1, index: 5, count: 5 })
        );
        assert_eq!(
            resolve(&escaped(&[0, 0], 5)),
            Err(Error::SymbolSectionOutOfRange { symbol: 1, index: 5, count: 5 })
        );
        assert_eq!(
            resolve(&escaped(&[0, 2, 3], SHN_XINDEX)),
            Err(Error::ShndxCountMismatch { section: 3, entries: 3, symbols: 2 })
        );
        assert_eq!(
            resolve(&escaped(&[0], SHN_XINDEX)),
            Err(Error::ShndxCountMismatch { section: 3, entries: 1, symbols: 2 })
        );
        let mut built = escaped(&[0, 2], SHN_XINDEX);
        built.set_shdr(3, |s| s.sh_entsize = 8);
        assert_eq!(resolve(&built), Err(Error::BadEntsize { section: 3, entsize: 8, expected: 4 }));
        // A table linked to another section is not this table's.
        let mut built = escaped(&[0, 2], SHN_XINDEX);
        built.set_shdr(3, |s| s.sh_link = 2);
        assert_eq!(resolve(&built), Err(Error::MissingShndx { symbol: 1 }));
        let mut built = escaped(&[0, 2], SHN_XINDEX);
        built.set_shdr(2, |s| {
            s.sh_type = SHT_SYMTAB_SHNDX;
            s.sh_link = 1;
        });
        // (The string table is gone too, but the duplicate is found first.)
        assert_eq!(resolve(&built), Err(Error::MultipleShndxTables { first: 2, second: 3 }));
    }
}

// ---- §3.6 ------------------------------------------------------------------

/// §3.6: `SHT_RELA` and `SHT_REL` records in both layouts: `r_info` holds
/// symbol and type as 24 and 8 bits in `ELFCLASS32`, 32 and 32 in
/// `ELFCLASS64`; an `ELFCLASS32` addend is sign-extended; records are
/// indexable and iterable in file order.
#[test]
fn s3_6_relocations_are_decoded_in_both_classes() {
    for (class, e_machine) in TARGETS {
        let built = sample(class, e_machine);
        let rela = section(&built, ".rela.text.f").rela().unwrap();
        assert_eq!(
            (rela.section(), rela.symtab(), rela.target()),
            (SectionIndex(3), SectionIndex(10), SectionIndex(2))
        );
        assert_eq!((rela.len(), rela.is_empty()), (2, false));
        let expect = [
            Rela { r_offset: 0, r_sym: 5, r_type: 1, r_addend: -4 },
            Rela { r_offset: 4, r_sym: 2, r_type: 2, r_addend: 0x1234 },
        ];
        assert_eq!(rela.iter().collect::<Vec<_>>(), expect);
        assert_eq!(
            (rela.get(0), rela.get(1), rela.get(2)),
            (Some(expect[0]), Some(expect[1]), None)
        );
        assert_eq!(rela.into_iter().count(), 2);
        let rel = section(&built, ".rel.data").rel().unwrap();
        assert_eq!((rel.symtab(), rel.target(), rel.len()), (SectionIndex(10), SectionIndex(5), 1));
        assert_eq!(rel.get(0), Some(Rel { r_offset: 2, r_sym: 4, r_type: 1 }));
        assert_eq!(rel.iter().next_back(), rel.get(0));
    }
    // The extremes of each packing.
    for (class, e_machine, sym, r_type, offset, addend) in [
        (Class::Elf32, EM_386, 0xff_ffff_u32, 0xff_u32, u64::from(u32::MAX), i64::from(i32::MIN)),
        (Class::Elf64, EM_X86_64, u32::MAX, u32::MAX, u64::MAX, i64::MIN),
        (Class::Elf64, EM_AARCH64, 1, 0x244, 1 << 40, -1),
    ] {
        let record = Rela { r_offset: offset, r_sym: sym, r_type, r_addend: addend };
        let mut b = Builder::new(class, e_machine);
        b.add(
            ".rela",
            Shdr { sh_type: SHT_RELA, sh_entsize: rela_size(class), ..Shdr::default() },
            enc_rela(class, &record),
        );
        b.add(
            ".rel",
            Shdr { sh_type: SHT_REL, sh_entsize: rel_size(class), ..Shdr::default() },
            enc_rel(class, &Rel { r_offset: offset, r_sym: sym, r_type }),
        );
        let built = b.build();
        assert_eq!(section(&built, ".rela").rela().unwrap().get(0), Some(record));
        assert_eq!(
            section(&built, ".rel").rel().unwrap().get(0),
            Some(Rel { r_offset: offset, r_sym: sym, r_type })
        );
    }
}

/// §3.6: a relocation section's type, entry size (the class's), size,
/// `sh_link` and `sh_info` are checked when its records are asked for;
/// `SHT_RELR` is a named type whose records are not read.
#[test]
fn s3_6_a_malformed_relocation_section_is_an_error() {
    for (class, e_machine) in TARGETS {
        let good = sample(class, e_machine);
        let (with_addend, without) = (rela_size(class), rel_size(class));
        let rela = |edit: &dyn Fn(&mut Shdr)| {
            let mut built = good.clone();
            built.set_shdr(3, edit);
            section(&built, ".rela.text.f").rela().map(|r| r.len())
        };
        assert_eq!(
            rela(&|s| s.sh_entsize = 23),
            Err(Error::BadEntsize { section: 3, entsize: 23, expected: with_addend })
        );
        assert_eq!(
            rela(&|s| s.sh_entsize = 0),
            Err(Error::BadEntsize { section: 3, entsize: 0, expected: with_addend })
        );
        assert_eq!(
            Error::BadEntsize { section: 7, entsize: 23, expected: 24 }.to_string(),
            "section 7: sh_entsize is 23, expected 24"
        );
        assert_eq!(
            rela(&|s| s.sh_size += 1),
            Err(Error::BadSize { section: 3, size: 2 * with_addend + 1, entsize: with_addend })
        );
        assert_eq!(
            rela(&|s| s.sh_link = 13),
            Err(Error::BadLink { section: 3, link: 13, count: 13 })
        );
        assert_eq!(
            rela(&|s| s.sh_info = 13),
            Err(Error::BadInfo { section: 3, info: 13, count: 13 })
        );
        assert!(matches!(
            rela(&|s| s.sh_offset = 1 << 30),
            Err(Error::SectionOutOfBounds { section: 3, .. })
        ));
        // RELA records are not REL records, nor the reverse.
        assert_eq!(
            section(&good, ".rela.text.f").rel().unwrap_err(),
            Error::WrongSectionType { section: 3, sh_type: SHT_RELA, expected: SHT_REL }
        );
        assert_eq!(
            section(&good, ".rel.data").rela().unwrap_err(),
            Error::WrongSectionType { section: 4, sh_type: SHT_REL, expected: SHT_RELA }
        );
        let mut built = good.clone();
        built.set_shdr(4, |s| s.sh_entsize = with_addend);
        assert_eq!(
            section(&built, ".rel.data").rel().unwrap_err(),
            Error::BadEntsize { section: 4, entsize: with_addend, expected: without }
        );
        // SHT_RELR: recognized and named, not read as RELA or REL.
        let mut built = good.clone();
        built.set_shdr(4, |s| s.sh_type = SHT_RELR);
        let packed = section(&built, ".rel.data");
        assert_eq!(parse(&built).machine().section_type_name(packed.sh_type()), Some("SHT_RELR"));
        let error = packed.rela().unwrap_err();
        assert_eq!(
            error,
            Error::WrongSectionType { section: 4, sh_type: SHT_RELR, expected: SHT_RELA }
        );
        assert_eq!(error.to_string(), "section 4: type is SHT_RELR, expected SHT_RELA");
        assert!(packed.rel().is_err());
    }
}

// ---- §3.7 ------------------------------------------------------------------

/// §3.7: a group is a flag word and member section indexes, 32-bit words
/// in both classes; `sh_link` is the symbol table and `sh_info` the
/// signature symbol.
#[test]
fn s3_7_groups_are_read() {
    for (class, e_machine) in TARGETS {
        let built = sample(class, e_machine);
        let group = section(&built, ".group").group().unwrap();
        assert_eq!((group.flags(), group.is_comdat()), (GRP_COMDAT, true));
        assert_eq!(
            (group.section(), group.symtab(), group.signature()),
            (SectionIndex(1), SectionIndex(10), SymbolIndex(4))
        );
        assert_eq!((group.len(), group.is_empty()), (2, false));
        let members: Vec<_> = group.members().collect();
        assert_eq!(members, [Ok(SectionIndex(2)), Ok(SectionIndex(3))]);
        assert_eq!(group.members().len(), 2);
        let symbols = parse(&built).symbols().unwrap().unwrap();
        let signature = symbols.get(group.signature()).unwrap();
        assert_eq!(symbols.name(&signature), Ok(&b"f"[..]));
    }
}

/// §3.7: a group's type, entry size, size, flag word and `sh_link` are
/// checked when it is asked for; each member is checked to be a section
/// other than section 0 as it is read, and a bad member does not hide the
/// ones after it.
#[test]
fn s3_7_a_malformed_group_is_an_error() {
    fn read(built: &Built) -> Result<mortar_elf::Group<'_>, Error> {
        section(built, ".group").group()
    }
    let group = |flags: u32, members: &[u32], edit: &dyn Fn(&mut Shdr)| {
        let mut b = Builder::new(Class::Elf64, EM_X86_64);
        let mut contents = vec![flags];
        contents.extend_from_slice(members);
        b.add(
            ".group",
            Shdr { sh_type: SHT_GROUP, sh_link: 2, sh_info: 0, sh_entsize: 4, ..Shdr::default() },
            words(&contents),
        );
        b.progbits(".text", SHT_PROGBITS, 0, Vec::new());
        let mut built = b.build();
        built.set_shdr(1, edit);
        built
    };
    let plain = group(0, &[], &|_| {});
    let empty = read(&plain).unwrap();
    assert_eq!(
        (empty.flags(), empty.is_comdat(), empty.len(), empty.is_empty()),
        (0, false, 0, true)
    );
    assert_eq!(empty.members().count(), 0);
    let mixed = group(GRP_COMDAT | 0x10, &[2, 0, 4, 3, u32::MAX], &|_| {});
    let members: Vec<_> = read(&mixed).unwrap().members().collect();
    let bad = |member: u32, index: u32| {
        Err(Error::GroupMemberOutOfRange { section: 1, member, index, count: 4 })
    };
    assert_eq!(
        members,
        [Ok(SectionIndex(2)), bad(1, 0), bad(2, 4), Ok(SectionIndex(3)), bad(4, u32::MAX)]
    );
    assert_eq!(read(&mixed).unwrap().flags(), GRP_COMDAT | 0x10);
    assert_eq!(
        read(&group(1, &[2], &|s| s.sh_size = 0)).unwrap_err(),
        Error::EmptyGroup { section: 1 }
    );
    assert_eq!(
        read(&group(1, &[2], &|s| s.sh_size = 7)).unwrap_err(),
        Error::BadSize { section: 1, size: 7, entsize: 4 }
    );
    assert_eq!(
        read(&group(1, &[2], &|s| s.sh_entsize = 8)).unwrap_err(),
        Error::BadEntsize { section: 1, entsize: 8, expected: 4 }
    );
    assert_eq!(
        read(&group(1, &[2], &|s| s.sh_link = 4)).unwrap_err(),
        Error::BadLink { section: 1, link: 4, count: 4 }
    );
    assert_eq!(
        section(&plain, ".text").group().unwrap_err(),
        Error::WrongSectionType { section: 2, sh_type: SHT_PROGBITS, expected: SHT_GROUP }
    );
}

// ---- §3.8 ------------------------------------------------------------------

fn note_section(class: Class, e_machine: u16, align: u64, data: Vec<u8>) -> Built {
    let mut b = Builder::new(class, e_machine);
    b.add(".note", Shdr { sh_type: SHT_NOTE, sh_addralign: align, ..Shdr::default() }, data);
    b.build()
}

type NoteSummary = (u32, Vec<u8>, Vec<u8>);

fn notes_of(built: &Built) -> Result<Vec<Result<NoteSummary, Error>>, Error> {
    Ok(section(built, ".note")
        .notes()?
        .map(|note| note.map(|n| (n.n_type, n.name().to_vec(), n.desc().to_vec())))
        .collect())
}

/// §3.8: notes are read in order; name and descriptor are padded to 4
/// bytes, or to 8 when the section's alignment is 8, in both classes; the
/// padding after the last descriptor may be missing.
#[test]
fn s3_8_notes_follow_the_section_alignment() {
    for (class, e_machine) in TARGETS {
        for (sh_addralign, align) in [(0u64, 4usize), (1, 4), (4, 4), (8, 8)] {
            let mut data = enc_note(b"GNU\0", NT_GNU_BUILD_ID, &[0xab; 20], align);
            data.extend(enc_note(b"stapsdt\0", 3, b"x", align));
            data.extend(enc_note(b"", 7, b"", align));
            data.extend(enc_note(b"LINUX", 0x1234_5678, &[1, 2, 3, 4, 5], align));
            let expect = vec![
                Ok((NT_GNU_BUILD_ID, b"GNU".to_vec(), vec![0xab; 20])),
                Ok((3, b"stapsdt".to_vec(), b"x".to_vec())),
                Ok((7, Vec::new(), Vec::new())),
                Ok((0x1234_5678, b"LINUX".to_vec(), vec![1, 2, 3, 4, 5])),
            ];
            let full = note_section(class, e_machine, sh_addralign, data.clone());
            assert_eq!(notes_of(&full), Ok(expect.clone()), "align {sh_addralign}");
            // Without the final padding (3 bytes, to either alignment).
            data.truncate(data.len() - 3);
            let unpadded = note_section(class, e_machine, sh_addralign, data);
            assert_eq!(notes_of(&unpadded), Ok(expect), "align {sh_addralign}");
        }
        // The stored name keeps its terminator; `name` strips one.
        let built = note_section(class, e_machine, 4, enc_note(b"GNU\0", 1, b"", 4));
        let note = section(&built, ".note").notes().unwrap().next().unwrap().unwrap();
        assert_eq!((note.name_bytes(), note.name()), (&b"GNU\0"[..], &b"GNU"[..]));
        assert!(note.gnu_properties().is_none());
        assert_eq!(notes_of(&note_section(class, e_machine, 4, Vec::new())), Ok(Vec::new()));
    }
}

/// §3.8: a note section's alignment must be at most 4, or 8; a note that
/// does not fit ends the iteration with an error that says which part and
/// where; a size near 2^32 is an error, not an allocation or an overflow.
#[test]
fn s3_8_malformed_notes_are_errors() {
    let (class, machine) = (Class::Elf64, EM_AARCH64);
    for align in [5u64, 16, 1 << 40] {
        assert_eq!(
            notes_of(&note_section(class, machine, align, Vec::new())),
            Err(Error::NoteAlignment { section: 1, align })
        );
    }
    let good = enc_note(b"GNU\0", 3, &[9; 8], 4);
    let bad = |offset: u64, part: NotePart| Err(Error::BadNote { section: 1, offset, part });
    let first = Ok((3, b"GNU".to_vec(), vec![9; 8]));
    // A second note cut short at each point.
    for (cut, part) in [
        (1, NotePart::Header),
        (11, NotePart::Header),
        (12, NotePart::Name),
        (15, NotePart::Name),
        (16, NotePart::Descriptor),
        (23, NotePart::Descriptor),
    ] {
        let mut data = good.clone();
        data.extend_from_slice(&good[..cut]);
        assert_eq!(
            notes_of(&note_section(class, machine, 4, data)),
            Ok(vec![first.clone(), bad(24, part)]),
            "cut {cut}"
        );
    }
    for (namesz, descsz, part) in [
        (u32::MAX, 0, NotePart::Name),
        (4, u32::MAX, NotePart::Descriptor),
        (u32::MAX - 2, u32::MAX, NotePart::Name),
        (0, 0x8000_0000, NotePart::Descriptor),
    ] {
        let mut data = good.clone();
        data[0..4].copy_from_slice(&namesz.to_le_bytes());
        data[4..8].copy_from_slice(&descsz.to_le_bytes());
        assert_eq!(notes_of(&note_section(class, machine, 4, data)), Ok(vec![bad(0, part)]));
    }
    assert_eq!(
        section(&sample(class, machine), ".data").notes().unwrap_err(),
        Error::WrongSectionType { section: 5, sh_type: SHT_PROGBITS, expected: SHT_NOTE }
    );
    // The same bytes read with the other alignment are a different note.
    let data = enc_note(b"GNU\0\0", 1, &[7; 4], 8);
    assert_eq!(
        notes_of(&note_section(class, machine, 8, data.clone())),
        Ok(vec![Ok((1, b"GNU\0".to_vec(), vec![7; 4]))])
    );
    assert_eq!(
        notes_of(&note_section(class, machine, 4, data)),
        Ok(vec![Ok((1, b"GNU\0".to_vec(), vec![0, 0, 0, 0])), bad(24, NotePart::Header)])
    );
}

/// §3.8: the properties of a `.note.gnu.property` note for each machine:
/// type, data, and data padded to 4 bytes in `ELFCLASS32` and 8 in
/// `ELFCLASS64`; a property that does not fit is an error that ends the
/// iteration.
#[test]
fn s3_8_program_properties_are_read_for_each_machine() {
    for (class, e_machine) in TARGETS {
        let built = sample(class, e_machine);
        let note = section(&built, ".note.gnu.property").notes().unwrap().next().unwrap().unwrap();
        assert_eq!((note.n_type, note.name()), (NT_GNU_PROPERTY_TYPE_0, &b"GNU"[..]));
        let properties: Vec<_> = note.gnu_properties().expect("a property note").collect();
        let feature = if e_machine == EM_AARCH64 {
            consts::GNU_PROPERTY_AARCH64_FEATURE_1_AND
        } else {
            consts::GNU_PROPERTY_X86_FEATURE_1_AND
        };
        assert_eq!(properties.len(), 2);
        let first = properties[0].unwrap();
        assert_eq!((first.pr_type, first.as_u32()), (feature, Some(3)));
        let second = properties[1].unwrap();
        assert_eq!(second.pr_type, consts::GNU_PROPERTY_STACK_SIZE);
        assert_eq!(second.data.len(), class.word_size());
        assert_eq!(second.as_u32(), (class == Class::Elf32).then_some(0x10_0000));

        // Properties cut short, and one whose size runs past the note.
        let word = class.word_size();
        let whole = enc_property(class, feature, &3u32.to_le_bytes());
        let desc_at = 16u64; // header 12, "GNU\0" 4: aligned for 4 and 8
        for cut in [1, 7, 8, 11] {
            let mut desc = whole.clone();
            desc.extend_from_slice(&whole[..cut]);
            let built =
                note_section(class, e_machine, word as u64, enc_note(b"GNU\0", 5, &desc, word));
            let note = section(&built, ".note").notes().unwrap().next().unwrap().unwrap();
            let read: Vec<_> = note.gnu_properties().unwrap().collect();
            assert_eq!(
                read,
                [
                    Ok(mortar_elf::Property { pr_type: feature, data: &[3, 0, 0, 0] }),
                    Err(Error::BadNote {
                        section: 1,
                        offset: desc_at + whole.len() as u64,
                        part: NotePart::Property
                    })
                ],
                "cut {cut}"
            );
        }
        let mut huge = whole.clone();
        huge[4..8].copy_from_slice(&u32::MAX.to_le_bytes());
        let built = note_section(class, e_machine, word as u64, enc_note(b"GNU\0", 5, &huge, word));
        let note = section(&built, ".note").notes().unwrap().next().unwrap().unwrap();
        assert_eq!(
            note.gnu_properties().unwrap().collect::<Vec<_>>(),
            [Err(Error::BadNote { section: 1, offset: desc_at, part: NotePart::Property })]
        );
        // The last property's padding may be missing; a note of another
        // owner or type has no properties.
        let built = note_section(class, e_machine, 4, enc_note(b"GNU\0", 5, &whole[..12], 4));
        let note = section(&built, ".note").notes().unwrap().next().unwrap().unwrap();
        assert_eq!(note.gnu_properties().unwrap().count(), 1);
        for (name, n_type) in [(&b"GNU\0"[..], 3), (b"XYZ\0", 5)] {
            let built = note_section(class, e_machine, 4, enc_note(name, n_type, &whole, 4));
            let note = section(&built, ".note").notes().unwrap().next().unwrap().unwrap();
            assert!(note.gnu_properties().is_none());
        }
    }
}

// ---- §3.9, §3.10 -----------------------------------------------------------

/// §3.9: the compression header of each class (`Elf32_Chdr` is three
/// words, `Elf64_Chdr` has a reserved word) and the bytes after it; a
/// section without the flag has none; nothing is decompressed.
#[test]
fn s3_9_compression_headers_are_decoded() {
    for (class, e_machine) in TARGETS {
        let mut built = sample(class, e_machine);
        let debug = section(&built, ".debug_info");
        let compression = debug.compression().unwrap().expect("compressed");
        assert_eq!(compression.header.ch_type, consts::ELFCOMPRESS_ZLIB);
        assert_eq!((compression.header.ch_size, compression.header.ch_addralign), (0x400, 1));
        assert_eq!(compression.payload, [0x78, 0x9c, 1, 2, 3]);
        let header_size = if class == Class::Elf32 { 12 } else { 24 };
        assert_eq!(debug.data().unwrap().len(), header_size + 5);
        assert_eq!(section(&built, ".data").compression(), Ok(None));

        let index = debug.index().0;
        built.set_shdr(index, |s| s.sh_size = header_size as u64);
        assert_eq!(section(&built, ".debug_info").compression().unwrap().unwrap().payload, &[][..]);
        built.set_shdr(index, |s| s.sh_size -= 1);
        assert_eq!(
            section(&built, ".debug_info").compression(),
            Err(Error::CompressedTooShort {
                section: index,
                size: header_size as u64 - 1,
                expected: header_size as u64
            })
        );
        built.set_shdr(index, |s| s.sh_type = SHT_NOBITS);
        assert_eq!(
            section(&built, ".debug_info").compression(),
            Err(Error::CompressedNobits { section: index })
        );
        let bss = index_of(&built, ".bss");
        built.set_shdr(bss, |s| s.sh_flags |= SHF_COMPRESSED);
        assert_eq!(
            section(&built, ".bss").compression(),
            Err(Error::CompressedNobits { section: bss })
        );
    }
}

/// §3.10: a `SHF_MERGE` section reports its entry size and whether it
/// holds strings; an entry size of 0 is reported, not refused; a size
/// that is not a multiple of the entry size is an error; a section
/// without the flag reports nothing, even with `SHF_STRINGS`.
#[test]
fn s3_10_mergeable_sections_report_their_entry_size() {
    use mortar_elf::Merge;
    let mut built = sample(Class::Elf64, EM_X86_64);
    let index = index_of(&built, ".rodata.str1.1");
    let merge = |built: &Built| section(built, ".rodata.str1.1").merge();
    assert_eq!(merge(&built), Ok(Some(Merge { entsize: 1, strings: true })));
    built.set_shdr(index, |s| {
        s.sh_flags = SHF_ALLOC | SHF_MERGE;
        s.sh_entsize = 4;
    });
    assert_eq!(merge(&built), Ok(Some(Merge { entsize: 4, strings: false })));
    built.set_shdr(index, |s| s.sh_entsize = 0);
    assert_eq!(merge(&built), Ok(Some(Merge { entsize: 0, strings: false })));
    built.set_shdr(index, |s| s.sh_entsize = 8);
    assert_eq!(merge(&built), Err(Error::BadSize { section: index, size: 12, entsize: 8 }));
    built.set_shdr(index, |s| s.sh_flags = SHF_ALLOC | SHF_STRINGS);
    assert_eq!(merge(&built), Ok(None));
    assert_eq!(section(&built, ".data").merge(), Ok(None));
}

// ---- §3.11 -----------------------------------------------------------------

/// §3.11: a table is indexable, sliceable and iterable from both ends;
/// internal iteration (`fold`, `for_each`) gives the records external
/// iteration gives, in both classes; a trailing partial record is not a
/// record.
#[test]
fn s3_11_tables_iterate_the_same_either_way() {
    for class in [Class::Elf32, Class::Elf64] {
        let size = sym_size(class) as usize;
        let mut bytes = Vec::new();
        for i in 0..9u32 {
            bytes.extend(enc_sym(
                class,
                &Sym {
                    st_name: i,
                    st_value: u64::from(i) * 3,
                    st_shndx: i as u16,
                    ..Sym::default()
                },
            ));
        }
        bytes.extend_from_slice(&[0xff; 5]);
        let table: Table<'_, Sym> = Table::new(&bytes, class);
        assert_eq!((table.len(), table.is_empty(), table.class()), (9, false, class));
        assert_eq!(table.bytes().len(), 9 * size);
        let by_next: Vec<Sym> = table.iter().collect();
        let mut by_fold = Vec::new();
        table.iter().for_each(|s| by_fold.push(s));
        let by_index: Vec<Sym> = (0..9).map(|i| table.get(i).unwrap()).collect();
        let mut by_back: Vec<Sym> = table.iter().rev().collect();
        by_back.reverse();
        assert_eq!(by_next.len(), 9);
        assert!(by_next.iter().enumerate().all(|(i, s)| s.st_name == i as u32));
        assert_eq!((&by_fold, &by_index, &by_back), (&by_next, &by_next, &by_next));
        assert_eq!(table.iter().fold(0, |sum, s| sum + s.st_value), 108);
        assert_eq!((table.get(9), table.get(usize::MAX)), (None, None));
        assert_eq!(table.iter().len(), 9);
        let mut iter = table.iter();
        assert_eq!(iter.nth(3).map(|s| s.st_name), Some(3));
        assert_eq!((iter.len(), iter.next().map(|s| s.st_name)), (5, Some(4)));
        assert_eq!(iter.nth(usize::MAX), None);
        assert_eq!(iter.next(), None);
        let middle = table.slice(2, 5).unwrap();
        assert_eq!(middle.iter().map(|s| s.st_name).collect::<Vec<_>>(), [2, 3, 4]);
        assert_eq!(table.slice(9, 9).map(|t| t.len()), Some(0));
        assert!(table.slice(5, 2).is_none() && table.slice(0, 10).is_none());
        assert!(Table::<Word>::empty(class).is_empty());
        assert_eq!((&table).into_iter().count(), 9);
    }
}

/// §3.11: the reader's modules do not allocate: outside their tests they
/// name no allocating type or macro and use `core` only.
#[test]
fn s3_11_the_reader_names_no_allocating_type() {
    let sources = [
        ("consts.rs", include_str!("../src/consts.rs")),
        ("error.rs", include_str!("../src/error.rs")),
        ("header.rs", include_str!("../src/header.rs")),
        ("note.rs", include_str!("../src/note.rs")),
        ("object.rs", include_str!("../src/object.rs")),
        ("raw.rs", include_str!("../src/raw.rs")),
        ("reloc.rs", include_str!("../src/reloc.rs")),
        ("section.rs", include_str!("../src/section.rs")),
        ("strtab.rs", include_str!("../src/strtab.rs")),
        ("symbol.rs", include_str!("../src/symbol.rs")),
    ];
    let banned = [
        "std::",
        "alloc::",
        "Vec<",
        "Vec::",
        "vec!",
        "String::",
        "<String",
        ": String",
        "-> String",
        "&String",
        "Box<",
        "Box::",
        "format!",
        "Rc<",
        "Arc<",
        "Cow<",
        ".to_vec(",
        ".to_owned(",
        ".to_string(",
        ".collect",
        "HashMap",
        "BTreeMap",
    ];
    for (name, source) in sources {
        let code = source.split("#[cfg(test)]").next().unwrap();
        for (number, line) in code.lines().enumerate() {
            let line = line.trim_start();
            if line.starts_with("//") {
                continue;
            }
            for word in banned {
                assert!(!line.contains(word), "{name}:{}: `{word}` in `{line}`", number + 1);
            }
        }
    }
}

// ---- §4 --------------------------------------------------------------------

/// §4: every error renders as one line that names the section or symbol
/// it is about, and `section` gives the section for a diagnostic to name.
#[test]
fn s4_errors_say_what_and_where() {
    let about_sections = [
        Error::SectionOutOfBounds { section: 7, offset: 0x40, size: 0x10, file_len: 0x48 },
        Error::WrongSectionType { section: 7, sh_type: 1, expected: 4 },
        Error::BadEntsize { section: 7, entsize: 23, expected: 24 },
        Error::BadSize { section: 7, size: 25, entsize: 24 },
        Error::BadAlignment { section: 7, align: 3 },
        Error::BadLink { section: 7, link: 99, count: 12 },
        Error::BadInfo { section: 7, info: 99, count: 12 },
        Error::StringOutOfBounds { section: 7, offset: 9, size: 4 },
        Error::StringNotTerminated { section: 7, offset: 2 },
        Error::TooManySymbols { section: 7, count: 1 << 33 },
        Error::BadFirstGlobal { section: 7, sh_info: 9, count: 4 },
        Error::ShndxCountMismatch { section: 7, entries: 3, symbols: 4 },
        Error::EmptyGroup { section: 7 },
        Error::GroupMemberOutOfRange { section: 7, member: 1, index: 40, count: 12 },
        Error::NoteAlignment { section: 7, align: 16 },
        Error::BadNote { section: 7, offset: 0x20, part: NotePart::Descriptor },
        Error::CompressedTooShort { section: 7, size: 3, expected: 24 },
        Error::CompressedNobits { section: 7 },
    ];
    for error in about_sections {
        let text = error.to_string();
        assert!(text.starts_with("section 7: "), "{text}");
        assert!(!text.contains('\n'));
        assert_eq!(error.section(), Some(7), "{text}");
    }
    let others = [
        (Error::NotElf, "not an ELF file (bad magic)"),
        (
            Error::TruncatedHeader { len: 20, expected: 64 },
            "file is 20 bytes, too short for an ELF header (64 bytes)",
        ),
        (Error::InvalidClass { class: 9 }, "invalid ELF class 9"),
        (Error::InvalidEncoding { data: 9 }, "invalid ELF data encoding 9"),
        (Error::BadShentsize { e_shentsize: 40, expected: 64 }, "e_shentsize is 40, expected 64"),
        (Error::MissingSectionTable { e_shnum: 3 }, "e_shoff is 0 but e_shnum is 3"),
        (
            Error::TooManySections { count: 1 << 40 },
            "section count 1099511627776 does not fit in 32 bits",
        ),
        (
            Error::SectionTableOutOfBounds { offset: 0x100, count: 9, file_len: 300 },
            "section header table (9 entries at offset 0x100) extends past the end of the file (300 bytes)",
        ),
        (
            Error::BadShstrndx { index: 9, count: 4 },
            "section name string table index is 9, but the file has 4 sections",
        ),
        (
            Error::SectionIndexOutOfRange { index: 9, count: 4 },
            "section index 9 is out of range: the file has 4 sections",
        ),
        (
            Error::MultipleSymbolTables { first: 3, second: 5 },
            "more than one symbol table: sections 3 and 5",
        ),
        (
            Error::SymbolIndexOutOfRange { index: 9, count: 4 },
            "symbol index 9 is out of range: the symbol table has 4 symbols",
        ),
        (
            Error::MissingShndx { symbol: 9 },
            "symbol 9: st_shndx is SHN_XINDEX, but there is no SHT_SYMTAB_SHNDX section",
        ),
        (
            Error::SymbolSectionOutOfRange { symbol: 9, index: 70, count: 12 },
            "symbol 9: section index 70 is out of range: the file has 12 sections",
        ),
    ];
    for (error, text) in others {
        assert_eq!(error.to_string(), text);
        assert_eq!(error.section(), None);
    }
    assert_eq!(
        Error::WrongSectionType { section: 2, sh_type: 0x7000_0001, expected: 4 }.to_string(),
        "section 2: type is 0x70000001, expected SHT_RELA"
    );
    let boxed: Box<dyn std::error::Error> = Box::new(Error::NotElf);
    assert_eq!(boxed.to_string(), "not an ELF file (bad magic)");
}

/// §4: the sample objects walk without an error, and the walk finds what
/// was put in them.
#[test]
fn s4_a_well_formed_object_walks_clean() {
    for (class, e_machine) in TARGETS {
        let built = sample(class, e_machine);
        let found = walk(&built.bytes).expect("parses");
        assert_eq!(found.first_error, None);
        assert_eq!(
            (found.sections, found.symbols, found.relocations, found.groups, found.group_members),
            (13, 7, 3, 1, 2)
        );
        assert_eq!(
            (found.notes, found.properties, found.compressed, found.mergeable),
            (1, 2, 1, 1)
        );
    }
}
