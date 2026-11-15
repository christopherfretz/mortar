//! Shared by the integration tests of `mortar_elf`: a small ELF writer for
//! test inputs (both classes), a walk of every accessor of the reader, and
//! a deterministic pseudo-random generator.
#![allow(dead_code)] // each test binary uses a part

use mortar_elf::consts::{
    EM_386, EM_AARCH64, EM_X86_64, ET_REL, SHF_INFO_LINK, SHN_LORESERVE, SHN_XINDEX, SHT_GROUP,
    SHT_NOBITS, SHT_NOTE, SHT_NULL, SHT_REL, SHT_RELA, SHT_STRTAB, SHT_SYMTAB,
};
use mortar_elf::{
    Class, Ehdr, Error, Object, Rel, Rela, Section, Shdr, Sym, SymbolIndex, SymbolSection,
    SymbolTable,
};

// ---- encoders (the inverse of the reader's decoders, written from the
// gABI's structure definitions) ---------------------------------------------

fn put(out: &mut Vec<u8>, class: Class, word: u64) {
    match class {
        Class::Elf32 => out.extend_from_slice(&(word as u32).to_le_bytes()),
        Class::Elf64 => out.extend_from_slice(&word.to_le_bytes()),
    }
}

fn put32(out: &mut Vec<u8>, value: u32) {
    out.extend_from_slice(&value.to_le_bytes());
}

fn put16(out: &mut Vec<u8>, value: u16) {
    out.extend_from_slice(&value.to_le_bytes());
}

pub fn enc_ehdr(class: Class, e: &Ehdr) -> Vec<u8> {
    let mut out = vec![0x7f, b'E', b'L', b'F'];
    out.push(if class == Class::Elf32 { 1 } else { 2 });
    out.extend_from_slice(&[1, e.ei_version, e.ei_osabi, e.ei_abiversion, 0, 0, 0, 0, 0, 0, 0]);
    put16(&mut out, e.e_type);
    put16(&mut out, e.e_machine);
    put32(&mut out, e.e_version);
    put(&mut out, class, e.e_entry);
    put(&mut out, class, e.e_phoff);
    put(&mut out, class, e.e_shoff);
    put32(&mut out, e.e_flags);
    for half in [e.e_ehsize, e.e_phentsize, e.e_phnum, e.e_shentsize, e.e_shnum, e.e_shstrndx] {
        put16(&mut out, half);
    }
    out
}

pub fn enc_shdr(class: Class, s: &Shdr) -> Vec<u8> {
    let mut out = Vec::new();
    put32(&mut out, s.sh_name);
    put32(&mut out, s.sh_type);
    put(&mut out, class, s.sh_flags);
    put(&mut out, class, s.sh_addr);
    put(&mut out, class, s.sh_offset);
    put(&mut out, class, s.sh_size);
    put32(&mut out, s.sh_link);
    put32(&mut out, s.sh_info);
    put(&mut out, class, s.sh_addralign);
    put(&mut out, class, s.sh_entsize);
    out
}

pub fn enc_sym(class: Class, s: &Sym) -> Vec<u8> {
    let mut out = Vec::new();
    put32(&mut out, s.st_name);
    match class {
        Class::Elf32 => {
            put32(&mut out, s.st_value as u32);
            put32(&mut out, s.st_size as u32);
            out.extend_from_slice(&[s.st_info, s.st_other]);
            put16(&mut out, s.st_shndx);
        }
        Class::Elf64 => {
            out.extend_from_slice(&[s.st_info, s.st_other]);
            put16(&mut out, s.st_shndx);
            out.extend_from_slice(&s.st_value.to_le_bytes());
            out.extend_from_slice(&s.st_size.to_le_bytes());
        }
    }
    out
}

fn r_info(class: Class, sym: u32, r_type: u32) -> u64 {
    match class {
        Class::Elf32 => u64::from((sym << 8) | (r_type & 0xff)),
        Class::Elf64 => (u64::from(sym) << 32) | u64::from(r_type),
    }
}

pub fn enc_rela(class: Class, r: &Rela) -> Vec<u8> {
    let mut out = Vec::new();
    put(&mut out, class, r.r_offset);
    put(&mut out, class, r_info(class, r.r_sym, r.r_type));
    put(&mut out, class, r.r_addend as u64);
    out
}

