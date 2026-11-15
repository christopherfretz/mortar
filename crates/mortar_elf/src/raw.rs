//! The on-disk records of both ELF classes, widened to one set of types,
//! and [`Table`], a zero-copy array of them (`spec/elf.md` §3.2).
//!
//! A record is decoded from its bytes when it is asked for; nothing here
//! allocates. All records are little-endian.

use core::fmt;
use core::iter::FusedIterator;
use core::marker::PhantomData;

use crate::consts::STV_MASK;

/// The file class, `e_ident[EI_CLASS]`: which of the two record layouts a
/// file uses.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Class {
    /// `ELFCLASS32`.
    Elf32,
    /// `ELFCLASS64`.
    Elf64,
}

impl Class {
    /// The size of an address, offset or "word-sized" field in bytes: 4 or 8.
    pub const fn word_size(self) -> usize {
        match self {
            Class::Elf32 => 4,
            Class::Elf64 => 8,
        }
    }

    /// `"ELF32"` or `"ELF64"`.
    pub const fn name(self) -> &'static str {
        match self {
            Class::Elf32 => "ELF32",
            Class::Elf64 => "ELF64",
        }
    }
}

#[inline]
fn u16_at<const N: usize>(b: &[u8; N], at: usize) -> u16 {
    u16::from_le_bytes([b[at], b[at + 1]])
}

#[inline]
fn u32_at<const N: usize>(b: &[u8; N], at: usize) -> u32 {
    u32::from_le_bytes([b[at], b[at + 1], b[at + 2], b[at + 3]])
}

#[inline]
fn u64_at<const N: usize>(b: &[u8; N], at: usize) -> u64 {
    u64::from_le_bytes([
        b[at],
        b[at + 1],
        b[at + 2],
        b[at + 3],
        b[at + 4],
        b[at + 5],
        b[at + 6],
        b[at + 7],
    ])
}

/// A fixed-size on-disk record with a layout per class.
///
/// `decode32` and `decode64` read the record from the first `SIZE32` or
/// `SIZE64` bytes of `bytes` and return `None` when there are fewer.
pub trait Record: Copy {
    /// The record's size in an `ELFCLASS32` file.
    const SIZE32: usize;
    /// The record's size in an `ELFCLASS64` file.
    const SIZE64: usize;

    fn decode32(bytes: &[u8]) -> Option<Self>;
    fn decode64(bytes: &[u8]) -> Option<Self>;

    /// The record's size in a file of `class`.
    fn size(class: Class) -> usize {
        match class {
            Class::Elf32 => Self::SIZE32,
            Class::Elf64 => Self::SIZE64,
        }
    }

    /// Decodes the record at the start of `bytes`.
    fn decode(class: Class, bytes: &[u8]) -> Option<Self> {
        match class {
            Class::Elf32 => Self::decode32(bytes),
            Class::Elf64 => Self::decode64(bytes),
        }
    }
}

/// A section header (gABI "Section Header": `Elf32_Shdr`, 40 bytes;
/// `Elf64_Shdr`, 64 bytes).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Shdr {
    pub sh_name: u32,
    pub sh_type: u32,
    pub sh_flags: u64,
    pub sh_addr: u64,
    pub sh_offset: u64,
    pub sh_size: u64,
    pub sh_link: u32,
    pub sh_info: u32,
    pub sh_addralign: u64,
    pub sh_entsize: u64,
}

impl Record for Shdr {
    const SIZE32: usize = 40;
    const SIZE64: usize = 64;

    #[inline]
    fn decode32(bytes: &[u8]) -> Option<Self> {
        let b = bytes.first_chunk::<40>()?;
        Some(Shdr {
            sh_name: u32_at(b, 0),
            sh_type: u32_at(b, 4),
            sh_flags: u64::from(u32_at(b, 8)),
            sh_addr: u64::from(u32_at(b, 12)),
            sh_offset: u64::from(u32_at(b, 16)),
            sh_size: u64::from(u32_at(b, 20)),
            sh_link: u32_at(b, 24),
            sh_info: u32_at(b, 28),
            sh_addralign: u64::from(u32_at(b, 32)),
            sh_entsize: u64::from(u32_at(b, 36)),
        })
    }

