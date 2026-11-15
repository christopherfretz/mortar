//! What the staged store keeps resident (`spec/input.md` §11.1 (b),
//! §11.3): a function of a file's bytes and nothing else.
//!
//! The rules read the structure through [`Fetch`], which the store answers
//! from what it has already read or with one more positioned read, so the
//! same functions decide the parts whatever was read first. They return
//! candidate ranges; the store merges them into parts ([`merge`]).

use std::ops::Range;

use mortar_elf::archive::{self, MAGIC};
use mortar_elf::consts::{
    EI_CLASS, EI_DATA, ELFCLASS32, ELFCLASS64, ELFDATA2LSB, EM_X86_64, ET_REL, SHF_ALLOC,
    SHF_COMPRESSED, SHF_MERGE, SHN_XINDEX, SHT_DYNSYM, SHT_GROUP, SHT_NOBITS, SHT_NOTE, SHT_NULL,
    SHT_REL, SHT_RELA, SHT_STRTAB, SHT_SYMTAB, SHT_SYMTAB_SHNDX, SHT_X86_64_UNWIND,
};
use mortar_elf::{Class, Ehdr, Record, Shdr, Table};

/// Sections of at most this size are resident whatever they are, and so
/// is an object or archive member of at most this size, whole (§11.1 (b)).
pub const SMALL: u64 = 64 << 10;

/// Resident ranges separated by less than this are one part (§11.3).
pub const GAP: u64 = 4 << 10;

/// The bytes of the file header that are always resident: enough for
/// either class's ELF header, the sniff of §2.1 and an archive's magic.
pub const PREFIX: u64 = 64;

/// What the rules read: bytes of the file at an absolute offset.
pub trait Fetch {
    /// The `size` bytes at `at`, which lie within the file.
    fn fetch(&mut self, at: u64, size: u64) -> Result<&[u8], crate::store::ReadError>;
}

/// What a file (or member) is, as far as the plan cares, from its first
/// bytes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Shape {
    /// A little-endian ELF relocatable object of either class.
    Object,
    /// A regular (not thin) archive.
    Archive,
    /// Anything else: read whole (a top-level file) or only its prefix
    /// (an archive member, §7.3: what it is is all the walk asks).
    Other,
}

/// The shape of a file from its first [`PREFIX`] bytes (or all of them).
#[must_use]
pub fn shape(head: &[u8]) -> Shape {
    if head.starts_with(MAGIC) {
        return Shape::Archive;
    }
    let elf = head.starts_with(b"\x7fELF")
        && head.len() >= 18
        && matches!(head[EI_CLASS], ELFCLASS32 | ELFCLASS64)
        && head[EI_DATA] == ELFDATA2LSB
        && u16::from_le_bytes([head[16], head[17]]) == ET_REL;
    if elf { Shape::Object } else { Shape::Other }
}