pub fn enc_rel(class: Class, r: &Rel) -> Vec<u8> {
    let mut out = Vec::new();
    put(&mut out, class, r.r_offset);
    put(&mut out, class, r_info(class, r.r_sym, r.r_type));
    out
}

/// One note, its name and descriptor padded to `align`.
pub fn enc_note(name: &[u8], n_type: u32, desc: &[u8], align: usize) -> Vec<u8> {
    let mut out = Vec::new();
    put32(&mut out, name.len() as u32);
    put32(&mut out, desc.len() as u32);
    put32(&mut out, n_type);
    out.extend_from_slice(name);
    out.resize(out.len().next_multiple_of(align), 0);
    out.extend_from_slice(desc);
    out.resize(out.len().next_multiple_of(align), 0);
    out
}

/// One program property, its data padded to the class's word.
pub fn enc_property(class: Class, pr_type: u32, data: &[u8]) -> Vec<u8> {
    let mut out = Vec::new();
    put32(&mut out, pr_type);
    put32(&mut out, data.len() as u32);
    out.extend_from_slice(data);
    out.resize(out.len().next_multiple_of(class.word_size()), 0);
    out
}

pub fn words(values: &[u32]) -> Vec<u8> {
    values.iter().flat_map(|v| v.to_le_bytes()).collect()
}

pub const fn shdr_size(class: Class) -> usize {
    match class {
        Class::Elf32 => 40,
        Class::Elf64 => 64,
    }
}

pub const fn sym_size(class: Class) -> u64 {
    match class {
        Class::Elf32 => 16,
        Class::Elf64 => 24,
    }
}

pub const fn rela_size(class: Class) -> u64 {
    match class {
        Class::Elf32 => 12,
        Class::Elf64 => 24,
    }
}

pub const fn rel_size(class: Class) -> u64 {
    match class {
        Class::Elf32 => 8,
        Class::Elf64 => 16,
    }
}

// ---- a relocatable-object writer -------------------------------------------

#[derive(Debug, Clone)]
struct Sec {
    name: String,
    shdr: Shdr,
    data: Vec<u8>,
}

/// Builds an `ET_REL` file: sections in the order added after the null
/// section, `.shstrtab` last, the section header table at the end.
#[derive(Debug, Clone)]
pub struct Builder {
    pub class: Class,
    pub ehdr: Ehdr,
    sections: Vec<Sec>,
}

/// A built file and where its parts are, for tests that damage it.
#[derive(Debug, Clone)]
pub struct Built {
    pub class: Class,
    pub bytes: Vec<u8>,
    /// `e_shoff`.
    pub shoff: usize,
    /// The number of sections, `.shstrtab` included.
    pub count: u32,
}

impl Builder {
    pub fn new(class: Class, e_machine: u16) -> Self {
        let ehdr = Ehdr {
            ei_version: 1,
            e_type: ET_REL,
            e_machine,
            e_version: 1,
            e_ehsize: if class == Class::Elf32 { 52 } else { 64 },
            e_shentsize: shdr_size(class) as u16,
            ..Ehdr::default()
        };
        let null = Sec { name: String::new(), shdr: Shdr::default(), data: Vec::new() };
        Builder { class, ehdr, sections: vec![null] }
    }

    /// The machine's natural class.
    pub fn for_machine(e_machine: u16) -> Self {
        Self::new(if e_machine == EM_386 { Class::Elf32 } else { Class::Elf64 }, e_machine)
    }

    /// Adds a section and returns its index. `sh_name`, `sh_offset` and
    /// (except for `SHT_NOBITS`) `sh_size` of `shdr` are set by `build`.
    pub fn add(&mut self, name: &str, shdr: Shdr, data: Vec<u8>) -> u32 {
        self.sections.push(Sec { name: name.to_owned(), shdr, data });
        self.sections.len() as u32 - 1
    }

    /// Adds a section of a type with contents, alignment 1.
    pub fn progbits(&mut self, name: &str, sh_type: u32, flags: u64, data: Vec<u8>) -> u32 {
        self.add(name, Shdr { sh_type, sh_flags: flags, sh_addralign: 1, ..Shdr::default() }, data)
    }

