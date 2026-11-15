//! What can be wrong with an ELF file (`spec/elf.md` §4, §6.11).

use core::fmt;

use crate::consts::{
    ELFCLASS32, ELFDATA2MSB, EM_AARCH64, EM_X86_64, PN_XNUM, dt_name, em_name, et_name, pt_name,
    sht_generic_name,
};
use crate::raw::Class;
use crate::strtab::DYNAMIC_STRINGS;

/// The kind of version record an [`Error::BadVersion`] is about.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VersionRecord {
    /// A version definition, `Elf_Verdef`.
    Definition,
    /// A name of a version definition, `Elf_Verdaux`.
    DefinitionName,
    /// The versions needed from one file, `Elf_Verneed`.
    Need,
    /// One needed version, `Elf_Vernaux`.
    NeedEntry,
}

/// What is wrong with a version record ([`Error::BadVersion`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VersionFault {
    /// The record does not lie within its table.
    Truncated,
    /// Its next-record offset is 0, but the count says more records follow.
    ChainEnds,
    /// `vd_version` or `vn_version` is not 1 (`VER_DEF_CURRENT`,
    /// `VER_NEED_CURRENT`).
    Revision(u16),
    /// The count of records is more than the table's bytes can hold.
    TooMany(u64),
    /// A version definition with no name (`vd_cnt` is 0).
    NoName,
}

/// Which part of a note does not fit ([`Error::BadNote`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NotePart {
    /// The 12-byte note header.
    Header,
    /// The name, `n_namesz` bytes.
    Name,
    /// The descriptor, `n_descsz` bytes.
    Descriptor,
    /// A program property's 8-byte header or its data.
    Property,
}

