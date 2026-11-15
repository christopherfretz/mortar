//! Shared objects as a linker reads them (`spec/elf.md` §6): the program
//! headers, the dynamic table, the dynamic symbols, their versions and the
//! program properties of an `ET_DYN` file, found through the section
//! headers or, when the file has none, through the dynamic table alone.

use crate::consts::{
    DT_GNU_HASH, DT_HASH, DT_STRSZ, DT_STRTAB, DT_SYMENT, DT_SYMTAB, DT_SYMTAB_SHNDX, DT_VERDEF,
    DT_VERDEFNUM, DT_VERNEED, DT_VERNEEDNUM, DT_VERSYM, ELF_NOTE_GNU, ET_DYN,
    NT_GNU_PROPERTY_TYPE_0, PT_DYNAMIC, PT_GNU_PROPERTY, SHT_DYNAMIC, SHT_DYNSYM, SHT_GNU_VERDEF,
    SHT_GNU_VERNEED, SHT_GNU_VERSYM, STB_LOCAL,
};
use crate::dynamic::DynamicTable;
use crate::error::Error;
use crate::hash;
use crate::header::{FileHeader, Machine};
use crate::note::Note;
use crate::raw::{Class, Dyn, Record, Sym, Table, Versym, Word};
use crate::section::{Section, SectionTable};
use crate::segment::ProgramHeaders;
use crate::strtab::{DYNAMIC_STRINGS, StrTab};
use crate::symbol::SymbolTable;
use crate::version::{Chain, VersionDefinitions, VersionNeeds, Versions};

/// How the parts of a shared object are found (§6.3).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Entry {
    /// Through the section headers: `SHT_DYNAMIC`, `SHT_DYNSYM`,
    /// `SHT_GNU_versym`, `SHT_GNU_verdef`, `SHT_GNU_verneed`.
    Sections,
    /// Through the dynamic table alone: `PT_DYNAMIC`, and the `DT_*`
    /// addresses mapped to file offsets by the `PT_LOAD` segments.
    Dynamic,
}

/// A shared object (`ET_DYN`) for one of the machines mortar links.
///
/// A view of the file's bytes: `Copy`, borrows only the input, owns
/// nothing. [`SharedObject::parse`] locates the parts; their contents are
/// decoded and checked when they are asked for, and an error in one part
/// is the error of that part's accessor only.
#[derive(Debug, Clone, Copy)]
pub struct SharedObject<'a> {
    header: FileHeader,
    machine: Machine,
    entry: Entry,
    segments: ProgramHeaders<'a>,
    sections: SectionTable<'a>,
    dynamic: Result<Option<DynamicTable<'a>>, Error>,
    symbols: Result<Option<SymbolTable<'a>>, Error>,
    versym: Result<Option<Table<'a, Versym>>, Error>,
    definitions: Result<Option<Chain<'a>>, Error>,
    needs: Result<Option<Chain<'a>>, Error>,
}

/// The parts, before they are put in a [`SharedObject`].
struct Parts<'a> {
    dynamic: Result<Option<DynamicTable<'a>>, Error>,
    symbols: Result<Option<SymbolTable<'a>>, Error>,
    versym: Result<Option<Table<'a, Versym>>, Error>,
    definitions: Result<Option<Chain<'a>>, Error>,
    needs: Result<Option<Chain<'a>>, Error>,
}

impl Parts<'_> {
    fn failed(error: Error) -> Self {
        Parts {
            dynamic: Err(error),
            symbols: Err(error),
            versym: Err(error),
            definitions: Err(error),
            needs: Err(error),
        }
    }

    fn none() -> Self {
        Parts {
            dynamic: Ok(None),
            symbols: Ok(None),
            versym: Ok(None),
            definitions: Ok(None),
            needs: Ok(None),
        }
    }
}