    /// The index the next added section gets.
    pub fn next_index(&self) -> u32 {
        self.sections.len() as u32
    }

    pub fn build(&self) -> Built {
        let class = self.class;
        let mut sections = self.sections.clone();
        let mut names = vec![0u8];
        for sec in &mut sections {
            if !sec.name.is_empty() {
                sec.shdr.sh_name = names.len() as u32;
                names.extend_from_slice(sec.name.as_bytes());
                names.push(0);
            }
        }
        let shstrtab_name = names.len() as u32;
        names.extend_from_slice(b".shstrtab\0");
        sections.push(Sec {
            name: ".shstrtab".to_owned(),
            shdr: Shdr {
                sh_name: shstrtab_name,
                sh_type: SHT_STRTAB,
                sh_addralign: 1,
                ..Shdr::default()
            },
            data: names,
        });
        let count = sections.len() as u32;
        let shstrndx = count - 1;

        let mut ehdr = self.ehdr;
        let mut bytes = vec![0u8; usize::from(ehdr.e_ehsize)];
        for sec in &mut sections[1..] {
            let align = sec.shdr.sh_addralign.clamp(1, 16) as usize;
            bytes.resize(bytes.len().next_multiple_of(align), 0);
            sec.shdr.sh_offset = bytes.len() as u64;
            if sec.shdr.sh_type != SHT_NOBITS {
                sec.shdr.sh_size = sec.data.len() as u64;
                bytes.extend_from_slice(&sec.data);
            }
        }
        bytes.resize(bytes.len().next_multiple_of(8), 0);
        let shoff = bytes.len();
        ehdr.e_shoff = shoff as u64;
        // gABI "ELF Header": the escapes for counts from SHN_LORESERVE up.
        if count >= u32::from(SHN_LORESERVE) {
            ehdr.e_shnum = 0;
            sections[0].shdr.sh_size = u64::from(count);
        } else {
            ehdr.e_shnum = count as u16;
        }
        if shstrndx >= u32::from(SHN_LORESERVE) {
            ehdr.e_shstrndx = SHN_XINDEX;
            sections[0].shdr.sh_link = shstrndx;
        } else {
            ehdr.e_shstrndx = shstrndx as u16;
        }
        for sec in &sections {
            bytes.extend_from_slice(&enc_shdr(class, &sec.shdr));
        }
        let header = enc_ehdr(class, &ehdr);
        bytes[..header.len()].copy_from_slice(&header);
        Built { class, bytes, shoff, count }
    }
}

impl Built {
    /// The file offset of section header `index`.
    pub fn shdr_at(&self, index: u32) -> usize {
        self.shoff + index as usize * shdr_size(self.class)
    }

    /// Overwrites a field of section header `index`.
    pub fn set_shdr(&mut self, index: u32, edit: impl FnOnce(&mut Shdr)) {
        let obj = Object::parse(&self.bytes).expect("a built file parses");
        let mut shdr = *obj.section(mortar_elf::SectionIndex(index)).expect("index").header();
        edit(&mut shdr);
        let at = self.shdr_at(index);
        let encoded = enc_shdr(self.class, &shdr);
        self.bytes[at..at + encoded.len()].copy_from_slice(&encoded);
    }

    /// Overwrites fields of the file header.
    pub fn set_ehdr(&mut self, edit: impl FnOnce(&mut Ehdr)) {
        let mut ehdr = mortar_elf::FileHeader::parse(&self.bytes).expect("header").ehdr;
        edit(&mut ehdr);
        let encoded = enc_ehdr(self.class, &ehdr);
        self.bytes[..encoded.len()].copy_from_slice(&encoded);
    }
}

/// The three supported (class, machine) pairs.
pub const TARGETS: [(Class, u16); 3] =
    [(Class::Elf64, EM_AARCH64), (Class::Elf64, EM_X86_64), (Class::Elf32, EM_386)];

