//! The section header table and the views of one section (`spec/elf.md`
//! §3.2, §3.4, §3.6, §3.7, §3.9, §3.10). Shared by every reader of this
//! crate.

use core::fmt;
use core::iter::FusedIterator;
use core::ops::Range;

use crate::consts::{
    GRP_COMDAT, SHF_COMPRESSED, SHF_MERGE, SHF_STRINGS, SHN_XINDEX, SHT_DYNSYM, SHT_GROUP,
    SHT_NOBITS, SHT_NOTE, SHT_NULL, SHT_REL, SHT_RELA, SHT_STRTAB, SHT_SYMTAB, SHT_SYMTAB_SHNDX,
};
use crate::error::Error;
use crate::header::FileHeader;
use crate::note::Notes;
use crate::raw::{Chdr, Class, Record, Rel, Rela, Shdr, Table, TableIter, Word};
use crate::strtab::StrTab;
use crate::symbol::{SymbolIndex, SymbolTable};

/// The index of a section in a file's section header table.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct SectionIndex(pub u32);

impl fmt::Display for SectionIndex {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(f)
    }
}

/// The section header table of a file and its section name string table.
/// `Copy`, and borrows only the input bytes.
#[derive(Debug, Clone, Copy)]
pub struct SectionTable<'a> {
    file: &'a [u8],
    headers: Table<'a, Shdr>,
    names: StrTab<'a>,
}

impl<'a> SectionTable<'a> {
    /// Locates the table of the file `data` with the header `header`,
    /// resolving the escapes for large section counts, and the section
    /// name string table. Constant time: no section header other than
    /// header 0 and the string table's is read.
    pub fn parse(data: &'a [u8], header: &FileHeader) -> Result<Self, Error> {
        let class = header.class;
        let file_len = data.len() as u64;
        let offset = header.e_shoff;
        if offset == 0 {
            if header.e_shnum != 0 {
                return Err(Error::MissingSectionTable { e_shnum: header.e_shnum });
            }
            if header.e_shstrndx != 0 {
                return Err(Error::BadShstrndx { index: u32::from(header.e_shstrndx), count: 0 });
            }
            return Ok(SectionTable {
                file: data,
                headers: Table::empty(class),
                names: StrTab::empty(),
            });
        }
        let entsize = Shdr::size(class);
        if usize::from(header.e_shentsize) != entsize {
            return Err(Error::BadShentsize {
                e_shentsize: header.e_shentsize,
                expected: entsize as u16,
            });
        }
        let table = usize::try_from(offset).ok().and_then(|start| data.get(start..));
        let count = if header.e_shnum != 0 {
            u64::from(header.e_shnum)
        } else {
            // gABI "ELF Header": the count is in sh_size of header 0.
            match table.and_then(|bytes| Shdr::decode(class, bytes)) {
                Some(first) => first.sh_size,
                None => {
                    return Err(Error::SectionTableOutOfBounds { offset, count: 1, file_len });
                }
            }
        };
        if count > u64::from(u32::MAX) {
            return Err(Error::TooManySections { count });
        }
        let bytes = count
            .checked_mul(entsize as u64)
            .and_then(|size| usize::try_from(size).ok())
            .and_then(|size| table?.get(..size))
            .ok_or(Error::SectionTableOutOfBounds { offset, count, file_len })?;
        let mut sections =
            SectionTable { file: data, headers: Table::new(bytes, class), names: StrTab::empty() };
        let count = sections.len();
        let names = if header.e_shstrndx == SHN_XINDEX {
            // gABI "ELF Header": the index is in sh_link of header 0.
            match sections.headers.get(0) {
                Some(first) => first.sh_link,
                None => return Err(Error::BadShstrndx { index: u32::from(SHN_XINDEX), count }),
            }
        } else {
            u32::from(header.e_shstrndx)
        };
        if names != 0 {
            if names >= count {
                return Err(Error::BadShstrndx { index: names, count });
            }
            sections.names = sections.get(SectionIndex(names))?.strings()?;
        }
        Ok(sections)
    }

