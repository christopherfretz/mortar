//! The file header and what it identifies (`spec/elf.md` §3.1). Shared by
//! every reader of this crate.

use core::ops::Deref;

use crate::consts::{
    EI_CLASS, EI_DATA, EI_NIDENT, ELFCLASS32, ELFCLASS64, ELFDATA2LSB, ELFDATA2MSB, ELFMAG, EM_386,
    EM_AARCH64, EM_X86_64, EV_CURRENT, sht_name,
};
use crate::error::Error;
use crate::raw::{Class, Ehdr, Record};
use crate::reloc;

/// The header of a little-endian ELF file of either class.
///
/// Dereferences to the widened [`Ehdr`] fields.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FileHeader {
    /// `e_ident[EI_CLASS]`.
    pub class: Class,
    /// The fields of the header, widened.
    pub ehdr: Ehdr,
}

impl Deref for FileHeader {
    type Target = Ehdr;

    fn deref(&self) -> &Ehdr {
        &self.ehdr
    }
}

impl FileHeader {
    /// The bytes a reader of the file header takes from a view: its first
    /// 64 (the larger header), or, when those are not resident and the
    /// file is ELF32, the first 52, the whole ELF32 header. A parts view
    /// of an ELF32 object whose first section is a Mason container at
    /// offset 52 holds no byte past 52 (`spec/meta.md` MT-42: a container
    /// is never resident). `None` when neither is resident.
    pub fn head<'a>(view: &crate::view::View<'a>) -> Option<&'a [u8]> {
        view.prefix(64).or_else(|| {
            let ident = view.prefix(EI_NIDENT as u64)?;
            if ident.get(EI_CLASS) == Some(&ELFCLASS32) {
                view.prefix(Ehdr::size(Class::Elf32) as u64)
            } else {
                None
            }
        })
    }

    /// Reads the header of any little-endian ELF file: the magic, the
    /// class, the data encoding and the versions are checked, in that
    /// order; the file type and the machine are not.
    pub fn parse(data: &[u8]) -> Result<FileHeader, Error> {
        if data.first_chunk::<4>() != Some(&ELFMAG) {
            return Err(Error::NotElf);
        }
        let len = data.len() as u64;
        let Some(ident) = data.first_chunk::<EI_NIDENT>() else {
            return Err(Error::TruncatedHeader { len, expected: EI_NIDENT as u64 });
        };
        let class = match ident[EI_CLASS] {
            ELFCLASS32 => Class::Elf32,
            ELFCLASS64 => Class::Elf64,
            class => return Err(Error::InvalidClass { class }),
        };
        match ident[EI_DATA] {
            ELFDATA2LSB => {}
            ELFDATA2MSB => return Err(Error::BigEndian),
            data => return Err(Error::InvalidEncoding { data }),
        }
        let Some(ehdr) = Ehdr::decode(class, data) else {
            return Err(Error::TruncatedHeader { len, expected: Ehdr::size(class) as u64 });
        };
        if u32::from(ehdr.ei_version) != EV_CURRENT || ehdr.e_version != EV_CURRENT {
            return Err(Error::UnsupportedVersion {
                ei_version: ehdr.ei_version,
                e_version: ehdr.e_version,
            });
        }
        Ok(FileHeader { class, ehdr })
    }
}

/// A machine mortar links, in the one class it links it in.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Machine {
    /// `EM_AARCH64`, `ELFCLASS64` (LP64).
    AArch64,
    /// `EM_X86_64`, `ELFCLASS64`.
    X86_64,
    /// `EM_386`, `ELFCLASS32`.
    I386,
}

impl Machine {
    /// The machine of a file, or the error that names why mortar does not
    /// read it: another machine ([`Error::UnsupportedMachine`]), or one of
    /// the three in the other class ([`Error::UnsupportedClass`]).
    pub fn of(header: &FileHeader) -> Result<Machine, Error> {
        let machine = match header.e_machine {
            EM_AARCH64 => Machine::AArch64,
            EM_X86_64 => Machine::X86_64,
            EM_386 => Machine::I386,
            e_machine => return Err(Error::UnsupportedMachine { e_machine }),
        };
        if machine.class() != header.class {
            return Err(Error::UnsupportedClass {
                class: header.class,
                e_machine: header.e_machine,
            });
        }
        Ok(machine)
    }

    /// The `e_machine` value.
    pub const fn e_machine(self) -> u16 {
        match self {
            Machine::AArch64 => EM_AARCH64,
            Machine::X86_64 => EM_X86_64,
            Machine::I386 => EM_386,
        }
    }

    /// The class of the machine's objects.
    pub const fn class(self) -> Class {
        match self {
            Machine::AArch64 | Machine::X86_64 => Class::Elf64,
            Machine::I386 => Class::Elf32,
        }
    }

    /// `"aarch64"`, `"x86-64"` or `"i386"`.
    pub const fn name(self) -> &'static str {
        match self {
            Machine::AArch64 => "aarch64",
            Machine::X86_64 => "x86-64",
            Machine::I386 => "i386",
        }
    }

    /// The name of one of the machine's relocation types ([`reloc::name`]).
    pub fn reloc_name(self, r_type: u32) -> Option<&'static str> {
        reloc::name(self.e_machine(), r_type)
    }

    /// The name of a section type in the machine's objects ([`sht_name`]).
    pub fn section_type_name(self, sh_type: u32) -> Option<&'static str> {
        sht_name(self.e_machine(), sh_type)
    }
}