/// A small object with one of everything the reader has an accessor for:
/// code with RELA and REL relocations, a COMDAT group, a note with
/// properties, a compressed section, a mergeable string section, a
/// `SHT_NOBITS` section and a symbol table with locals and globals.
#[allow(clippy::too_many_lines)] // one section after another
pub fn sample(class: Class, e_machine: u16) -> Built {
    use mortar_elf::consts::{
        ELFCOMPRESS_ZLIB, GRP_COMDAT, NT_GNU_PROPERTY_TYPE_0, SHF_ALLOC, SHF_COMPRESSED,
        SHF_EXECINSTR, SHF_GROUP, SHF_MERGE, SHF_STRINGS, SHF_WRITE, SHN_ABS, SHN_COMMON,
    };
    let mut b = Builder::new(class, e_machine);
    // Fixed indexes: the sections are added in this order.
    let (group, text, rela, rel, symtab) = (1, 2, 3, 4, 10);
    let word = class.word_size() as u64;
    b.add(
        ".group",
        Shdr {
            sh_type: SHT_GROUP,
            sh_link: symtab,
            sh_info: 4,
            sh_addralign: 4,
            sh_entsize: 4,
            ..Shdr::default()
        },
        words(&[GRP_COMDAT, text, rela]),
    );
    b.add(
        ".text.f",
        Shdr {
            sh_type: 1,
            sh_flags: SHF_ALLOC | SHF_EXECINSTR | SHF_GROUP,
            sh_addralign: 4,
            ..Shdr::default()
        },
        vec![0x1f, 0x20, 0x03, 0xd5, 0xc0, 0x03, 0x5f, 0xd6],
    );
    let explicit: Vec<u8> = [
        Rela { r_offset: 0, r_sym: 5, r_type: 1, r_addend: -4 },
        Rela { r_offset: 4, r_sym: 2, r_type: 2, r_addend: 0x1234 },
    ]
    .iter()
    .flat_map(|r| enc_rela(class, r))
    .collect();
    b.add(
        ".rela.text.f",
        Shdr {
            sh_type: SHT_RELA,
            sh_flags: SHF_INFO_LINK | SHF_GROUP,
            sh_link: symtab,
            sh_info: text,
            sh_addralign: word,
            sh_entsize: rela_size(class),
            ..Shdr::default()
        },
        explicit,
    );
    let implicit: Vec<u8> =
        [Rel { r_offset: 2, r_sym: 4, r_type: 1 }].iter().flat_map(|r| enc_rel(class, r)).collect();
    b.add(
        ".rel.data",
        Shdr {
            sh_type: SHT_REL,
            sh_flags: SHF_INFO_LINK,
            sh_link: symtab,
            sh_info: 5,
            sh_addralign: word,
            sh_entsize: rel_size(class),
            ..Shdr::default()
        },
        implicit,
    );
    assert_eq!(rel, 4);
    b.progbits(".data", 1, SHF_ALLOC | SHF_WRITE, vec![0; 16]);
    b.add(
        ".bss",
        Shdr {
            sh_type: SHT_NOBITS,
            sh_flags: SHF_ALLOC | SHF_WRITE,
            sh_size: 0x1000,
            sh_addralign: 16,
            ..Shdr::default()
        },
        Vec::new(),
    );
    b.add(
        ".rodata.str1.1",
        Shdr {
            sh_type: 1,
            sh_flags: SHF_ALLOC | SHF_MERGE | SHF_STRINGS,
            sh_addralign: 1,
            sh_entsize: 1,
            ..Shdr::default()
        },
        b"hello\0world\0".to_vec(),
    );
    let feature = if e_machine == EM_AARCH64 { 0xc000_0000 } else { 0xc000_0002 };
    let mut properties = enc_property(class, feature, &3u32.to_le_bytes());
    properties.extend(enc_property(class, 1, &0x10_0000u64.to_le_bytes()[..class.word_size()]));
    b.add(
        ".note.gnu.property",
        Shdr { sh_type: SHT_NOTE, sh_flags: SHF_ALLOC, sh_addralign: word, ..Shdr::default() },
        enc_note(b"GNU\0", NT_GNU_PROPERTY_TYPE_0, &properties, class.word_size()),
    );
    let mut compressed = Vec::new();
    put32(&mut compressed, ELFCOMPRESS_ZLIB);
    if class == Class::Elf64 {
        put32(&mut compressed, 0);
    }
    put(&mut compressed, class, 0x400);
    put(&mut compressed, class, 1);
    compressed.extend_from_slice(&[0x78, 0x9c, 1, 2, 3]);
    b.add(
        ".debug_info",
        Shdr { sh_type: 1, sh_flags: SHF_COMPRESSED, sh_addralign: word, ..Shdr::default() },
        compressed,
    );
    let strtab = b"\0f\0local\0und\0common\0abs\0".to_vec();
    let at = |name: &[u8]| {
        strtab
            .windows(name.len() + 1)
            .position(|w| &w[..name.len()] == name && w[name.len()] == 0)
            .expect("name in strtab") as u32
    };
    let symbols: Vec<u8> = [
        Sym::default(),
        Sym {
            st_name: at(b"local"),
            st_info: 1,
            st_shndx: 5,
            st_value: 8,
            st_size: 4,
            ..Sym::default()
        },
        Sym { st_info: 3, st_shndx: text as u16, ..Sym::default() },
        Sym { st_name: at(b"abs"), st_info: 0, st_shndx: SHN_ABS, st_value: 42, ..Sym::default() },
        Sym {
            st_name: at(b"f"),
            st_info: 0x22,
            st_other: 2,
            st_shndx: text as u16,
            st_size: 8,
            ..Sym::default()
        },
        Sym { st_name: at(b"und"), st_info: 0x10, ..Sym::default() },
        Sym {
            st_name: at(b"common"),
            st_info: 0x11,
            st_shndx: SHN_COMMON,
            st_value: 8,
            st_size: 64,
            ..Sym::default()
        },
    ]
    .iter()
    .flat_map(|s| enc_sym(class, s))
    .collect();
    let index = b.add(
        ".symtab",
        Shdr {
            sh_type: SHT_SYMTAB,
            sh_link: symtab + 1,
            sh_info: 4,
            sh_addralign: word,
            sh_entsize: sym_size(class),
            ..Shdr::default()
        },
        symbols,
    );
    assert_eq!((index, group), (symtab, 1));
    b.progbits(".strtab", SHT_STRTAB, 0, strtab);
    b.build()
}