    /// The number of sections, section 0 included.
    pub fn len(&self) -> u32 {
        self.headers.len() as u32
    }

    pub fn is_empty(&self) -> bool {
        self.headers.is_empty()
    }

    pub fn class(&self) -> Class {
        self.headers.class()
    }

    /// The whole file.
    pub fn file(&self) -> &'a [u8] {
        self.file
    }

    /// The raw section headers.
    pub fn headers(&self) -> Table<'a, Shdr> {
        self.headers
    }

    /// The section name string table; empty when the file has none.
    pub fn names(&self) -> StrTab<'a> {
        self.names
    }

    fn view(&self, index: u32, shdr: Shdr) -> Section<'a> {
        Section {
            file: self.file,
            names: self.names,
            shdr,
            index,
            count: self.len(),
            class: self.class(),
        }
    }

    /// Section `index`.
    #[inline]
    pub fn get(&self, index: SectionIndex) -> Result<Section<'a>, Error> {
        match self.headers.get(index.0 as usize) {
            Some(shdr) => Ok(self.view(index.0, shdr)),
            None => Err(Error::SectionIndexOutOfRange { index: index.0, count: self.len() }),
        }
    }

    /// Every section in index order, section 0 included.
    pub fn iter(&self) -> SectionIter<'a> {
        SectionIter { headers: self.headers.iter(), table: *self, next: 0 }
    }

    /// The symbol table in section `symtab` (of type `SHT_SYMTAB` or
    /// `SHT_DYNSYM`), with the section indexes of `shndx`, its
    /// `SHT_SYMTAB_SHNDX` section, if it has one.
    pub fn symbol_table(
        &self,
        symtab: SectionIndex,
        shndx: Option<SectionIndex>,
    ) -> Result<SymbolTable<'a>, Error> {
        let section = self.get(symtab)?;
        if !matches!(section.sh_type(), SHT_SYMTAB | SHT_DYNSYM) {
            section.expect_type(SHT_SYMTAB)?;
        }
        let strings = self.get(section.link()?)?.strings()?;
        let symbols = section.records_of_class()?;
        if symbols.len() > u32::MAX as usize {
            return Err(Error::TooManySymbols { section: symtab.0, count: symbols.len() as u64 });
        }
        let count = symbols.len() as u32;
        let first_global = section.shdr.sh_info;
        if first_global > count {
            return Err(Error::BadFirstGlobal { section: symtab.0, sh_info: first_global, count });
        }
        let shndx = match shndx {
            None => None,
            Some(index) => {
                let section = self.get(index)?;
                section.expect_type(SHT_SYMTAB_SHNDX)?;
                let words: Table<'a, Word> = section.records_of_class()?;
                if words.len() != symbols.len() {
                    return Err(Error::ShndxCountMismatch {
                        section: index.0,
                        entries: words.len() as u64,
                        symbols: count,
                    });
                }
                Some(words)
            }
        };
        Ok(SymbolTable::new(symbols, strings, shndx, symtab.0, first_global, self.len()))
    }

    /// The one symbol table of type `sh_type` (`SHT_SYMTAB` or
    /// `SHT_DYNSYM`) with its `SHT_SYMTAB_SHNDX` section, or `None` when
    /// the file has no section of that type. Two passes over the section
    /// headers; more than one table of the type, or more than one
    /// `SHT_SYMTAB_SHNDX` section linked to it, is an error.
    pub fn find_symbol_table(&self, sh_type: u32) -> Result<Option<SymbolTable<'a>>, Error> {
        let mut symtab: Option<u32> = None;
        for (index, shdr) in (0u32..).zip(self.headers.iter()) {
            if shdr.sh_type == sh_type {
                if let Some(first) = symtab {
                    return Err(Error::MultipleSymbolTables { first, second: index });
                }
                symtab = Some(index);
            }
        }
        let Some(symtab) = symtab else { return Ok(None) };
        let mut shndx: Option<u32> = None;
        for (index, shdr) in (0u32..).zip(self.headers.iter()) {
            if shdr.sh_type == SHT_SYMTAB_SHNDX && shdr.sh_link == symtab {
                if let Some(first) = shndx {
                    return Err(Error::MultipleShndxTables { first, second: index });
                }
                shndx = Some(index);
            }
        }
        self.symbol_table(SectionIndex(symtab), shndx.map(SectionIndex)).map(Some)
    }
}

