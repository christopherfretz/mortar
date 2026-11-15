//! Program headers (`spec/elf.md` §6.2): the segments of a file, and the
//! map from addresses to file offsets that the dynamic table's addresses
//! are read through.

use core::iter::FusedIterator;
use core::ops::Range;

use crate::consts::{PF_R, PF_W, PF_X, PN_XNUM, PT_GNU_PROPERTY, PT_LOAD, PT_NOTE};
use crate::error::Error;
use crate::header::FileHeader;
use crate::note::Notes;
use crate::raw::{Class, Phdr, Record, Table, TableIter};
use crate::section::SectionTable;

/// The program header table of a file. `Copy`, and borrows only the input
/// bytes.
#[derive(Debug, Clone, Copy)]
pub struct ProgramHeaders<'a> {
    file: &'a [u8],
    headers: Table<'a, Phdr>,
}

impl<'a> ProgramHeaders<'a> {
    /// Locates the program header table of the file `data` with the header
    /// `header`. `sections` is the file's section header table, which holds
    /// the count when `e_phnum` is `PN_XNUM`. Constant time: no program
    /// header is read.
    pub fn parse(
        data: &'a [u8],
        header: &FileHeader,
        sections: &SectionTable<'a>,
    ) -> Result<Self, Error> {
        let class = header.class;
        let empty = ProgramHeaders { file: data, headers: Table::empty(class) };
        let offset = header.e_phoff;
        if offset == 0 {
            if header.e_phnum != 0 {
                return Err(Error::MissingProgramHeaderTable { e_phnum: header.e_phnum });
            }
            return Ok(empty);
        }
        let count = if header.e_phnum == PN_XNUM {
            // gABI "Program Header": the count is in sh_info of section 0.
            match sections.headers().get(0) {
                Some(first) => u64::from(first.sh_info),
                None => return Err(Error::PhnumEscapeWithoutSections),
            }
        } else {
            u64::from(header.e_phnum)
        };
        if count == 0 {
            return Ok(empty);
        }
        let entsize = Phdr::size(class);
        if usize::from(header.e_phentsize) != entsize {
            return Err(Error::BadPhentsize {
                e_phentsize: header.e_phentsize,
                expected: entsize as u16,
            });
        }
        let file_len = data.len() as u64;
        let bytes = count
            .checked_mul(entsize as u64)
            .and_then(|size| offset.checked_add(size))
            .filter(|end| *end <= file_len)
            .and_then(|end| data.get(usize::try_from(offset).ok()?..usize::try_from(end).ok()?))
            .ok_or(Error::ProgramHeadersOutOfBounds { offset, count, file_len })?;
        Ok(ProgramHeaders { file: data, headers: Table::new(bytes, class) })
    }

    /// The number of program headers.
    pub fn len(&self) -> u32 {
        self.headers.len() as u32
    }

    pub fn is_empty(&self) -> bool {
        self.headers.is_empty()
    }

    pub fn class(&self) -> Class {
        self.headers.class()
    }