// ---- a walk of every accessor ----------------------------------------------

/// What a full walk of an object found.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct Walk {
    pub sections: u64,
    pub symbols: u64,
    pub relocations: u64,
    pub groups: u64,
    pub group_members: u64,
    pub notes: u64,
    pub properties: u64,
    pub compressed: u64,
    pub mergeable: u64,
    pub strings: u64,
    /// Accessors that returned an error.
    pub errors: u64,
    /// The first error, for messages.
    pub first_error: Option<Error>,
    /// Records, names and sections visited: the walk's work.
    pub steps: u64,
}

impl Walk {
    fn note<T>(&mut self, result: Result<T, Error>) -> Option<T> {
        self.steps += 1;
        match result {
            Ok(value) => Some(value),
            Err(error) => {
                // Every error renders.
                assert!(!error.to_string().is_empty());
                self.errors += 1;
                self.first_error.get_or_insert(error);
                None
            }
        }
    }
}

fn walk_section(walk: &mut Walk, section: &Section<'_>, symbols: Option<&SymbolTable<'_>>) {
    walk.sections += 1;
    walk.note(section.name());
    walk.note(section.alignment());
    walk.note(section.file_range());
    walk.note(section.data());
    walk.note(section.link());
    walk.note(section.info_section());
    let _ = (section.index(), section.class(), section.flags(), section.size(), section.header());
    if let Some(Some(_)) = walk.note(section.compression()) {
        walk.compressed += 1;
    }
    if let Some(Some(_)) = walk.note(section.merge()) {
        walk.mergeable += 1;
    }
    match section.sh_type() {
        SHT_STRTAB => {
            if let Some(strings) = walk.note(section.strings()) {
                // Every string, by walking the offsets after each NUL.
                let mut offset = 0u32;
                while (offset as usize) < strings.bytes().len() {
                    match walk.note(strings.get(offset)) {
                        Some(string) => {
                            walk.strings += 1;
                            offset += string.len() as u32 + 1;
                        }
                        None => break,
                    }
                }
            }
        }
        SHT_RELA => {
            if let Some(rela) = walk.note(section.rela()) {
                let _ = (rela.section(), rela.symtab(), rela.target(), rela.is_empty());
                let mut via_fold = 0usize;
                rela.iter().for_each(|_| via_fold += 1);
                assert_eq!(via_fold, rela.len());
                for (i, record) in rela.iter().enumerate() {
                    assert_eq!(rela.get(i), Some(record));
                    walk.relocations += 1;
                    if let Some(symbols) = symbols {
                        walk.note(symbols.get(SymbolIndex(record.r_sym)));
                    }
                }
                assert_eq!(rela.get(rela.len()), None);
            }
        }
        SHT_REL => {
            if let Some(rel) = walk.note(section.rel()) {
                for (i, record) in rel.iter().enumerate() {
                    assert_eq!(rel.get(i), Some(record));
                    walk.relocations += 1;
                    if let Some(symbols) = symbols {
                        walk.note(symbols.get(SymbolIndex(record.r_sym)));
                    }
                }
            }
        }
        SHT_GROUP => {
            if let Some(group) = walk.note(section.group()) {
                walk.groups += 1;
                let _ = (group.flags(), group.is_comdat(), group.symtab(), group.section());
                if let Some(symbols) = symbols {
                    walk.note(symbols.get(group.signature()));
                }
                let mut members = 0;
                for member in group.members() {
                    members += 1;
                    if walk.note(member).is_some() {
                        walk.group_members += 1;
                    }
                }
                assert_eq!(members, group.len());
            }
        }
        SHT_NOTE => {
            if let Some(notes) = walk.note(section.notes()) {
                for note in notes {
                    let Some(note) = walk.note(note) else { continue };
                    walk.notes += 1;
                    let _ = (note.name(), note.name_bytes(), note.desc(), note.n_type);
                    for property in note.gnu_properties().into_iter().flatten() {
                        if let Some(property) = walk.note(property) {
                            walk.properties += 1;
                            let _ = property.as_u32();
                        }
                    }
                }
            }
        }
        SHT_NULL | SHT_NOBITS => assert_eq!(section.data(), Ok(&[][..])),
        _ => {}
    }
}