impl<'a> SharedObject<'a> {
    /// Reads the file header, locates the program header table and the
    /// section header table, and then the parts: through the section
    /// headers when the file has a section, through the dynamic table when
    /// it has none (§6.3).
    pub fn parse(data: &'a [u8]) -> Result<SharedObject<'a>, Error> {
        Self::parse_with(data, None)
    }

    /// [`SharedObject::parse`], with the parts found the way `entry` says
    /// whether or not the file has section headers. Through
    /// [`Entry::Sections`], a file without sections has no parts.
    pub fn parse_by(data: &'a [u8], entry: Entry) -> Result<SharedObject<'a>, Error> {
        Self::parse_with(data, Some(entry))
    }

    fn parse_with(data: &'a [u8], entry: Option<Entry>) -> Result<SharedObject<'a>, Error> {
        let header = FileHeader::parse(data)?;
        let machine = Machine::of(&header)?;
        if header.e_type != ET_DYN {
            return Err(Error::NotSharedObject { e_type: header.e_type });
        }
        let sections = SectionTable::parse(data, &header)?;
        let segments = ProgramHeaders::parse(data, &header, &sections)?;
        let entry =
            entry.unwrap_or(if sections.is_empty() { Entry::Dynamic } else { Entry::Sections });
        let parts = match entry {
            Entry::Sections => by_sections(&sections),
            Entry::Dynamic => by_dynamic(&segments, header.class),
        };
        Ok(SharedObject {
            header,
            machine,
            entry,
            segments,
            sections,
            dynamic: parts.dynamic,
            symbols: parts.symbols,
            versym: parts.versym,
            definitions: parts.definitions,
            needs: parts.needs,
        })
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

    /// How the parts were found.
    pub fn entry(&self) -> Entry {
        self.entry
    }

    /// The program header table.
    pub fn program_headers(&self) -> ProgramHeaders<'a> {
        self.segments
    }

    /// The section header table; empty for a file without one.
    pub fn section_table(&self) -> SectionTable<'a> {
        self.sections
    }

    /// The dynamic table, or `None` when the object has none.
    pub fn dynamic(&self) -> Result<Option<DynamicTable<'a>>, Error> {
        self.dynamic
    }

    /// The dynamic symbol table, or `None` when the object has none.
    pub fn symbols(&self) -> Result<Option<SymbolTable<'a>>, Error> {
        self.symbols
    }

    /// The version table, one entry per dynamic symbol, or `None` when the
    /// object has none.
    pub fn versym(&self) -> Result<Option<Table<'a, Versym>>, Error> {
        self.versym
    }

    /// The version definitions, or `None` when the object has none.
    pub fn version_definitions(&self) -> Result<Option<VersionDefinitions<'a>>, Error> {
        self.definitions?.map(VersionDefinitions::new).transpose()
    }

    /// The version needs, or `None` when the object has none.
    pub fn version_needs(&self) -> Result<Option<VersionNeeds<'a>>, Error> {
        self.needs?.map(VersionNeeds::new).transpose()
    }

    /// The three version parts together, to ask which version a symbol
    /// carries ([`Versions::of`]).
    pub fn versions(&self) -> Result<Versions<'a>, Error> {
        Ok(Versions { versym: self.versym?, definitions: self.definitions?, needs: self.needs? })
    }

    /// The program property note: the `GNU` note of type
    /// `NT_GNU_PROPERTY_TYPE_0` in the `PT_GNU_PROPERTY` segment, or `None`
    /// when there is no such segment or it holds no such note.
    pub fn gnu_property_note(&self) -> Result<Option<Note<'a>>, Error> {
        let Some(segment) = self.segments.unique(PT_GNU_PROPERTY)? else { return Ok(None) };
        for note in segment.notes()? {
            let note = note?;
            if note.n_type == NT_GNU_PROPERTY_TYPE_0 && note.name() == ELF_NOTE_GNU {
                return Ok(Some(note));
            }
        }
        Ok(None)
    }
}

/// The one section of type `sh_type`, or `None`.
fn unique_section<'a>(
    sections: &SectionTable<'a>,
    sh_type: u32,
) -> Result<Option<Section<'a>>, Error> {
    let mut found: Option<Section<'a>> = None;
    for section in sections {
        if section.sh_type() == sh_type {
            if let Some(first) = found {
                return Err(Error::MultipleSections {
                    sh_type,
                    first: first.index().0,
                    second: section.index().0,
                });
            }
            found = Some(section);
        }
    }
    Ok(found)
}