/// A malformed or unsupported file. Every variant says what is wrong and
/// where; section and symbol numbers are indexes into the file's tables.
///
/// `Display` gives the text after the file name of a diagnostic:
/// `section 7: sh_entsize is 23, expected 24`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum Error {
    // ---- identification (§3.1) --------------------------------------------
    /// The file does not start with the ELF magic.
    NotElf,
    /// The file ends inside its file header.
    TruncatedHeader { len: u64, expected: u64 },
    /// `e_ident[EI_CLASS]` is neither `ELFCLASS32` nor `ELFCLASS64`.
    InvalidClass { class: u8 },
    /// `e_ident[EI_DATA]` is `ELFDATA2MSB`.
    BigEndian,
    /// `e_ident[EI_DATA]` is neither encoding.
    InvalidEncoding { data: u8 },
    /// `e_ident[EI_VERSION]` or `e_version` is not `EV_CURRENT`.
    UnsupportedVersion { ei_version: u8, e_version: u32 },
    /// `e_machine` is not one mortar links.
    UnsupportedMachine { e_machine: u16 },
    /// A machine mortar links in a class it does not: `ELFCLASS32` with
    /// `EM_X86_64` (the x32 ABI) or `EM_AARCH64` (ILP32), `ELFCLASS64`
    /// with `EM_386`.
    UnsupportedClass { class: Class, e_machine: u16 },
    /// `e_type` is not `ET_REL`.
    NotRelocatable { e_type: u16 },

    // ---- the section header table (§3.2) ----------------------------------
    /// `e_shentsize` is not the class's section header size.
    BadShentsize { e_shentsize: u16, expected: u16 },
    /// `e_shoff` is 0 and `e_shnum` is not.
    MissingSectionTable { e_shnum: u16 },
    /// More sections than a 32-bit index can address.
    TooManySections { count: u64 },
    /// The section header table does not lie within the file.
    SectionTableOutOfBounds { offset: u64, count: u64, file_len: u64 },
    /// The section name string table index is not a section.
    BadShstrndx { index: u32, count: u32 },
    /// A section index given to an accessor is not a section.
    SectionIndexOutOfRange { index: u32, count: u32 },

    // ---- one section (§3.3, §3.4) ------------------------------------------
    /// The section's contents do not lie within the file.
    SectionOutOfBounds { section: u32, offset: u64, size: u64, file_len: u64 },
    /// The section does not have the type the accessor reads.
    WrongSectionType { section: u32, sh_type: u32, expected: u32 },
    /// `sh_entsize` is not the size of the section's records.
    BadEntsize { section: u32, entsize: u64, expected: u64 },
    /// `sh_size` is not a whole number of entries.
    BadSize { section: u32, size: u64, entsize: u64 },
    /// `sh_addralign` is not 0 or a power of two.
    BadAlignment { section: u32, align: u64 },
    /// `sh_link` is not a section.
    BadLink { section: u32, link: u32, count: u32 },
    /// `sh_info` is read as a section index and is not a section.
    BadInfo { section: u32, info: u32, count: u32 },
    /// A string table offset is not within the table.
    StringOutOfBounds { section: u32, offset: u32, size: u64 },
    /// The string at an offset runs to the end of the table without a NUL.
    StringNotTerminated { section: u32, offset: u32 },

    // ---- symbols (§3.5) -----------------------------------------------------
    /// More than one section of a symbol table type.
    MultipleSymbolTables { first: u32, second: u32 },
    /// More than one `SHT_SYMTAB_SHNDX` section for one symbol table.
    MultipleShndxTables { first: u32, second: u32 },
    /// More symbols than a 32-bit index can address.
    TooManySymbols { section: u32, count: u64 },
    /// `sh_info` of a symbol table (the first non-local symbol) is past
    /// its end.
    BadFirstGlobal { section: u32, sh_info: u32, count: u32 },
    /// The `SHT_SYMTAB_SHNDX` section does not have one entry per symbol.
    ShndxCountMismatch { section: u32, entries: u64, symbols: u32 },
    /// A symbol index is not in the symbol table.
    SymbolIndexOutOfRange { index: u32, count: u32 },
    /// A symbol's `st_shndx` is `SHN_XINDEX` and the symbol table has no
    /// `SHT_SYMTAB_SHNDX` section.
    MissingShndx { symbol: u32 },
    /// A symbol's section index is not a section.
    SymbolSectionOutOfRange { symbol: u32, index: u32, count: u32 },

    // ---- groups, notes, compression (§3.7 to §3.9) -------------------------
    /// A group section too short for its flag word.
    EmptyGroup { section: u32 },
    /// A group member is not a section (or is section 0).
    GroupMemberOutOfRange { section: u32, member: u32, index: u32, count: u32 },
    /// A note section's alignment is neither at most 4 nor 8.
    NoteAlignment { section: u32, align: u64 },
    /// A note, or a program property in one, runs past the end of its
    /// container. `offset` is the note's or property's, from the start
    /// of the section.
    BadNote { section: u32, offset: u64, part: NotePart },
    /// A `SHF_COMPRESSED` section too short for its compression header.
    CompressedTooShort { section: u32, size: u64, expected: u64 },
    /// A `SHF_COMPRESSED` section without contents in the file
    /// (`SHT_NOBITS`).
    CompressedNobits { section: u32 },

    // ---- shared objects: identification and program headers (§6.1, §6.2) --
    /// `e_type` is not `ET_DYN`.
    NotSharedObject { e_type: u16 },
    /// `e_phentsize` is not the class's program header size.
    BadPhentsize { e_phentsize: u16, expected: u16 },
    /// `e_phoff` is 0 and `e_phnum` is not.
    MissingProgramHeaderTable { e_phnum: u16 },
    /// `e_phnum` is `PN_XNUM` and the file has no section header 0 to
    /// hold the count.
    PhnumEscapeWithoutSections,
    /// The program header table does not lie within the file.
    ProgramHeadersOutOfBounds { offset: u64, count: u64, file_len: u64 },
    /// A program header index given to an accessor is not a program
    /// header.
    SegmentIndexOutOfRange { index: u32, count: u32 },
    /// A segment's file contents do not lie within the file.
    SegmentOutOfBounds { segment: u32, offset: u64, size: u64, file_len: u64 },
    /// `p_align` is not 0 or a power of two.
    BadSegmentAlignment { segment: u32, align: u64 },
    /// The segment does not have the type the accessor reads.
    WrongSegmentType { segment: u32, p_type: u32, expected: u32 },
    /// More than one segment of a type the file may have once.
    MultipleSegments { p_type: u32, first: u32, second: u32 },
    /// A note segment's alignment is neither at most 4 nor 8.
    SegmentNoteAlignment { segment: u32, align: u64 },
    /// A note, or a program property in one, runs past the end of its
    /// segment. `offset` is from the start of the segment.
    BadSegmentNote { segment: u32, offset: u64, part: NotePart },

    // ---- shared objects: the parts and the dynamic table (§6.3, §6.4) ------
    /// More than one section of a type the file may have once.
    MultipleSections { sh_type: u32, first: u32, second: u32 },
    /// The `SHT_GNU_versym` section's `sh_link` is not the dynamic symbol
    /// table.
    VersymLink { section: u32, link: u32 },
    /// The dynamic table has no `DT_NULL` entry.
    DynamicNotTerminated { entries: u64 },
    /// A tag the dynamic table may have once appears twice.
    DuplicateDynamicTag { d_tag: i64, first: u32, second: u32 },
    /// A tag is present without the tag it needs (`DT_STRTAB` without
    /// `DT_STRSZ`, `DT_VERDEF` without `DT_VERDEFNUM`, ...).
    MissingDynamicTag { d_tag: i64, needed_by: i64 },
    /// The bytes a dynamic tag addresses are not file bytes of a `PT_LOAD`
    /// segment.
    AddressNotMapped { d_tag: i64, addr: u64, size: u64 },
    /// An entry-size tag (`DT_SYMENT`) is not the record's size.
    BadDynamicEntsize { d_tag: i64, value: u64, expected: u64 },
    /// `DT_SYMTAB` without `DT_HASH` or `DT_GNU_HASH`: without section
    /// headers, nothing says how many dynamic symbols there are.
    NoSymbolCount,
    /// A hash table that does not lie within its segment, or whose chains
    /// do not end.
    BadHashTable { d_tag: i64, addr: u64 },
    /// `DT_HASH` and `DT_GNU_HASH` give different dynamic symbol counts.
    SymbolCountMismatch { hash: u64, gnu_hash: u64 },

    // ---- shared objects: symbol versions (§6.7) ----------------------------
    /// The symbol version table does not have one entry per dynamic
    /// symbol.
    VersymCountMismatch { entries: u64, symbols: u32 },
    /// A malformed version definition or need. `offset` is the record's
    /// offset in the file.
    BadVersion { record: VersionRecord, offset: u64, fault: VersionFault },
    /// A symbol's version index is neither 0, 1, a defined version nor a
    /// needed one.
    UnknownVersionIndex { symbol: u32, index: u16 },
    /// A version index is defined or needed more than once.
    AmbiguousVersionIndex { index: u16 },
}