impl<'a> IntoIterator for SectionTable<'a> {
    type Item = Section<'a>;
    type IntoIter = SectionIter<'a>;

    fn into_iter(self) -> SectionIter<'a> {
        self.iter()
    }
}

impl<'a> IntoIterator for &SectionTable<'a> {
    type Item = Section<'a>;
    type IntoIter = SectionIter<'a>;

    fn into_iter(self) -> SectionIter<'a> {
        self.iter()
    }
}

/// The sections of a [`SectionTable`], in index order.
#[derive(Debug, Clone)]
pub struct SectionIter<'a> {
    headers: TableIter<'a, Shdr>,
    table: SectionTable<'a>,
    next: u32,
}

impl<'a> Iterator for SectionIter<'a> {
    type Item = Section<'a>;

    #[inline]
    fn next(&mut self) -> Option<Section<'a>> {
        let shdr = self.headers.next()?;
        let index = self.next;
        self.next += 1;
        Some(self.table.view(index, shdr))
    }

    fn size_hint(&self) -> (usize, Option<usize>) {
        self.headers.size_hint()
    }

    fn fold<B, F>(self, init: B, mut f: F) -> B
    where
        F: FnMut(B, Section<'a>) -> B,
    {
        let table = self.table;
        let mut index = self.next;
        self.headers.fold(init, |acc, shdr| {
            let section = table.view(index, shdr);
            index += 1;
            f(acc, section)
        })
    }
}

impl ExactSizeIterator for SectionIter<'_> {}
impl FusedIterator for SectionIter<'_> {}

/// One section: its header and, through the accessors, its contents read
/// as what its type says they are. `Copy`, and borrows only the input
/// bytes. Every accessor checks what it reads; none allocates.
#[derive(Debug, Clone, Copy)]
pub struct Section<'a> {
    file: &'a [u8],
    names: StrTab<'a>,
    shdr: Shdr,
    index: u32,
    count: u32,
    class: Class,
}

impl<'a> Section<'a> {
    /// The section's index.
    pub fn index(&self) -> SectionIndex {
        SectionIndex(self.index)
    }

    /// The section header, every field as stored.
    pub fn header(&self) -> &Shdr {
        &self.shdr
    }

    pub fn class(&self) -> Class {
        self.class
    }

    /// `sh_type`.
    pub fn sh_type(&self) -> u32 {
        self.shdr.sh_type
    }

    /// `sh_flags`.
    pub fn flags(&self) -> u64 {
        self.shdr.sh_flags
    }

    /// `sh_size`: the size of the contents in the file, or for
    /// `SHT_NOBITS` in memory.
    pub fn size(&self) -> u64 {
        self.shdr.sh_size
    }

