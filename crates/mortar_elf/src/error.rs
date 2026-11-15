//! What can be wrong with an ELF file (`spec/elf.md` §4).

use core::fmt;

use crate::consts::{
    ELFCLASS32, ELFDATA2MSB, EM_AARCH64, EM_X86_64, em_name, et_name, sht_generic_name,
};
use crate::raw::Class;

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
            | Error::StringOutOfBounds { section, .. }
            | Error::StringNotTerminated { section, .. }
            | Error::TooManySymbols { section, .. }
            | Error::BadFirstGlobal { section, .. }
            | Error::ShndxCountMismatch { section, .. }
            | Error::EmptyGroup { section }
            | Error::GroupMemberOutOfRange { section, .. }
            | Error::NoteAlignment { section, .. }
            | Error::BadNote { section, .. }
            | Error::CompressedTooShort { section, .. }
            | Error::CompressedNobits { section } => Some(section),
            _ => None,
        }
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
                "section {section}: string offset {offset} is past the end of the string table \
                 ({size} bytes)"
            ),
            Error::StringNotTerminated { section, offset } => {
                write!(f, "section {section}: string at offset {offset} is not NUL-terminated")
            }
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
            Error::BadNote { section, offset, part } => {
                let part = match part {
                    NotePart::Header => "note header",
                    NotePart::Name => "note name",
                    NotePart::Descriptor => "note descriptor",
                    NotePart::Property => "program property",
                };
                write!(
                    f,
                    "section {section}: {part} at offset {offset:#x} extends past the end of \
                     its container"
                )
            }
            Error::CompressedTooShort { section, size, expected } => write!(
                f,
                "section {section}: SHF_COMPRESSED section of {size} bytes is too short for its \
                 compression header ({expected} bytes)"
            ),
            Error::CompressedNobits { section } => {
                write!(f, "section {section}: SHF_COMPRESSED on a SHT_NOBITS section")
            }
        }
    }
}

impl core::error::Error for Error {}