/// A version chain in a section: its bytes, `sh_info` records, names in
/// the `sh_link` string table.
fn section_chain<'a>(
    sections: &SectionTable<'a>,
    sh_type: u32,
) -> Result<Option<Chain<'a>>, Error> {
    let Some(section) = unique_section(sections, sh_type)? else { return Ok(None) };
    let strings = sections.get(section.link()?)?.strings()?;
    let bytes = section.data()?;
    Ok(Some(Chain {
        bytes,
        base: section.header().sh_offset,
        count: u64::from(section.header().sh_info),
        strings,
    }))
}

/// §6.3: the parts through the section headers.
fn by_sections<'a>(sections: &SectionTable<'a>) -> Parts<'a> {
    let dynamic = unique_section(sections, SHT_DYNAMIC).and_then(|found| {
        let Some(section) = found else { return Ok(None) };
        let records = section.records_of_class::<Dyn>()?;
        let strings = section
            .link()
            .and_then(|link| sections.get(link))
            .and_then(|link| link.strings())
            .map(Some);
        DynamicTable::new(records, strings, section.header().sh_offset).map(Some)
    });
    let symbols = sections.find_symbol_table(SHT_DYNSYM);
    let versym = unique_section(sections, SHT_GNU_VERSYM).and_then(|found| {
        let Some(section) = found else { return Ok(None) };
        let table: Table<'a, Versym> = section.records_of_class()?;
        let dynsym = symbols?;
        let link = section.header().sh_link;
        let Some(dynsym) = dynsym.filter(|dynsym| dynsym.section().0 == link) else {
            return Err(Error::VersymLink { section: section.index().0, link });
        };
        if table.len() != dynsym.len() as usize {
            return Err(Error::VersymCountMismatch {
                entries: table.len() as u64,
                symbols: dynsym.len(),
            });
        }
        Ok(Some(table))
    });
    Parts {
        dynamic,
        symbols,
        versym,
        definitions: section_chain(sections, SHT_GNU_VERDEF),
        needs: section_chain(sections, SHT_GNU_VERNEED),
    }
}

/// §6.3: the parts through the dynamic table alone.
fn by_dynamic<'a>(segments: &ProgramHeaders<'a>, class: Class) -> Parts<'a> {
    let table = match locate_dynamic(segments, class) {
        Ok(Some(table)) => table,
        Ok(None) => return Parts::none(),
        Err(error) => return Parts::failed(error),
    };
    let strings = table.strings();
    let symbols = dynamic_symbols(segments, class, &table);
    let versym = symbols.and_then(|symbols| {
        let Some(address) = table.value(DT_VERSYM)? else { return Ok(None) };
        let Some(symbols) = symbols else {
            return Err(Error::MissingDynamicTag { d_tag: DT_SYMTAB, needed_by: DT_VERSYM });
        };
        let bytes = mapped(segments, DT_VERSYM, address, u64::from(symbols.len()), 2)?;
        Ok(Some(Table::new(bytes, class)))
    });
    let chain = |d_tag: i64, count_tag: i64| -> Result<Option<Chain<'a>>, Error> {
        let Some(address) = table.value(d_tag)? else { return Ok(None) };
        let Some(count) = table.value(count_tag)? else {
            return Err(Error::MissingDynamicTag { d_tag: count_tag, needed_by: d_tag });
        };
        let Some(strings) = strings? else {
            return Err(Error::MissingDynamicTag { d_tag: DT_STRTAB, needed_by: d_tag });
        };
        let Some((base, bytes)) = segments.file_bytes_from(address) else {
            return Err(Error::AddressNotMapped { d_tag, addr: address, size: 1 });
        };
        Ok(Some(Chain { bytes, base, count, strings }))
    };
    Parts {
        dynamic: Ok(Some(table)),
        symbols,
        versym,
        definitions: chain(DT_VERDEF, DT_VERDEFNUM),
        needs: chain(DT_VERNEED, DT_VERNEEDNUM),
    }
}

/// The bytes of `count` records of `size` bytes at `address`, which the
/// entry `d_tag` gives.
fn mapped<'a>(
    segments: &ProgramHeaders<'a>,
    d_tag: i64,
    address: u64,
    count: u64,
    size: u64,
) -> Result<&'a [u8], Error> {
    let bytes = count.checked_mul(size);
    bytes
        .and_then(|bytes| segments.file_range(address, bytes))
        .and_then(|range| segments.file().get(range))
        .ok_or(Error::AddressNotMapped { d_tag, addr: address, size: bytes.unwrap_or(u64::MAX) })
}

