//! Notes and GNU program properties (`spec/elf.md` §3.8, §6.8). Shared by
//! every reader of this crate.

use core::iter::FusedIterator;

use crate::consts::{ELF_NOTE_GNU, NT_GNU_PROPERTY_TYPE_0};
use crate::error::{Error, NotePart};
use crate::raw::{Class, Nhdr, Record};

/// Where a sequence of notes is: the contents of a section, or of a
/// segment. Errors name it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Origin {
    Section(u32),
    Segment(u32),
}

impl Origin {
    fn bad_note(self, offset: usize, part: NotePart) -> Error {
        let offset = offset as u64;
        match self {
            Origin::Section(section) => Error::BadNote { section, offset, part },
            Origin::Segment(segment) => Error::BadSegmentNote { segment, offset, part },
        }
    }
}

/// `value` rounded up to a multiple of `align` (a power of two).
fn align_up(value: usize, align: usize) -> Option<usize> {
    Some(value.checked_add(align - 1)? & !(align - 1))
}

/// The notes of a note section or segment, in order (gABI "Note
/// Section"). Borrows only the input bytes.
///
/// An item is a note or the error that ends the iteration: after an
/// error the iterator is finished.
#[derive(Debug, Clone)]
pub struct Notes<'a> {
    data: &'a [u8],
    offset: usize,
    align: usize,
    class: Class,
    origin: Origin,
}

impl<'a> Notes<'a> {
    /// The notes in `data`, with names and descriptors padded to `align`
    /// bytes (4 or 8; any other value is read as 4). `section` is the
    /// index errors name (0 for bytes that are not a section's).
    pub fn new(data: &'a [u8], align: usize, class: Class, section: u32) -> Self {
        let align = if align == 8 { 8 } else { 4 };
        Notes { data, offset: 0, align, class, origin: Origin::Section(section) }
    }

    /// The notes in `data`, the contents of program header `segment`, as
    /// [`Notes::new`]. Errors are [`Error::BadSegmentNote`], with offsets
    /// from the start of the segment.
    pub fn in_segment(data: &'a [u8], align: usize, class: Class, segment: u32) -> Self {
        let align = if align == 8 { 8 } else { 4 };
        Notes { data, offset: 0, align, class, origin: Origin::Segment(segment) }
    }

    fn fail(&mut self, offset: usize, part: NotePart) -> Error {
        self.offset = self.data.len();
        self.origin.bad_note(offset, part)
    }
}

impl<'a> Iterator for Notes<'a> {
    type Item = Result<Note<'a>, Error>;

    fn next(&mut self) -> Option<Self::Item> {
        let offset = self.offset;
        let rest = self.data.get(offset..).filter(|rest| !rest.is_empty())?;
        let Some(header) = Nhdr::decode32(rest) else {
            return Some(Err(self.fail(offset, NotePart::Header)));
        };
        let name_end = Nhdr::SIZE32.checked_add(header.n_namesz as usize);
        let Some(name) = name_end.and_then(|end| rest.get(Nhdr::SIZE32..end)) else {
            return Some(Err(self.fail(offset, NotePart::Name)));
        };
        let desc_start = name_end.and_then(|end| align_up(end, self.align));
        let desc_end = desc_start.and_then(|start| start.checked_add(header.n_descsz as usize));
        let desc = if header.n_descsz == 0 {
            // The padding after the name may be missing at the end.
            Some(&[][..])
        } else {
            desc_start.zip(desc_end).and_then(|(start, end)| rest.get(start..end))
        };
        let (Some(desc), Some(desc_end)) = (desc, desc_end) else {
            return Some(Err(self.fail(offset, NotePart::Descriptor)));
        };
        // The padding after the last descriptor may be missing.
        let size = align_up(desc_end, self.align).map_or(rest.len(), |end| end.min(rest.len()));
        self.offset = offset + size;
        Some(Ok(Note {
            n_type: header.n_type,
            name,
            desc,
            desc_offset: offset + desc_start.unwrap_or(0),
            class: self.class,
            origin: self.origin,
        }))
    }
}

impl FusedIterator for Notes<'_> {}

/// One note. `Copy`, and borrows only the input bytes.
#[derive(Debug, Clone, Copy)]
pub struct Note<'a> {
    /// `n_type`; its meaning depends on the name.
    pub n_type: u32,
    name: &'a [u8],
    desc: &'a [u8],
    desc_offset: usize,
    class: Class,
    origin: Origin,
}

impl<'a> Note<'a> {
    /// The name (the owner) as stored: `n_namesz` bytes, which include
    /// the terminating NUL when the producer wrote one.
    pub fn name_bytes(&self) -> &'a [u8] {
        self.name
    }

    /// The name without one terminating NUL.
    pub fn name(&self) -> &'a [u8] {
        self.name.strip_suffix(&[0]).unwrap_or(self.name)
    }

    /// The descriptor: `n_descsz` bytes.
    pub fn desc(&self) -> &'a [u8] {
        self.desc
    }

    /// The program properties of a `GNU` note of type
    /// `NT_GNU_PROPERTY_TYPE_0`, or `None` for any other note.
    pub fn gnu_properties(&self) -> Option<Properties<'a>> {
        (self.n_type == NT_GNU_PROPERTY_TYPE_0 && self.name() == ELF_NOTE_GNU).then_some(
            Properties {
                data: self.desc,
                offset: 0,
                base: self.desc_offset,
                word: self.class.word_size(),
                origin: self.origin,
            },
        )
    }
}

/// The program properties of a `.note.gnu.property` note, in order (Linux
/// gABI extensions, "Program Property"). Each is an 8-byte header and
/// data padded to 4 bytes in `ELFCLASS32` and to 8 in `ELFCLASS64`.
///
/// An item is a property or the error that ends the iteration.
#[derive(Debug, Clone)]
pub struct Properties<'a> {
    data: &'a [u8],
    offset: usize,
    base: usize,
    word: usize,
    origin: Origin,
}

impl<'a> Iterator for Properties<'a> {
    type Item = Result<Property<'a>, Error>;

    fn next(&mut self) -> Option<Self::Item> {
        let offset = self.offset;
        let rest = self.data.get(offset..).filter(|rest| !rest.is_empty())?;
        let property = rest.first_chunk::<8>().and_then(|header| {
            let [t0, t1, t2, t3, s0, s1, s2, s3] = *header;
            let size = u32::from_le_bytes([s0, s1, s2, s3]) as usize;
            let end = size.checked_add(8)?;
            let data = rest.get(8..end)?;
            Some((Property { pr_type: u32::from_le_bytes([t0, t1, t2, t3]), data }, end))
        });
        let Some((property, end)) = property else {
            self.offset = self.data.len();
            return Some(Err(self.origin.bad_note(self.base + offset, NotePart::Property)));
        };
        // The padding after the last property may be missing.
        let size = align_up(end, self.word).map_or(rest.len(), |end| end.min(rest.len()));
        self.offset = offset + size;
        Some(Ok(property))
    }
}

impl FusedIterator for Properties<'_> {}

/// One program property.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Property<'a> {
    /// `pr_type` (`GNU_PROPERTY_*`).
    pub pr_type: u32,
    /// `pr_datasz` bytes of data.
    pub data: &'a [u8],
}

impl Property<'_> {
    /// The data as one 32-bit word (a feature or ISA word), or `None`
    /// when the data is not 4 bytes.
    pub fn as_u32(&self) -> Option<u32> {
        Some(u32::from_le_bytes(*self.data.first_chunk::<4>().filter(|_| self.data.len() == 4)?))
    }
}
