//! Relocatable objects (`spec/elf.md` §3).

use crate::consts::{ET_REL, SHT_SYMTAB};
use crate::error::Error;
use crate::header::{FileHeader, Machine};
use crate::raw::Class;
use crate::section::{Section, SectionIndex, SectionIter, SectionTable};
use crate::symbol::SymbolTable;

/// A relocatable object (`ET_REL`) for one of the machines mortar links.
///
/// A view of the file's bytes: `Copy`, borrows only the input, owns
/// nothing. [`Object::parse`] takes constant time; everything else is
/// decoded and checked when it is asked for.
#[derive(Debug, Clone, Copy)]
pub struct Object<'a> {
    header: FileHeader,
    machine: Machine,
    sections: SectionTable<'a>,
}

impl<'a> Object<'a> {
    /// Reads the file header and locates the section header table.
    ///
    /// Checked here, in this order: the header ([`FileHeader::parse`]),
    /// the machine and its class ([`Machine::of`]), that `e_type` is
    /// `ET_REL`, and that the section header table and the section name
    /// string table lie within the file ([`SectionTable::parse`]).
    pub fn parse(data: &'a [u8]) -> Result<Object<'a>, Error> {
        let header = FileHeader::parse(data)?;
        let machine = Machine::of(&header)?;
        if header.e_type != ET_REL {
            return Err(Error::NotRelocatable { e_type: header.e_type });
        }
        let sections = SectionTable::parse(data, &header)?;
        Ok(Object { header, machine, sections })
    }

    /// The whole file.
    pub fn data(&self) -> &'a [u8] {
        self.sections.file()
    }

    /// The file header.
    pub fn header(&self) -> &FileHeader {
        &self.header
    }

    pub fn class(&self) -> Class {
        self.header.class
    }

    pub fn machine(&self) -> Machine {
        self.machine
    }

    /// The section header table.
    pub fn section_table(&self) -> SectionTable<'a> {
        self.sections
    }

    /// The number of sections, section 0 included.
    pub fn section_count(&self) -> u32 {
        self.sections.len()
    }

    /// Section `index`.
    pub fn section(&self, index: SectionIndex) -> Result<Section<'a>, Error> {
        self.sections.get(index)
    }

    /// Every section in index order, section 0 included.
    pub fn sections(&self) -> SectionIter<'a> {
        self.sections.iter()
    }

    /// The symbol table (`SHT_SYMTAB`), or `None` when the object has
    /// none ([`SectionTable::find_symbol_table`]).
    pub fn symbols(&self) -> Result<Option<SymbolTable<'a>>, Error> {
        self.sections.find_symbol_table(SHT_SYMTAB)
    }
}