/// The dynamic table of `PT_DYNAMIC` and the string table of `DT_STRTAB`.
fn locate_dynamic<'a>(
    segments: &ProgramHeaders<'a>,
    class: Class,
) -> Result<Option<DynamicTable<'a>>, Error> {
    let Some(segment) = segments.unique(PT_DYNAMIC)? else { return Ok(None) };
    let records: Table<'a, Dyn> = Table::new(segment.data()?, class);
    let offset = segment.header().p_offset;
    // The table without its string table first, to read DT_STRTAB from it.
    let bare = DynamicTable::new(records, Ok(None), offset)?;
    let strings = (|| {
        let Some(address) = bare.value(DT_STRTAB)? else { return Ok(None) };
        let Some(size) = bare.value(DT_STRSZ)? else {
            return Err(Error::MissingDynamicTag { d_tag: DT_STRSZ, needed_by: DT_STRTAB });
        };
        let bytes = mapped(segments, DT_STRTAB, address, size, 1)?;
        Ok(Some(StrTab::new(bytes, DYNAMIC_STRINGS)))
    })();
    DynamicTable::new(records, strings, offset).map(Some)
}

/// §6.5, §6.6: the dynamic symbol table of `DT_SYMTAB`, as many symbols as
/// the hash tables count.
fn dynamic_symbols<'a>(
    segments: &ProgramHeaders<'a>,
    class: Class,
    table: &DynamicTable<'a>,
) -> Result<Option<SymbolTable<'a>>, Error> {
    let Some(address) = table.value(DT_SYMTAB)? else { return Ok(None) };
    let size = Sym::size(class) as u64;
    if let Some(value) = table.value(DT_SYMENT)?
        && value != size
    {
        return Err(Error::BadDynamicEntsize { d_tag: DT_SYMENT, value, expected: size });
    }
    let count = symbol_count(segments, class, table)?;
    let Some(strings) = table.strings()? else {
        return Err(Error::MissingDynamicTag { d_tag: DT_STRTAB, needed_by: DT_SYMTAB });
    };
    let symbols: Table<'a, Sym> =
        Table::new(mapped(segments, DT_SYMTAB, address, u64::from(count), size)?, class);
    let shndx = match table.value(DT_SYMTAB_SHNDX)? {
        Some(address) => Some(Table::<Word>::new(
            mapped(segments, DT_SYMTAB_SHNDX, address, u64::from(count), 4)?,
            class,
        )),
        None => None,
    };
    // Without sh_info, the first global is the first symbol that is not
    // STB_LOCAL.
    let first_global =
        symbols.iter().position(|sym| sym.binding() != STB_LOCAL).unwrap_or(symbols.len()) as u32;
    Ok(Some(SymbolTable::new(symbols, strings, shndx, 0, first_global, None)))
}

/// §6.6: the number of dynamic symbols, from `DT_GNU_HASH` and `DT_HASH`;
/// when both are present they must agree.
fn symbol_count(
    segments: &ProgramHeaders<'_>,
    class: Class,
    table: &DynamicTable<'_>,
) -> Result<u32, Error> {
    let count = |d_tag: i64| -> Result<Option<u32>, Error> {
        let Some(address) = table.value(d_tag)? else { return Ok(None) };
        let bad = Error::BadHashTable { d_tag, addr: address };
        let (_, bytes) = segments.file_bytes_from(address).ok_or(bad)?;
        let count =
            if d_tag == DT_HASH { hash::sysv_count(bytes) } else { hash::gnu_count(bytes, class) };
        count.ok_or(bad).map(Some)
    };
    match (count(DT_HASH)?, count(DT_GNU_HASH)?) {
        (Some(hash), Some(gnu_hash)) if hash != gnu_hash => {
            Err(Error::SymbolCountMismatch { hash: u64::from(hash), gnu_hash: u64::from(gnu_hash) })
        }
        (Some(count), _) | (None, Some(count)) => Ok(count),
        (None, None) => Err(Error::NoSymbolCount),
    }
}