    #[inline]
    fn decode64(bytes: &[u8]) -> Option<Self> {
        let b = bytes.first_chunk::<64>()?;
        Some(Shdr {
            sh_name: u32_at(b, 0),
            sh_type: u32_at(b, 4),
            sh_flags: u64_at(b, 8),
            sh_addr: u64_at(b, 16),
            sh_offset: u64_at(b, 24),
            sh_size: u64_at(b, 32),
            sh_link: u32_at(b, 40),
            sh_info: u32_at(b, 44),
            sh_addralign: u64_at(b, 48),
            sh_entsize: u64_at(b, 56),
        })
    }
}

/// A symbol table entry (gABI "Symbol Table": `Elf32_Sym`, 16 bytes;
/// `Elf64_Sym`, 24 bytes; the two order their fields differently).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Sym {
    pub st_name: u32,
    pub st_info: u8,
    pub st_other: u8,
    pub st_shndx: u16,
    pub st_value: u64,
    pub st_size: u64,
}

impl Sym {
    /// The binding, `st_info >> 4` (`STB_*`).
    pub const fn binding(&self) -> u8 {
        self.st_info >> 4
    }

    /// The type, `st_info & 0xf` (`STT_*`).
    pub const fn sym_type(&self) -> u8 {
        self.st_info & 0xf
    }

    /// The visibility, `st_other & 3` (`STV_*`).
    pub const fn visibility(&self) -> u8 {
        self.st_other & STV_MASK
    }
}

impl Record for Sym {
    const SIZE32: usize = 16;
    const SIZE64: usize = 24;

    #[inline]
    fn decode32(bytes: &[u8]) -> Option<Self> {
        let b = bytes.first_chunk::<16>()?;
        Some(Sym {
            st_name: u32_at(b, 0),
            st_value: u64::from(u32_at(b, 4)),
            st_size: u64::from(u32_at(b, 8)),
            st_info: b[12],
            st_other: b[13],
            st_shndx: u16_at(b, 14),
        })
    }

    #[inline]
    fn decode64(bytes: &[u8]) -> Option<Self> {
        let b = bytes.first_chunk::<24>()?;
        Some(Sym {
            st_name: u32_at(b, 0),
            st_info: b[4],
            st_other: b[5],
            st_shndx: u16_at(b, 6),
            st_value: u64_at(b, 8),
            st_size: u64_at(b, 16),
        })
    }
}

/// A relocation with an explicit addend (gABI "Relocation": `Elf32_Rela`,
/// 12 bytes; `Elf64_Rela`, 24 bytes).
///
/// `r_info` is stored unpacked: it holds the symbol index and the type as
/// 24 and 8 bits in `ELFCLASS32` and as 32 and 32 bits in `ELFCLASS64`.
/// The `ELFCLASS32` addend is sign-extended.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Rela {
    pub r_offset: u64,
    /// The symbol table index, `ELF32_R_SYM` or `ELF64_R_SYM` of `r_info`.
    pub r_sym: u32,
    /// The relocation type, `ELF32_R_TYPE` or `ELF64_R_TYPE` of `r_info`.
    pub r_type: u32,
    pub r_addend: i64,
}

impl Record for Rela {
    const SIZE32: usize = 12;
    const SIZE64: usize = 24;

    #[inline]
    fn decode32(bytes: &[u8]) -> Option<Self> {
        let b = bytes.first_chunk::<12>()?;
        let info = u32_at(b, 4);
        Some(Rela {
            r_offset: u64::from(u32_at(b, 0)),
            r_sym: info >> 8,
            r_type: info & 0xff,
            r_addend: i64::from(u32_at(b, 8).cast_signed()),
        })
    }