    /// The section's name, from the section name string table.
    pub fn name(&self) -> Result<&'a [u8], Error> {
        self.names.get(self.shdr.sh_name)
    }

    /// The alignment: `sh_addralign`, with 0 read as 1. A value that is
    /// not a power of two is an error.
    pub fn alignment(&self) -> Result<u64, Error> {
        match self.shdr.sh_addralign {
            0 => Ok(1),
            align if align.is_power_of_two() => Ok(align),
            align => Err(Error::BadAlignment { section: self.index, align }),
        }
    }

    /// Where the contents are in the file: `sh_offset..sh_offset +
    /// sh_size`, or `None` for a section without contents in the file
    /// (`SHT_NOBITS`, `SHT_NULL`).
    pub fn file_range(&self) -> Result<Option<Range<usize>>, Error> {
        if matches!(self.shdr.sh_type, SHT_NOBITS | SHT_NULL) {
            return Ok(None);
        }
        let Shdr { sh_offset: offset, sh_size: size, .. } = self.shdr;
        let range = usize::try_from(offset).ok().zip(
            offset
                .checked_add(size)
                .filter(|end| *end <= self.file.len() as u64)
                .and_then(|end| usize::try_from(end).ok()),
        );
        match range {
            Some((start, end)) => Ok(Some(start..end)),
            None => Err(Error::SectionOutOfBounds {
                section: self.index,
                offset,
                size,
                file_len: self.file.len() as u64,
            }),
        }
    }

    /// The contents as stored in the file; empty for a section without
    /// contents in the file. A `SHF_COMPRESSED` section's contents start
    /// with its compression header ([`Section::compression`]).
    pub fn data(&self) -> Result<&'a [u8], Error> {
        match self.file_range()? {
            Some(range) => Ok(self.file.get(range).unwrap_or(&[])),
            None => Ok(&[]),
        }
    }

    /// `sh_link` as a section index, checked to be a section.
    pub fn link(&self) -> Result<SectionIndex, Error> {
        let link = self.shdr.sh_link;
        if link < self.count {
            Ok(SectionIndex(link))
        } else {
            Err(Error::BadLink { section: self.index, link, count: self.count })
        }
    }

    /// `sh_info` as a section index, checked to be a section: the section
    /// a relocation section applies to.
    pub fn info_section(&self) -> Result<SectionIndex, Error> {
        let info = self.shdr.sh_info;
        if info < self.count {
            Ok(SectionIndex(info))
        } else {
            Err(Error::BadInfo { section: self.index, info, count: self.count })
        }
    }

    fn expect_type(&self, expected: u32) -> Result<(), Error> {
        if self.shdr.sh_type == expected {
            Ok(())
        } else {
            Err(Error::WrongSectionType {
                section: self.index,
                sh_type: self.shdr.sh_type,
                expected,
            })
        }
    }

    /// The contents as a table of `T`: `sh_entsize` must be the size of
    /// `T` in the file's class and `sh_size` a multiple of it.
    pub(crate) fn records_of_class<T: Record>(&self) -> Result<Table<'a, T>, Error> {
        let expected = T::size(self.class) as u64;
        let Shdr { sh_entsize: entsize, sh_size: size, .. } = self.shdr;
        if entsize != expected {
            return Err(Error::BadEntsize { section: self.index, entsize, expected });
        }
        if size % expected != 0 {
            return Err(Error::BadSize { section: self.index, size, entsize });
        }
        Ok(Table::new(self.data()?, self.class))
    }

    /// The contents of a `SHT_STRTAB` section.
    pub fn strings(&self) -> Result<StrTab<'a>, Error> {
        self.expect_type(SHT_STRTAB)?;
        Ok(StrTab::new(self.data()?, self.index))
    }

    fn relocations<R: Record>(&self, sh_type: u32) -> Result<RelocSection<'a, R>, Error> {
        self.expect_type(sh_type)?;
        Ok(RelocSection {
            symtab: self.link()?,
            target: self.info_section()?,
            records: self.records_of_class()?,
            section: self.index,
        })
    }

    /// The records of a `SHT_RELA` section.
    pub fn rela(&self) -> Result<RelaSection<'a>, Error> {
        self.relocations(SHT_RELA)
    }

    /// The records of a `SHT_REL` section.
    pub fn rel(&self) -> Result<RelSection<'a>, Error> {
        self.relocations(SHT_REL)
    }

    /// The flag word and the members of a `SHT_GROUP` section.
    pub fn group(&self) -> Result<Group<'a>, Error> {
        self.expect_type(SHT_GROUP)?;
        let symtab = self.link()?;
        let words: Table<'a, Word> = self.records_of_class()?;
        let Some(Word(flags)) = words.get(0) else {
            return Err(Error::EmptyGroup { section: self.index });
        };
        let members = words.slice(1, words.len()).unwrap_or(Table::empty(self.class));
        Ok(Group {
            members,
            flags,
            symtab,
            signature: SymbolIndex(self.shdr.sh_info),
            section: self.index,
            count: self.count,
        })
    }

    /// The notes of a `SHT_NOTE` section. Names and descriptors are padded
    /// to 8 bytes when `sh_addralign` is 8 and to 4 bytes when it is at
    /// most 4; any other alignment is an error.
    pub fn notes(&self) -> Result<Notes<'a>, Error> {
        self.expect_type(SHT_NOTE)?;
        let align = match self.shdr.sh_addralign {
            0..=4 => 4,
            8 => 8,
            align => return Err(Error::NoteAlignment { section: self.index, align }),
        };
        Ok(Notes::new(self.data()?, align, self.class, self.index))
    }

    /// The compression header of a `SHF_COMPRESSED` section and the
    /// compressed bytes after it, or `None` for a section without the
    /// flag. Nothing is decompressed.
    pub fn compression(&self) -> Result<Option<Compression<'a>>, Error> {
        if self.shdr.sh_flags & SHF_COMPRESSED == 0 {
            return Ok(None);
        }
        if self.shdr.sh_type == SHT_NOBITS {
            return Err(Error::CompressedNobits { section: self.index });
        }
        let data = self.data()?;
        let size = Chdr::size(self.class);
        match (Chdr::decode(self.class, data), data.get(size..)) {
            (Some(header), Some(payload)) => Ok(Some(Compression { header, payload })),
            _ => Err(Error::CompressedTooShort {
                section: self.index,
                size: data.len() as u64,
                expected: size as u64,
            }),
        }
    }

    /// The entry size of a `SHF_MERGE` section and whether its entries
    /// are strings, or `None` for a section without the flag. An entry
    /// size of 0 is reported as it is; a size that is not a multiple of a
    /// non-zero entry size is an error.
    pub fn merge(&self) -> Result<Option<Merge>, Error> {
        if self.shdr.sh_flags & SHF_MERGE == 0 {
            return Ok(None);
        }
        let Shdr { sh_entsize: entsize, sh_size: size, .. } = self.shdr;
        if entsize != 0 && size % entsize != 0 {
            return Err(Error::BadSize { section: self.index, size, entsize });
        }
        Ok(Some(Merge { entsize, strings: self.shdr.sh_flags & SHF_STRINGS != 0 }))
    }
}