    /// The whole file the table is in.
    pub fn file(&self) -> &'a [u8] {
        self.file
    }

    /// The raw program headers.
    pub fn headers(&self) -> Table<'a, Phdr> {
        self.headers
    }

    fn view(&self, index: u32, phdr: Phdr) -> Segment<'a> {
        Segment { file: self.file, phdr, index, class: self.class() }
    }

    /// Program header `index`.
    pub fn get(&self, index: u32) -> Result<Segment<'a>, Error> {
        match self.headers.get(index as usize) {
            Some(phdr) => Ok(self.view(index, phdr)),
            None => Err(Error::SegmentIndexOutOfRange { index, count: self.len() }),
        }
    }

    /// Every program header in table order.
    pub fn iter(&self) -> SegmentIter<'a> {
        SegmentIter { headers: self.headers.iter(), table: *self, next: 0 }
    }

    /// The one segment of type `p_type`, or `None` when there is none. For
    /// the types a file has at most once (`PT_DYNAMIC`, `PT_TLS`,
    /// `PT_GNU_PROPERTY`, ...): a second is an error.
    pub fn unique(&self, p_type: u32) -> Result<Option<Segment<'a>>, Error> {
        let mut found: Option<Segment<'a>> = None;
        for segment in self {
            if segment.p_type() == p_type {
                if let Some(first) = found {
                    return Err(Error::MultipleSegments {
                        p_type,
                        first: first.index,
                        second: segment.index,
                    });
                }
                found = Some(segment);
            }
        }
        Ok(found)
    }

    /// The first `PT_LOAD` segment whose memory image, `p_vaddr` to
    /// `p_vaddr + p_memsz`, holds `addr`: the segment a symbol of that
    /// value is in.
    pub fn load_containing(&self, addr: u64) -> Option<Segment<'a>> {
        self.iter().find(|segment| segment.p_type() == PT_LOAD && segment.contains(addr))
    }

    /// The file bytes at addresses `addr` to `addr + size`: the first
    /// `PT_LOAD` segment whose file contents, `p_vaddr` to `p_vaddr +
    /// p_filesz`, hold the whole range, and the range's offsets in the
    /// file. `None` when no segment does, or the bytes are not in the file.
    pub fn file_range(&self, addr: u64, size: u64) -> Option<Range<usize>> {
        let end = addr.checked_add(size)?;
        self.iter().find_map(|segment| {
            let Phdr { p_type, p_vaddr, p_filesz, p_offset, .. } = segment.phdr;
            if p_type != PT_LOAD || addr < p_vaddr || end > p_vaddr.checked_add(p_filesz)? {
                return None;
            }
            let start = p_offset.checked_add(addr - p_vaddr)?;
            let stop = start.checked_add(size).filter(|stop| *stop <= self.file.len() as u64)?;
            Some(usize::try_from(start).ok()?..usize::try_from(stop).ok()?)
        })
    }

    /// The file bytes from address `addr` to the end of the file contents
    /// of the first `PT_LOAD` segment that holds it, and the offset of the
    /// first byte: for a table whose size the file does not state (version
    /// definitions and needs, hash chains).
    pub fn file_bytes_from(&self, addr: u64) -> Option<(u64, &'a [u8])> {
        self.iter().find_map(|segment| {
            let Phdr { p_type, p_vaddr, p_filesz, p_offset, .. } = segment.phdr;
            if p_type != PT_LOAD || addr < p_vaddr || addr >= p_vaddr.checked_add(p_filesz)? {
                return None;
            }
            let start = p_offset.checked_add(addr - p_vaddr)?;
            let stop = p_offset.checked_add(p_filesz)?.min(self.file.len() as u64);
            let bytes = self.file.get(usize::try_from(start).ok()?..usize::try_from(stop).ok()?)?;
            Some((start, bytes))
        })
    }
}

impl<'a> IntoIterator for ProgramHeaders<'a> {
    type Item = Segment<'a>;
    type IntoIter = SegmentIter<'a>;

    fn into_iter(self) -> SegmentIter<'a> {
        self.iter()
    }
}

impl<'a> IntoIterator for &ProgramHeaders<'a> {
    type Item = Segment<'a>;
    type IntoIter = SegmentIter<'a>;

    fn into_iter(self) -> SegmentIter<'a> {
        self.iter()
    }
}

/// The segments of a [`ProgramHeaders`] table, in order.
#[derive(Debug, Clone)]
pub struct SegmentIter<'a> {
    headers: TableIter<'a, Phdr>,
    table: ProgramHeaders<'a>,
    next: u32,
}

impl<'a> Iterator for SegmentIter<'a> {
    type Item = Segment<'a>;

    fn next(&mut self) -> Option<Segment<'a>> {
        let phdr = self.headers.next()?;
        let index = self.next;
        self.next += 1;
        Some(self.table.view(index, phdr))
    }

    fn size_hint(&self) -> (usize, Option<usize>) {
        self.headers.size_hint()
    }
}