    #[inline]
    fn decode64(bytes: &[u8]) -> Option<Self> {
        let b = bytes.first_chunk::<24>()?;
        Some(Rela {
            r_offset: u64_at(b, 0),
            r_type: u32_at(b, 8),
            r_sym: u32_at(b, 12),
            r_addend: u64_at(b, 16).cast_signed(),
        })
    }
}

/// A relocation whose addend is in the relocated field (gABI
/// "Relocation": `Elf32_Rel`, 8 bytes; `Elf64_Rel`, 16 bytes). `r_info`
/// is unpacked as in [`Rela`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Rel {
    pub r_offset: u64,
    pub r_sym: u32,
    pub r_type: u32,
}

impl Record for Rel {
    const SIZE32: usize = 8;
    const SIZE64: usize = 16;

    #[inline]
    fn decode32(bytes: &[u8]) -> Option<Self> {
        let b = bytes.first_chunk::<8>()?;
        let info = u32_at(b, 4);
        Some(Rel { r_offset: u64::from(u32_at(b, 0)), r_sym: info >> 8, r_type: info & 0xff })
    }

    #[inline]
    fn decode64(bytes: &[u8]) -> Option<Self> {
        let b = bytes.first_chunk::<16>()?;
        Some(Rel { r_offset: u64_at(b, 0), r_type: u32_at(b, 8), r_sym: u32_at(b, 12) })
    }
}

/// A 32-bit word, the entry of `SHT_GROUP` and `SHT_SYMTAB_SHNDX`
/// sections in both classes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Word(pub u32);

impl Record for Word {
    const SIZE32: usize = 4;
    const SIZE64: usize = 4;

    #[inline]
    fn decode32(bytes: &[u8]) -> Option<Self> {
        Some(Word(u32::from_le_bytes(*bytes.first_chunk::<4>()?)))
    }

    #[inline]
    fn decode64(bytes: &[u8]) -> Option<Self> {
        Self::decode32(bytes)
    }
}

/// The header of a compressed section (gABI "Section Compression":
/// `Elf32_Chdr`, 12 bytes; `Elf64_Chdr`, 24 bytes, of which 4 are
/// reserved).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Chdr {
    /// The algorithm (`ELFCOMPRESS_*`).
    pub ch_type: u32,
    /// The size of the uncompressed contents.
    pub ch_size: u64,
    /// The alignment of the uncompressed contents.
    pub ch_addralign: u64,
}

impl Record for Chdr {
    const SIZE32: usize = 12;
    const SIZE64: usize = 24;

    #[inline]
    fn decode32(bytes: &[u8]) -> Option<Self> {
        let b = bytes.first_chunk::<12>()?;
        Some(Chdr {
            ch_type: u32_at(b, 0),
            ch_size: u64::from(u32_at(b, 4)),
            ch_addralign: u64::from(u32_at(b, 8)),
        })
    }

    #[inline]
    fn decode64(bytes: &[u8]) -> Option<Self> {
        let b = bytes.first_chunk::<24>()?;
        Some(Chdr { ch_type: u32_at(b, 0), ch_size: u64_at(b, 8), ch_addralign: u64_at(b, 16) })
    }
}

/// A note header (gABI "Note Section"): three 32-bit words in both
/// classes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Nhdr {
    pub n_namesz: u32,
    pub n_descsz: u32,
    pub n_type: u32,
}

impl Record for Nhdr {
    const SIZE32: usize = 12;
    const SIZE64: usize = 12;

    #[inline]
    fn decode32(bytes: &[u8]) -> Option<Self> {
        let b = bytes.first_chunk::<12>()?;
        Some(Nhdr { n_namesz: u32_at(b, 0), n_descsz: u32_at(b, 4), n_type: u32_at(b, 8) })
    }

    #[inline]
    fn decode64(bytes: &[u8]) -> Option<Self> {
        Self::decode32(bytes)
    }
}