/// Parses `data` and calls every accessor on everything in it, going on
/// after errors. The result of `parse` is the result; the walk never
/// panics for a correct reader.
pub fn walk(data: &[u8]) -> Result<Walk, Error> {
    let object = Object::parse(data)?;
    let mut walk = Walk::default();
    let _ = (object.class(), object.machine(), object.header(), object.data());
    let symbols = walk.note(object.symbols()).flatten();
    assert_eq!(object.sections().len(), object.section_count() as usize);
    let mut via_fold = 0u32;
    object.sections().for_each(|_| via_fold += 1);
    assert_eq!(via_fold, object.section_count());
    for (index, section) in (0u32..).zip(object.sections()) {
        assert_eq!(section.index().0, index);
        walk_section(&mut walk, &section, symbols.as_ref());
    }
    assert!(object.section(mortar_elf::SectionIndex(object.section_count())).is_err());
    if let Some(symbols) = symbols {
        let _ = (symbols.section(), symbols.strings(), symbols.has_extended_indexes());
        assert!(symbols.first_global().0 <= symbols.len());
        assert_eq!(symbols.locals().len() + symbols.globals().len(), symbols.len() as usize);
        for (index, symbol) in symbols.iter() {
            walk.symbols += 1;
            assert_eq!(symbols.get(index), Ok(symbol));
            walk.note(symbols.name(&symbol));
            if let Some(SymbolSection::Section(section)) =
                walk.note(symbols.section_of(index, &symbol))
            {
                // A section index the reader hands out is a section.
                assert!(object.section(section).is_ok());
            }
        }
        assert!(symbols.get(SymbolIndex(symbols.len())).is_err());
    }
    Ok(walk)
}

// ---- a deterministic pseudo-random generator -------------------------------

/// `SplitMix64` (public domain, Sebastiano Vigna).
#[derive(Debug, Clone)]
pub struct Rng(pub u64);

impl Rng {
    pub fn next(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9e37_79b9_7f4a_7c15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
        z ^ (z >> 31)
    }

    /// A value in `0..bound` (`bound` > 0).
    pub fn below(&mut self, bound: u64) -> u64 {
        self.next() % bound
    }
}

/// Machines for a class that the reader accepts.
pub fn machine_name(e_machine: u16) -> &'static str {
    match e_machine {
        EM_AARCH64 => "aarch64",
        EM_X86_64 => "x86-64",
        EM_386 => "i386",
        _ => "other",
    }
}
