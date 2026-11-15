//! Symbol tables (`spec/elf.md` §3.5, §6.5). Shared by every reader of
//! this crate.

use core::fmt;

use crate::consts::{SHN_ABS, SHN_COMMON, SHN_LORESERVE, SHN_UNDEF, SHN_XINDEX};
use crate::error::Error;
use crate::raw::{Sym, Table, Word};
use crate::section::SectionIndex;
use crate::strtab::StrTab;

/// The index of a symbol in a symbol table.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct SymbolIndex(pub u32);

impl fmt::Display for SymbolIndex {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(f)
    }
}

/// Where a symbol is defined: `st_shndx` with the `SHN_XINDEX` escape
/// resolved ([`SymbolTable::section_of`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum SymbolSection {
    /// `SHN_UNDEF`: not defined in this file.
    Undefined,
    /// `SHN_ABS`: an absolute value.
    Absolute,
    /// `SHN_COMMON`: a common symbol; `st_value` is its alignment.
    Common,
    /// Defined in this section of the file. The index is a section.
    Section(SectionIndex),
    /// Any other reserved value (`SHN_LORESERVE..=SHN_HIRESERVE`), for
    /// example `SHN_X86_64_LCOMMON`.
    Reserved(u16),
}

/// A symbol table with its string table and its extended section
/// indexes. `Copy`, and borrows only the input bytes.
#[derive(Debug, Clone, Copy)]
pub struct SymbolTable<'a> {
    symbols: Table<'a, Sym>,
    strings: StrTab<'a>,
    shndx: Option<Table<'a, Word>>,
    section: u32,
    first_global: u32,
    /// The number of sections a section index is checked against; `None`
    /// for a table found without section headers (§6.5).
    section_count: Option<u32>,
}

impl<'a> SymbolTable<'a> {
    pub(crate) fn new(
        symbols: Table<'a, Sym>,
        strings: StrTab<'a>,
        shndx: Option<Table<'a, Word>>,
        section: u32,
        first_global: u32,
        section_count: Option<u32>,
    ) -> Self {
        SymbolTable { symbols, strings, shndx, section, first_global, section_count }
    }

    /// The index of the symbol table's section; 0 for a table found
    /// through the dynamic table (`spec/elf.md` §6.5).
    pub fn section(&self) -> SectionIndex {
        SectionIndex(self.section)
    }

    /// The number of symbols, symbol 0 included.
    pub fn len(&self) -> u32 {
        self.symbols.len() as u32
    }

    pub fn is_empty(&self) -> bool {
        self.symbols.is_empty()
    }

    /// `sh_info`: the index of the first symbol that is not `STB_LOCAL`,
    /// at most `len()`. The bindings of the symbols are not checked
    /// against it.
    pub fn first_global(&self) -> SymbolIndex {
        SymbolIndex(self.first_global)
    }

    /// The symbol records as a table (indexable, sliceable).
    pub fn records(&self) -> Table<'a, Sym> {
        self.symbols
    }

    /// The string table the names are in.
    pub fn strings(&self) -> StrTab<'a> {
        self.strings
    }

    /// Whether the table has a `SHT_SYMTAB_SHNDX` section.
    pub fn has_extended_indexes(&self) -> bool {
        self.shndx.is_some()
    }

    /// Symbol `index`.
    #[inline]
    pub fn get(&self, index: SymbolIndex) -> Result<Sym, Error> {
        match self.symbols.get(index.0 as usize) {
            Some(symbol) => Ok(symbol),
            None => Err(Error::SymbolIndexOutOfRange { index: index.0, count: self.len() }),
        }
    }

    fn range(
        &self,
        start: u32,
        end: u32,
    ) -> impl ExactSizeIterator<Item = (SymbolIndex, Sym)> + use<'a> {
        let part = self.symbols.slice(start as usize, end as usize);
        (start..end).map(SymbolIndex).zip(part.unwrap_or(Table::empty(self.symbols.class())))
    }

    /// Every symbol with its index, symbol 0 included.
    pub fn iter(&self) -> impl ExactSizeIterator<Item = (SymbolIndex, Sym)> + use<'a> {
        self.range(0, self.len())
    }

    /// The symbols before [`SymbolTable::first_global`].
    pub fn locals(&self) -> impl ExactSizeIterator<Item = (SymbolIndex, Sym)> + use<'a> {
        self.range(0, self.first_global)
    }

    /// The symbols from [`SymbolTable::first_global`] on.
    pub fn globals(&self) -> impl ExactSizeIterator<Item = (SymbolIndex, Sym)> + use<'a> {
        self.range(self.first_global, self.len())
    }

    /// The name of `symbol`, a record of this table.
    #[inline]
    pub fn name(&self, symbol: &Sym) -> Result<&'a [u8], Error> {
        self.strings.get(symbol.st_name)
    }

    /// Where `symbol`, the record at `index` of this table, is defined:
    /// `st_shndx`, or for `SHN_XINDEX` entry `index` of the
    /// `SHT_SYMTAB_SHNDX` section. A section index is checked to be a
    /// section of the file, except in a table found through the dynamic
    /// table, whose file may have no sections (`spec/elf.md` §6.5).
    #[inline]
    pub fn section_of(&self, index: SymbolIndex, symbol: &Sym) -> Result<SymbolSection, Error> {
        let section = match symbol.st_shndx {
            SHN_UNDEF => return Ok(SymbolSection::Undefined),
            SHN_ABS => return Ok(SymbolSection::Absolute),
            SHN_COMMON => return Ok(SymbolSection::Common),
            SHN_XINDEX => match self.shndx.and_then(|table| table.get(index.0 as usize)) {
                Some(Word(0)) => return Ok(SymbolSection::Undefined),
                Some(Word(section)) => section,
                None if self.shndx.is_some() => {
                    return Err(Error::SymbolIndexOutOfRange { index: index.0, count: self.len() });
                }
                None => return Err(Error::MissingShndx { symbol: index.0 }),
            },
            reserved if reserved >= SHN_LORESERVE => return Ok(SymbolSection::Reserved(reserved)),
            section => u32::from(section),
        };
        match self.section_count {
            Some(count) if section >= count => {
                Err(Error::SymbolSectionOutOfRange { symbol: index.0, index: section, count })
            }
            _ => Ok(SymbolSection::Section(SectionIndex(section))),
        }
    }
}