impl ExactSizeIterator for SegmentIter<'_> {}
impl FusedIterator for SegmentIter<'_> {}

/// One program header and, through the accessors, the segment's contents.
/// `Copy`, and borrows only the input bytes.
#[derive(Debug, Clone, Copy)]
pub struct Segment<'a> {
    file: &'a [u8],
    phdr: Phdr,
    index: u32,
    class: Class,
}

impl<'a> Segment<'a> {
    /// The program header's index in the table.
    pub fn index(&self) -> u32 {
        self.index
    }

    /// The program header, every field as stored.
    pub fn header(&self) -> &Phdr {
        &self.phdr
    }

    /// `p_type`.
    pub fn p_type(&self) -> u32 {
        self.phdr.p_type
    }

    /// `p_flags`.
    pub fn flags(&self) -> u32 {
        self.phdr.p_flags
    }

    /// `PF_R` is set.
    pub fn is_readable(&self) -> bool {
        self.phdr.p_flags & PF_R != 0
    }

    /// `PF_W` is set. A segment without it is read-only.
    pub fn is_writable(&self) -> bool {
        self.phdr.p_flags & PF_W != 0
    }

    /// `PF_X` is set.
    pub fn is_executable(&self) -> bool {
        self.phdr.p_flags & PF_X != 0
    }

    /// The alignment: `p_align`, with 0 read as 1. A value that is not a
    /// power of two is an error.
    pub fn alignment(&self) -> Result<u64, Error> {
        match self.phdr.p_align {
            0 => Ok(1),
            align if align.is_power_of_two() => Ok(align),
            align => Err(Error::BadSegmentAlignment { segment: self.index, align }),
        }
    }

    /// Whether the memory image, `p_vaddr` to `p_vaddr + p_memsz`, holds
    /// `addr`. An image whose end overflows holds the addresses up to the
    /// largest.
    pub fn contains(&self, addr: u64) -> bool {
        let Phdr { p_vaddr, p_memsz, .. } = self.phdr;
        addr >= p_vaddr && addr - p_vaddr < p_memsz
    }

    /// Where the file contents are: `p_offset` to `p_offset + p_filesz`.
    pub fn file_range(&self) -> Result<Range<usize>, Error> {
        let Phdr { p_offset: offset, p_filesz: size, .. } = self.phdr;
        let file_len = self.file.len() as u64;
        let range = offset
            .checked_add(size)
            .filter(|end| *end <= file_len)
            .and_then(|end| Some(usize::try_from(offset).ok()?..usize::try_from(end).ok()?));
        range.ok_or(Error::SegmentOutOfBounds { segment: self.index, offset, size, file_len })
    }

    /// The file contents: `p_filesz` bytes at `p_offset`.
    pub fn data(&self) -> Result<&'a [u8], Error> {
        Ok(self.file.get(self.file_range()?).unwrap_or(&[]))
    }

    /// The notes of a `PT_NOTE` or `PT_GNU_PROPERTY` segment. Names and
    /// descriptors are padded to 8 bytes when `p_align` is 8 and to 4 bytes
    /// when it is at most 4; any other alignment is an error. Another type
    /// of segment is an error.
    pub fn notes(&self) -> Result<Notes<'a>, Error> {
        if !matches!(self.phdr.p_type, PT_NOTE | PT_GNU_PROPERTY) {
            return Err(Error::WrongSegmentType {
                segment: self.index,
                p_type: self.phdr.p_type,
                expected: PT_NOTE,
            });
        }
        let align = match self.phdr.p_align {
            0..=4 => 4,
            8 => 8,
            align => return Err(Error::SegmentNoteAlignment { segment: self.index, align }),
        };
        Ok(Notes::in_segment(self.data()?, align, self.class, self.index))
    }
}