impl Error {
    /// The section the error is about, if it is about one.
    pub fn section(&self) -> Option<u32> {
        match *self {
            Error::SectionOutOfBounds { section, .. }
            | Error::WrongSectionType { section, .. }
            | Error::BadEntsize { section, .. }
            | Error::BadSize { section, .. }
            | Error::BadAlignment { section, .. }
            | Error::BadLink { section, .. }
            | Error::BadInfo { section, .. }
            | Error::TooManySymbols { section, .. }
            | Error::BadFirstGlobal { section, .. }
            | Error::ShndxCountMismatch { section, .. }
            | Error::EmptyGroup { section }
            | Error::GroupMemberOutOfRange { section, .. }
            | Error::NoteAlignment { section, .. }
            | Error::BadNote { section, .. }
            | Error::CompressedTooShort { section, .. }
            | Error::CompressedNobits { section }
            | Error::VersymLink { section, .. } => Some(section),
            Error::StringOutOfBounds { section, .. }
            | Error::StringNotTerminated { section, .. }
                if section != DYNAMIC_STRINGS =>
            {
                Some(section)
            }
            _ => None,
        }
    }

    /// The program header (segment) the error is about, if it is about
    /// one.
    pub fn segment(&self) -> Option<u32> {
        match *self {
            Error::SegmentOutOfBounds { segment, .. }
            | Error::BadSegmentAlignment { segment, .. }
            | Error::WrongSegmentType { segment, .. }
            | Error::SegmentNoteAlignment { segment, .. }
            | Error::BadSegmentNote { segment, .. } => Some(segment),
            _ => None,
        }
    }
}

/// `DT_NAME` or the number in hexadecimal.
struct TagName(i64);

impl fmt::Display for TagName {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match dt_name(self.0) {
            Some(name) => f.write_str(name),
            None => write!(f, "dynamic tag {:#x}", self.0),
        }
    }
}

/// `PT_NAME` or the number in hexadecimal.
struct SegmentType(u32);

impl fmt::Display for SegmentType {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match pt_name(self.0) {
            Some(name) => f.write_str(name),
            None => write!(f, "{:#x}", self.0),
        }
    }
}