/// The resident ranges of the relocatable object at `base` of `size`
/// bytes, appended to `out`, and the ranges of its Mason containers
/// (`spec/meta.md` MT-6), which are never resident (amendment A-8 to
/// §11.1), appended to `containers`; `Ok(false)` when its section
/// structure is not one the rules can follow (a bad `e_shentsize`, a
/// section header table out of bounds, an absurd count): the caller then
/// keeps it whole, and the reader finds the error in the whole bytes,
/// exactly as the resident store's does.
pub fn object_ranges(
    src: &mut dyn Fetch,
    base: u64,
    size: u64,
    out: &mut Vec<Range<u64>>,
    containers: &mut Vec<Range<u64>>,
) -> Result<bool, crate::store::ReadError> {
    let head = src.fetch(base, size.min(PREFIX))?;
    let class = if head.get(EI_CLASS) == Some(&ELFCLASS32) { Class::Elf32 } else { Class::Elf64 };
    let Some(ehdr) = Ehdr::decode(class, head) else { return Ok(false) };
    out.push(base..base + size.min(PREFIX));
    let entsize = Shdr::size(class) as u64;
    let shoff = ehdr.e_shoff;
    if shoff == 0 {
        // No section header table: the reader needs nothing more.
        return Ok(true);
    }
    if u64::from(ehdr.e_shentsize) != entsize || !within(shoff, entsize, size) {
        return Ok(false);
    }
    let first = Shdr::decode(class, src.fetch(base + shoff, entsize)?);
    let Some(first) = first else { return Ok(false) };
    let count = if ehdr.e_shnum != 0 { u64::from(ehdr.e_shnum) } else { first.sh_size };
    let Some(table_size) = count.checked_mul(entsize) else { return Ok(false) };
    if count > u64::from(u32::MAX) || !within(shoff, table_size, size) {
        return Ok(false);
    }
    out.push(base + shoff..base + shoff + table_size);
    let table = src.fetch(base + shoff, table_size)?.to_vec();
    let headers: Table<'_, Shdr> = Table::new(&table, class);
    let shstrndx = if ehdr.e_shstrndx == SHN_XINDEX {
        u64::from(first.sh_link)
    } else {
        u64::from(ehdr.e_shstrndx)
    };
    let body = |s: &Shdr| -> Option<Range<u64>> {
        let has = !matches!(s.sh_type, SHT_NOBITS | SHT_NULL) && s.sh_size > 0;
        (has && within(s.sh_offset, s.sh_size, size))
            .then(|| base + s.sh_offset..base + s.sh_offset + s.sh_size)
    };
    // The section names, for the rules that go by name.
    let names: Vec<u8> = match usize::try_from(shstrndx).ok().and_then(|i| headers.get(i)) {
        Some(s) if shstrndx != 0 => match body(&s) {
            Some(r) => {
                out.push(r.clone());
                src.fetch(r.start, r.end - r.start)?.to_vec()
            }
            None => Vec::new(),
        },
        _ => Vec::new(),
    };
    let name = |s: &Shdr| -> &[u8] {
        let at = s.sh_name as usize;
        let tail = names.get(at..).unwrap_or_default();
        tail.iter().position(|&b| b == 0).map_or(&[][..], |nul| &tail[..nul])
    };
    let section = |i: u32| headers.get(i as usize);
    for (index, s) in (0u32..).zip(headers.iter()) {
        if index == 0 {
            continue;
        }
        let Some(range) = body(&s) else { continue };
        // A-8: a container is never resident, whatever its size; the
        // name is looked at only for a section MT-6's type and flags
        // allow.
        if mortar_meta::is_candidate(s.sh_type, s.sh_flags)
            && mortar_meta::is_container(name(&s), s.sh_type, s.sh_flags)
        {
            containers.push(range);
            continue;
        }
        let resident = match s.sh_type {
            SHT_SYMTAB | SHT_DYNSYM => {
                // And its string table, whatever its type: the reader
                // checks it.
                if let Some(r) =
                    section(s.sh_link).filter(|_| s.sh_link != 0).and_then(|t| body(&t))
                {
                    out.push(r);
                }
                true
            }
            SHT_STRTAB | SHT_SYMTAB_SHNDX | SHT_GROUP | SHT_NOTE => true,
            // Relocations of an allocated section: liveness, the scan and
            // fill read them (spec/image.md I-82). Of anything else (debug
            // sections), not: read with `copy_to`, twice.
            SHT_REL | SHT_RELA => match section(s.sh_info).filter(|_| s.sh_info != 0) {
                Some(target) => target.sh_flags & SHF_ALLOC != 0 || s.sh_flags & SHF_ALLOC != 0,
                None => true,
            },
            sh_type => {
                // Split and merged by phase 60 (spec/synth.md §3, §4), or
                // read by the readers (a compression header), or small.
                s.sh_flags & SHF_COMPRESSED != 0
                    || (s.sh_flags & SHF_MERGE != 0 && s.sh_entsize != 0)
                    || (sh_type == SHT_X86_64_UNWIND && ehdr.e_machine == EM_X86_64)
                    || name(&s) == b".eh_frame"
                    || name(&s).starts_with(b".gnu.warning")
                    || s.sh_size <= SMALL
            }
        };
        if resident {
            out.push(range);
        }
    }
    Ok(true)
}