/// The file header (gABI "ELF Header": `Elf32_Ehdr`, 52 bytes;
/// `Elf64_Ehdr`, 64 bytes), without the magic, class and data encoding,
/// which [`crate::FileHeader`] holds.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Ehdr {
    /// `e_ident[EI_VERSION]`.
    pub ei_version: u8,
    /// `e_ident[EI_OSABI]`.
    pub ei_osabi: u8,
    /// `e_ident[EI_ABIVERSION]`.
    pub ei_abiversion: u8,
    pub e_type: u16,
    pub e_machine: u16,
    pub e_version: u32,
    pub e_entry: u64,
    pub e_phoff: u64,
    pub e_shoff: u64,
    pub e_flags: u32,
    pub e_ehsize: u16,
    pub e_phentsize: u16,
    pub e_phnum: u16,
    pub e_shentsize: u16,
    pub e_shnum: u16,
    pub e_shstrndx: u16,
}

impl Record for Ehdr {
    const SIZE32: usize = 52;
    const SIZE64: usize = 64;

    fn decode32(bytes: &[u8]) -> Option<Self> {
        let b = bytes.first_chunk::<52>()?;
        Some(Ehdr {
            ei_version: b[6],
            ei_osabi: b[7],
            ei_abiversion: b[8],
            e_type: u16_at(b, 16),
            e_machine: u16_at(b, 18),
            e_version: u32_at(b, 20),
            e_entry: u64::from(u32_at(b, 24)),
            e_phoff: u64::from(u32_at(b, 28)),
            e_shoff: u64::from(u32_at(b, 32)),
            e_flags: u32_at(b, 36),
            e_ehsize: u16_at(b, 40),
            e_phentsize: u16_at(b, 42),
            e_phnum: u16_at(b, 44),
            e_shentsize: u16_at(b, 46),
            e_shnum: u16_at(b, 48),
            e_shstrndx: u16_at(b, 50),
        })
    }

    fn decode64(bytes: &[u8]) -> Option<Self> {
        let b = bytes.first_chunk::<64>()?;
        Some(Ehdr {
            ei_version: b[6],
            ei_osabi: b[7],
            ei_abiversion: b[8],
            e_type: u16_at(b, 16),
            e_machine: u16_at(b, 18),
            e_version: u32_at(b, 20),
            e_entry: u64_at(b, 24),
            e_phoff: u64_at(b, 32),
            e_shoff: u64_at(b, 40),
            e_flags: u32_at(b, 48),
            e_ehsize: u16_at(b, 52),
            e_phentsize: u16_at(b, 54),
            e_phnum: u16_at(b, 56),
            e_shentsize: u16_at(b, 58),
            e_shnum: u16_at(b, 60),
            e_shstrndx: u16_at(b, 62),
        })
    }
}

/// A zero-copy array of records of one class: `len` records of
/// `T::size(class)` bytes each, decoded on access.
///
/// `Copy`, and borrows only the input bytes.
pub struct Table<'a, T> {
    bytes: &'a [u8],
    len: usize,
    class: Class,
    record: PhantomData<fn() -> T>,
}

impl<T> Clone for Table<'_, T> {
    fn clone(&self) -> Self {
        *self
    }
}

impl<T> Copy for Table<'_, T> {}

impl<T> fmt::Debug for Table<'_, T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Table").field("len", &self.len).field("class", &self.class).finish()
    }
}

impl<'a, T: Record> Table<'a, T> {
    /// A table over the whole records in `bytes`; a trailing partial
    /// record is left out.
    pub fn new(bytes: &'a [u8], class: Class) -> Self {
        let size = T::size(class);
        let len = bytes.len() / size;
        Table { bytes: &bytes[..len * size], len, class, record: PhantomData }
    }

    /// A table of no records.
    pub fn empty(class: Class) -> Self {
        Table { bytes: &[], len: 0, class, record: PhantomData }
    }

    /// The number of records.
    pub fn len(&self) -> usize {
        self.len
    }

    pub fn is_empty(&self) -> bool {
        self.len == 0
    }

    pub fn class(&self) -> Class {
        self.class
    }