/// What [`Section::merge`] reports of a `SHF_MERGE` section.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Merge {
    /// `sh_entsize`: the size of a constant, or of a character when
    /// `strings`.
    pub entsize: u64,
    /// `SHF_STRINGS`: the entries are NUL-terminated strings of
    /// `entsize`-byte characters.
    pub strings: bool,
}

/// What [`Section::compression`] reports of a `SHF_COMPRESSED` section.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Compression<'a> {
    /// The compression header.
    pub header: Chdr,
    /// The compressed bytes: the section's contents after the header.
    pub payload: &'a [u8],
}

/// The records of a relocation section ([`Section::rela`],
/// [`Section::rel`]). `Copy`, and borrows only the input bytes.
#[derive(Debug, Clone, Copy)]
pub struct RelocSection<'a, R> {
    records: Table<'a, R>,
    section: u32,
    symtab: SectionIndex,
    target: SectionIndex,
}

/// A `SHT_RELA` section.
pub type RelaSection<'a> = RelocSection<'a, Rela>;
/// A `SHT_REL` section.
pub type RelSection<'a> = RelocSection<'a, Rel>;

impl<'a, R: Record> RelocSection<'a, R> {
    /// The relocation section's own index.
    pub fn section(&self) -> SectionIndex {
        SectionIndex(self.section)
    }