/// The ranges of the Mason containers (`spec/meta.md` MT-6) of the
/// relocatable object at `base` of `size` bytes, found by reading only its
/// ELF header, its section header table and, when a section has a
/// container's type and flags, its `.shstrtab` (MT-42: "mortar reads the
/// section header ... and nothing else of it"). Each structure is fetched
/// exactly, so that nothing past it is read. `None` when the section
/// structure is not one the rules follow (the caller reads the file as
/// before; the reader then refuses it).
pub fn container_ranges(
    src: &mut dyn Fetch,
    base: u64,
    size: u64,
) -> Result<Option<Vec<Range<u64>>>, crate::store::ReadError> {
    let head = src.fetch(base, size.min(PREFIX))?;
    let class = if head.get(EI_CLASS) == Some(&ELFCLASS32) { Class::Elf32 } else { Class::Elf64 };
    let Some(ehdr) = Ehdr::decode(class, head) else { return Ok(None) };
    let entsize = Shdr::size(class) as u64;
    let shoff = ehdr.e_shoff;
    if shoff == 0 {
        return Ok(Some(Vec::new()));
    }
    if u64::from(ehdr.e_shentsize) != entsize || !within(shoff, entsize, size) {
        return Ok(None);
    }
    let Some(first) = Shdr::decode(class, src.fetch(base + shoff, entsize)?) else {
        return Ok(None);
    };
    let count = if ehdr.e_shnum != 0 { u64::from(ehdr.e_shnum) } else { first.sh_size };
    let Some(table_size) = count.checked_mul(entsize) else { return Ok(None) };
    if count > u64::from(u32::MAX) || !within(shoff, table_size, size) {
        return Ok(None);
    }
    // Borrowed, not copied: a file with no candidate (nearly every one)
    // costs no allocation here.
    let headers: Table<'_, Shdr> = Table::new(src.fetch(base + shoff, table_size)?, class);
    let candidates: Vec<Shdr> = headers
        .iter()
        .skip(1)
        .filter(|s| mortar_meta::is_candidate(s.sh_type, s.sh_flags))
        .filter(|s| s.sh_size > 0 && within(s.sh_offset, s.sh_size, size))
        .collect();
    if candidates.is_empty() {
        return Ok(Some(Vec::new()));
    }
    let shstrndx = if ehdr.e_shstrndx == SHN_XINDEX {
        u64::from(first.sh_link)
    } else {
        u64::from(ehdr.e_shstrndx)
    };
    let strtab = usize::try_from(shstrndx).ok().and_then(|i| headers.get(i));
    let names = match strtab {
        Some(t)
            if shstrndx != 0 && t.sh_type != SHT_NOBITS && within(t.sh_offset, t.sh_size, size) =>
        {
            src.fetch(base + t.sh_offset, t.sh_size)?.to_vec()
        }
        _ => Vec::new(),
    };
    let mut out: Vec<Range<u64>> = candidates
        .iter()
        .filter(|s| {
            let tail = names.get(s.sh_name as usize..).unwrap_or_default();
            let name = tail.iter().position(|&b| b == 0).map_or(&[][..], |nul| &tail[..nul]);
            mortar_meta::is_container(name, s.sh_type, s.sh_flags)
        })
        .map(|s| base + s.sh_offset..base + s.sh_offset + s.sh_size)
        .collect();
    out.sort_by_key(|r| (r.start, r.end));
    Ok(Some(out))
}

/// `range` less the `forbidden` ranges (sorted by start): the pieces, in
/// order.
#[must_use]
pub fn clip(range: Range<u64>, forbidden: &[Range<u64>]) -> Vec<Range<u64>> {
    let mut out = Vec::new();
    let mut at = range.start;
    for f in forbidden {
        if f.end <= at || f.start >= range.end {
            continue;
        }
        if f.start > at {
            out.push(at..f.start);
        }
        at = at.max(f.end);
    }
    if at < range.end {
        out.push(at..range.end);
    }
    out
}

/// Whether `start..start + len` lies within `0..size`.
fn within(start: u64, len: u64, size: u64) -> bool {
    start.checked_add(len).is_some_and(|end| end <= size)
}

/// One member of a regular archive the chain found: where its content is.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct MemberSpan {
    pub content: u64,
    pub size: u64,
}

/// The front and the headers of a regular archive of `len` bytes (§7):
/// the resident ranges (the magic, every special member whole, every
/// member header and BSD name) appended to `out`, and the ordinary members
/// whose contents the caller plans. The chain stops at the first header
/// the reader would refuse, keeping its bytes resident so that the reader
/// gives the same error.
pub fn archive_ranges(
    src: &mut dyn Fetch,
    len: u64,
    out: &mut Vec<Range<u64>>,
    members: &mut Vec<MemberSpan>,
) -> Result<(), crate::store::ReadError> {
    let magic = MAGIC.len() as u64;
    out.push(0..magic.min(len));
    let mut at = magic;
    while at < len {
        let raw_len = archive::HEADER_LEN.min(len - at);
        let raw = src.fetch(at, raw_len)?;
        let Ok(span) = archive::header_span(at, raw, len) else {
            out.push(at..at + raw_len);
            break;
        };
        out.push(at..span.content + span.front);
        if span.front < span.size {
            members.push(MemberSpan {
                content: span.content + span.front,
                size: span.size - span.front,
            });
        }
        if span.next <= at {
            break;
        }
        at = span.next;
    }
    Ok(())
}

/// Sorts `ranges` and merges those that overlap, touch, or are less than
/// [`GAP`] apart, into the parts of §11.3; never across a byte of a
/// `forbidden` range (a Mason container, A-8: the bytes between two
/// ranges a merge joins become resident).
#[must_use]
pub fn merge(mut ranges: Vec<Range<u64>>, forbidden: &[Range<u64>]) -> Vec<Range<u64>> {
    ranges.retain(|r| r.start < r.end);
    ranges.sort_by_key(|r| (r.start, r.end));
    let mut out: Vec<Range<u64>> = Vec::with_capacity(ranges.len());
    for r in ranges {
        match out.last_mut() {
            Some(last)
                if r.start < last.end.saturating_add(GAP)
                    && (r.start <= last.end
                        || !forbidden.iter().any(|f| f.start < r.start && f.end > last.end)) =>
            {
                last.end = last.end.max(r.end);
            }
            _ => out.push(r),
        }
    }
    out
}