/// Where a string error is: `section N` or the dynamic string table.
struct StringPlace(u32);

impl fmt::Display for StringPlace {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if self.0 == DYNAMIC_STRINGS {
            f.write_str("dynamic string table (DT_STRTAB)")
        } else {
            write!(f, "section {}", self.0)
        }
    }
}

fn note_part(part: NotePart) -> &'static str {
    match part {
        NotePart::Header => "note header",
        NotePart::Name => "note name",
        NotePart::Descriptor => "note descriptor",
        NotePart::Property => "program property",
    }
}

/// `SHT_NAME` or the number in hexadecimal.
struct TypeName(u32);

impl fmt::Display for TypeName {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match sht_generic_name(self.0) {
            Some(name) => f.write_str(name),
            None => write!(f, "{:#x}", self.0),
        }
    }
}

impl fmt::Display for Error {
    #[allow(clippy::too_many_lines)] // one arm per variant
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match *self {
            Error::NotElf => f.write_str("not an ELF file (bad magic)"),
            Error::TruncatedHeader { len, expected } => {
                write!(f, "file is {len} bytes, too short for an ELF header ({expected} bytes)")
            }
            Error::InvalidClass { class } => write!(f, "invalid ELF class {class}"),
            Error::BigEndian => {
                write!(f, "big-endian ELF (ELFDATA2MSB, {ELFDATA2MSB}) is not supported")
            }
            Error::InvalidEncoding { data } => write!(f, "invalid ELF data encoding {data}"),
            Error::UnsupportedVersion { ei_version, e_version } => write!(
                f,
                "unsupported ELF version (e_ident[EI_VERSION] is {ei_version}, e_version is \
                 {e_version}, expected 1)"
            ),
            Error::UnsupportedMachine { e_machine } => match em_name(e_machine) {
                Some(name) => write!(f, "unsupported machine {name} ({e_machine})"),
                None => write!(f, "unsupported machine {e_machine}"),
            },
            Error::UnsupportedClass { class, e_machine } => match (class, e_machine) {
                (Class::Elf32, EM_X86_64) => write!(
                    f,
                    "the x32 ABI (ELF32 for EM_X86_64, ELFCLASS32 = {ELFCLASS32}) is not supported"
                ),
                (Class::Elf32, EM_AARCH64) => {
                    f.write_str("AArch64 ILP32 (ELF32 for EM_AARCH64) is not supported")
                }
                _ => write!(
                    f,
                    "{} is not a class of {}",
                    class.name(),
                    em_name(e_machine).unwrap_or("this machine")
                ),
            },
            Error::NotRelocatable { e_type } => match et_name(e_type) {
                Some(name) => write!(f, "not a relocatable object: e_type is {name}"),
                None => write!(f, "not a relocatable object: e_type is {e_type:#x}"),
            },
            Error::BadShentsize { e_shentsize, expected } => {
                write!(f, "e_shentsize is {e_shentsize}, expected {expected}")
            }
            Error::MissingSectionTable { e_shnum } => {
                write!(f, "e_shoff is 0 but e_shnum is {e_shnum}")
            }
            Error::TooManySections { count } => {
                write!(f, "section count {count} does not fit in 32 bits")
            }
            Error::SectionTableOutOfBounds { offset, count, file_len } => write!(
                f,
                "section header table ({count} entries at offset {offset:#x}) extends past the \
                 end of the file ({file_len} bytes)"
            ),
            Error::BadShstrndx { index, count } => write!(
                f,
                "section name string table index is {index}, but the file has {count} sections"
            ),
            Error::SectionIndexOutOfRange { index, count } => {
                write!(f, "section index {index} is out of range: the file has {count} sections")
            }
            Error::SectionOutOfBounds { section, offset, size, file_len } => write!(
                f,
                "section {section}: contents ({size:#x} bytes at offset {offset:#x}) extend past \
                 the end of the file ({file_len} bytes)"
            ),
            Error::WrongSectionType { section, sh_type, expected } => write!(
                f,
                "section {section}: type is {}, expected {}",
                TypeName(sh_type),
                TypeName(expected)
            ),
            Error::BadEntsize { section, entsize, expected } => {
                write!(f, "section {section}: sh_entsize is {entsize}, expected {expected}")
            }
            Error::BadSize { section, size, entsize } => write!(
                f,
                "section {section}: sh_size {size} is not a multiple of the entry size {entsize}"
            ),
            Error::BadAlignment { section, align } => {
                write!(f, "section {section}: sh_addralign {align} is not a power of two")
            }
            Error::BadLink { section, link, count } => {
                write!(f, "section {section}: sh_link is {link}, but the file has {count} sections")
            }
            Error::BadInfo { section, info, count } => {
                write!(f, "section {section}: sh_info is {info}, but the file has {count} sections")
            }
            Error::StringOutOfBounds { section, offset, size } => write!(
                f,
                "{}: string offset {offset} is past the end of the string table ({size} bytes)",
                StringPlace(section)
            ),
            Error::StringNotTerminated { section, offset } => write!(
                f,
                "{}: string at offset {offset} is not NUL-terminated",
                StringPlace(section)
            ),
            Error::MultipleSymbolTables { first, second } => {
                write!(f, "more than one symbol table: sections {first} and {second}")
            }
            Error::MultipleShndxTables { first, second } => write!(
                f,
                "more than one SHT_SYMTAB_SHNDX section for one symbol table: sections {first} \
                 and {second}"
            ),
            Error::TooManySymbols { section, count } => {
                write!(f, "section {section}: symbol count {count} does not fit in 32 bits")
            }
            Error::BadFirstGlobal { section, sh_info, count } => write!(
                f,
                "section {section}: sh_info (first non-local symbol) is {sh_info}, but the table \
                 has {count} symbols"
            ),
            Error::ShndxCountMismatch { section, entries, symbols } => write!(
                f,
                "section {section}: SHT_SYMTAB_SHNDX has {entries} entries, but the symbol table \
                 has {symbols} symbols"
            ),
            Error::SymbolIndexOutOfRange { index, count } => write!(
                f,
                "symbol index {index} is out of range: the symbol table has {count} symbols"
            ),
            Error::MissingShndx { symbol } => write!(
                f,
                "symbol {symbol}: st_shndx is SHN_XINDEX, but there is no SHT_SYMTAB_SHNDX section"
            ),
            Error::SymbolSectionOutOfRange { symbol, index, count } => write!(
                f,
                "symbol {symbol}: section index {index} is out of range: the file has {count} \
                 sections"
            ),
            Error::EmptyGroup { section } => {
                write!(f, "section {section}: group section has no flag word")
            }
            Error::GroupMemberOutOfRange { section, member, index, count } => write!(
                f,
                "section {section}: group member {member} is section index {index}, but the file \
                 has sections 1 to {}",
                count.saturating_sub(1)
            ),
            Error::NoteAlignment { section, align } => write!(
                f,
                "section {section}: note section alignment is {align}, expected at most 4, or 8"
            ),
            Error::BadNote { section, offset, part } => write!(
                f,
                "section {section}: {} at offset {offset:#x} extends past the end of its \
                 container",
                note_part(part)
            ),
            Error::CompressedTooShort { section, size, expected } => write!(
                f,
                "section {section}: SHF_COMPRESSED section of {size} bytes is too short for its \
                 compression header ({expected} bytes)"
            ),
            Error::CompressedNobits { section } => {
                write!(f, "section {section}: SHF_COMPRESSED on a SHT_NOBITS section")
            }
            Error::NotSharedObject { e_type } => match et_name(e_type) {
                Some(name) => write!(f, "not a shared object: e_type is {name}"),
                None => write!(f, "not a shared object: e_type is {e_type:#x}"),
            },
            Error::BadPhentsize { e_phentsize, expected } => {
                write!(f, "e_phentsize is {e_phentsize}, expected {expected}")
            }
            Error::MissingProgramHeaderTable { e_phnum } => {
                write!(f, "e_phoff is 0 but e_phnum is {e_phnum}")
            }
            Error::PhnumEscapeWithoutSections => write!(
                f,
                "e_phnum is PN_XNUM ({PN_XNUM:#x}) but there is no section header 0 to hold the \
                 count"
            ),
            Error::ProgramHeadersOutOfBounds { offset, count, file_len } => write!(
                f,
                "program header table ({count} entries at offset {offset:#x}) extends past the \
                 end of the file ({file_len} bytes)"
            ),
            Error::SegmentIndexOutOfRange { index, count } => write!(
                f,
                "program header index {index} is out of range: the file has {count} program \
                 headers"
            ),
            Error::SegmentOutOfBounds { segment, offset, size, file_len } => write!(
                f,
                "segment {segment}: contents ({size:#x} bytes at offset {offset:#x}) extend past \
                 the end of the file ({file_len} bytes)"
            ),
            Error::BadSegmentAlignment { segment, align } => {
                write!(f, "segment {segment}: p_align {align} is not a power of two")
            }
            Error::WrongSegmentType { segment, p_type, expected } => write!(
                f,
                "segment {segment}: type is {}, expected {}",
                SegmentType(p_type),
                SegmentType(expected)
            ),
            Error::MultipleSegments { p_type, first, second } => write!(
                f,
                "more than one {} segment: program headers {first} and {second}",
                SegmentType(p_type)
            ),
            Error::SegmentNoteAlignment { segment, align } => write!(
                f,
                "segment {segment}: note segment alignment is {align}, expected at most 4, or 8"
            ),
            Error::BadSegmentNote { segment, offset, part } => write!(
                f,
                "segment {segment}: {} at offset {offset:#x} extends past the end of the segment",
                note_part(part)
            ),
            Error::MultipleSections { sh_type, first, second } => write!(
                f,
                "more than one {} section: sections {first} and {second}",
                TypeName(sh_type)
            ),
            Error::VersymLink { section, link } => write!(
                f,
                "section {section}: the symbol version table's sh_link is {link}, which is not \
                 the dynamic symbol table"
            ),
            Error::DynamicNotTerminated { entries } => {
                write!(f, "the dynamic table ({entries} entries) has no DT_NULL entry")
            }
            Error::DuplicateDynamicTag { d_tag, first, second } => write!(
                f,
                "the dynamic table has more than one {}: entries {first} and {second}",
                TagName(d_tag)
            ),
            Error::MissingDynamicTag { d_tag, needed_by } => {
                write!(f, "the dynamic table has {} but no {}", TagName(needed_by), TagName(d_tag))
            }
            Error::AddressNotMapped { d_tag, addr, size } => write!(
                f,
                "{}: {size:#x} bytes at address {addr:#x} are not in the file contents of a \
                 PT_LOAD segment",
                TagName(d_tag)
            ),
            Error::BadDynamicEntsize { d_tag, value, expected } => {
                write!(f, "{} is {value}, expected {expected}", TagName(d_tag))
            }
            Error::NoSymbolCount => f.write_str(
                "the dynamic table has DT_SYMTAB but neither DT_HASH nor DT_GNU_HASH: the number \
                 of dynamic symbols is not known",
            ),
            Error::BadHashTable { d_tag, addr } => write!(
                f,
                "{}: the hash table at address {addr:#x} does not lie within its segment",
                TagName(d_tag)
            ),
            Error::SymbolCountMismatch { hash, gnu_hash } => {
                write!(f, "DT_HASH counts {hash} dynamic symbols but DT_GNU_HASH counts {gnu_hash}")
            }
            Error::VersymCountMismatch { entries, symbols } => write!(
                f,
                "the symbol version table has {entries} entries, but the dynamic symbol table \
                 has {symbols} symbols"
            ),
            Error::BadVersion { record, offset, fault } => {
                let record = match record {
                    VersionRecord::Definition => "version definition",
                    VersionRecord::DefinitionName => "version definition name",
                    VersionRecord::Need => "version need",
                    VersionRecord::NeedEntry => "version need entry",
                };
                write!(f, "{record} at file offset {offset:#x}: ")?;
                match fault {
                    VersionFault::Truncated => f.write_str("extends past the end of its table"),
                    VersionFault::ChainEnds => {
                        f.write_str("its next offset is 0, but the count says more follow")
                    }
                    VersionFault::Revision(version) => {
                        write!(f, "revision is {version}, expected 1")
                    }
                    VersionFault::TooMany(count) => {
                        write!(f, "the count {count} is more than the table can hold")
                    }
                    VersionFault::NoName => f.write_str("vd_cnt is 0: the version has no name"),
                }
            }
            Error::UnknownVersionIndex { symbol, index } => {
                write!(f, "symbol {symbol}: version index {index} is neither defined nor needed")
            }
            Error::AmbiguousVersionIndex { index } => {
                write!(f, "version index {index} is defined or needed more than once")
            }
        }
    }
}

impl core::error::Error for Error {}