    /// The bytes of all records.
    pub fn bytes(&self) -> &'a [u8] {
        self.bytes
    }

    /// Record `index`, or `None` when `index >= len()`.
    #[inline]
    pub fn get(&self, index: usize) -> Option<T> {
        match self.class {
            Class::Elf32 => T::decode32(self.bytes.get(index.checked_mul(T::SIZE32)?..)?),
            Class::Elf64 => T::decode64(self.bytes.get(index.checked_mul(T::SIZE64)?..)?),
        }
    }

    /// The records `start..end`, or `None` when the range is not within
    /// the table. For splitting a table between workers.
    pub fn slice(&self, start: usize, end: usize) -> Option<Self> {
        if start > end || end > self.len {
            return None;
        }
        let size = T::size(self.class);
        let bytes = self.bytes.get(start * size..end * size)?;
        Some(Table { bytes, len: end - start, class: self.class, record: PhantomData })
    }

    /// The records in order.
    pub fn iter(&self) -> TableIter<'a, T> {
        TableIter { bytes: self.bytes, class: self.class, record: PhantomData }
    }
}

impl<'a, T: Record> IntoIterator for Table<'a, T> {
    type Item = T;
    type IntoIter = TableIter<'a, T>;

    fn into_iter(self) -> TableIter<'a, T> {
        self.iter()
    }
}

impl<'a, T: Record> IntoIterator for &Table<'a, T> {
    type Item = T;
    type IntoIter = TableIter<'a, T>;

    fn into_iter(self) -> TableIter<'a, T> {
        self.iter()
    }
}

/// The records of a [`Table`], in order.
///
/// `next` tests the class once per record. `fold`, and with it
/// `for_each`, `count`, `sum` and the other adapters built on `fold`, test
/// it once per iteration: each class has its own loop over fixed-size
/// chunks.
pub struct TableIter<'a, T> {
    bytes: &'a [u8],
    class: Class,
    record: PhantomData<fn() -> T>,
}

impl<T> Clone for TableIter<'_, T> {
    fn clone(&self) -> Self {
        TableIter { bytes: self.bytes, class: self.class, record: PhantomData }
    }
}

impl<T> fmt::Debug for TableIter<'_, T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("TableIter")
            .field("bytes", &self.bytes.len())
            .field("class", &self.class)
            .finish()
    }
}

impl<T: Record> Iterator for TableIter<'_, T> {
    type Item = T;

    #[inline]
    fn next(&mut self) -> Option<T> {
        let (head, rest) = self.bytes.split_at_checked(T::size(self.class))?;
        self.bytes = rest;
        T::decode(self.class, head)
    }

    fn size_hint(&self) -> (usize, Option<usize>) {
        let len = self.bytes.len() / T::size(self.class);
        (len, Some(len))
    }

    fn nth(&mut self, n: usize) -> Option<T> {
        let skip = n.saturating_mul(T::size(self.class));
        self.bytes = self.bytes.get(skip..).unwrap_or(&[]);
        self.next()
    }

    #[inline]
    fn fold<B, F>(self, init: B, mut f: F) -> B
    where
        F: FnMut(B, T) -> B,
    {
        match self.class {
            Class::Elf32 => self.bytes.chunks_exact(T::SIZE32).fold(init, |acc, chunk| {
                match T::decode32(chunk) {
                    Some(record) => f(acc, record),
                    None => acc,
                }
            }),
            Class::Elf64 => self.bytes.chunks_exact(T::SIZE64).fold(init, |acc, chunk| {
                match T::decode64(chunk) {
                    Some(record) => f(acc, record),
                    None => acc,
                }
            }),
        }
    }
}

impl<T: Record> DoubleEndedIterator for TableIter<'_, T> {
    fn next_back(&mut self) -> Option<T> {
        let at = self.bytes.len().checked_sub(T::size(self.class))?;
        let (rest, tail) = self.bytes.split_at_checked(at)?;
        self.bytes = rest;
        T::decode(self.class, tail)
    }
}

impl<T: Record> ExactSizeIterator for TableIter<'_, T> {}
impl<T: Record> FusedIterator for TableIter<'_, T> {}