    /// `sh_link`: the symbol table the records' symbol indexes refer to.
    pub fn symtab(&self) -> SectionIndex {
        self.symtab
    }

    /// `sh_info`: the section the records' offsets are in.
    pub fn target(&self) -> SectionIndex {
        self.target
    }

    /// The records as a table (indexable, sliceable).
    pub fn records(&self) -> Table<'a, R> {
        self.records
    }

    pub fn len(&self) -> usize {
        self.records.len()
    }

    pub fn is_empty(&self) -> bool {
        self.records.is_empty()
    }

    /// Record `index`, or `None` past the end.
    pub fn get(&self, index: usize) -> Option<R> {
        self.records.get(index)
    }

    /// The records in file order.
    pub fn iter(&self) -> TableIter<'a, R> {
        self.records.iter()
    }
}

impl<'a, R: Record> IntoIterator for RelocSection<'a, R> {
    type Item = R;
    type IntoIter = TableIter<'a, R>;

    fn into_iter(self) -> TableIter<'a, R> {
        self.records.iter()
    }
}

impl<'a, R: Record> IntoIterator for &RelocSection<'a, R> {
    type Item = R;
    type IntoIter = TableIter<'a, R>;

    fn into_iter(self) -> TableIter<'a, R> {
        self.records.iter()
    }
}

/// A section group ([`Section::group`]; gABI "Section Groups"). `Copy`,
/// and borrows only the input bytes.
#[derive(Debug, Clone, Copy)]
pub struct Group<'a> {
    members: Table<'a, Word>,
    flags: u32,
    symtab: SectionIndex,
    signature: SymbolIndex,
    section: u32,
    count: u32,
}

impl<'a> Group<'a> {
    /// The group section's own index.
    pub fn section(&self) -> SectionIndex {
        SectionIndex(self.section)
    }

    /// The flag word (`GRP_*`).
    pub fn flags(&self) -> u32 {
        self.flags
    }

    /// Whether `GRP_COMDAT` is set.
    pub fn is_comdat(&self) -> bool {
        self.flags & GRP_COMDAT != 0
    }

    /// `sh_link`: the symbol table that holds the signature.
    pub fn symtab(&self) -> SectionIndex {
        self.symtab
    }

    /// `sh_info`: the signature symbol's index in [`Group::symtab`]. Not
    /// checked against the symbol table: looking it up is.
    pub fn signature(&self) -> SymbolIndex {
        self.signature
    }

    /// The number of members.
    pub fn len(&self) -> usize {
        self.members.len()
    }

    pub fn is_empty(&self) -> bool {
        self.members.is_empty()
    }

    /// The member section indexes in file order. A member that is not a
    /// section, or is section 0, is an error item; iteration continues
    /// after it.
    pub fn members(&self) -> GroupMembers<'a> {
        GroupMembers {
            words: self.members.iter(),
            position: 0,
            section: self.section,
            count: self.count,
        }
    }
}

/// The members of a [`Group`].
#[derive(Debug, Clone)]
pub struct GroupMembers<'a> {
    words: TableIter<'a, Word>,
    position: u32,
    section: u32,
    count: u32,
}

impl Iterator for GroupMembers<'_> {
    type Item = Result<SectionIndex, Error>;

    fn next(&mut self) -> Option<Self::Item> {
        let Word(index) = self.words.next()?;
        let member = self.position;
        self.position = self.position.wrapping_add(1);
        Some(if index != 0 && index < self.count {
            Ok(SectionIndex(index))
        } else {
            Err(Error::GroupMemberOutOfRange {
                section: self.section,
                member,
                index,
                count: self.count,
            })
        })
    }

    fn size_hint(&self) -> (usize, Option<usize>) {
        self.words.size_hint()
    }
}

impl ExactSizeIterator for GroupMembers<'_> {}
impl FusedIterator for GroupMembers<'_> {}
