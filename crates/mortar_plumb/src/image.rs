//! The output as a loader reads it (`spec/plumb.md` §2.1): the file
//! header, the program headers, the section headers and `.symtab`.
//! Memory is read through the `PT_LOAD` segments.

use crate::hasher::HashMap;

use mortar_elf::consts::{PF_X, PT_LOAD, SHF_ALLOC, SHT_NOBITS, SHT_SYMTAB, STB_GLOBAL, STB_WEAK};
use mortar_elf::{Class, FileHeader, SectionTable, Shdr, Sym};

/// A program header, widened (gABI "Program Header").
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[allow(clippy::struct_field_names)] // the gABI's names
pub struct Phdr {
    pub p_type: u32,
    pub p_flags: u32,
    pub p_offset: u64,
    pub p_vaddr: u64,
    pub p_paddr: u64,
    pub p_filesz: u64,
    pub p_memsz: u64,
    pub p_align: u64,
}

/// A section of the output.
#[derive(Debug, Clone)]
pub struct OutSection {
    pub name: String,
    pub shdr: Shdr,
}

impl OutSection {
    pub fn alloc(&self) -> bool {
        self.shdr.sh_flags & SHF_ALLOC != 0
    }
}

/// A symbol of the output's `.symtab`.
#[derive(Debug, Clone)]
pub struct OutSym {
    pub name: String,
    pub sym: Sym,
    /// The index in `.symtab`.
    pub index: u32,
}

/// What a name means in the output's `.symtab` (§4.4).
#[derive(Debug, Clone, Copy, Default)]
struct NameEntry {
    /// The first `STB_GLOBAL` or `STB_WEAK` symbol of the name.
    global: Option<u32>,
    /// The first symbol of the name, of any binding.
    first: Option<u32>,
    /// How many symbols have the name.
    count: u32,
}

/// The output file, read.
#[derive(Debug)]
pub struct Image<'a> {
    pub data: &'a [u8],
    pub header: FileHeader,
    pub phdrs: Vec<Phdr>,
    /// Section headers; index 0 is the null section.
    pub sections: Vec<OutSection>,
    /// Indexes of `PT_LOAD` headers sorted by address.
    loads: Vec<usize>,
    pub symbols: Vec<OutSym>,
    names: HashMap<String, NameEntry>,
}

/// Why the output cannot be read (a finding of rule O1, §6.1).
pub type ReadError = String;

fn u16_at(b: &[u8], o: usize) -> u16 {
    u16::from_le_bytes([b[o], b[o + 1]])
}
fn u32_at(b: &[u8], o: usize) -> u32 {
    u32::from_le_bytes([b[o], b[o + 1], b[o + 2], b[o + 3]])
}
fn u64_at(b: &[u8], o: usize) -> u64 {
    let mut a = [0u8; 8];
    a.copy_from_slice(&b[o..o + 8]);
    u64::from_le_bytes(a)
}

impl<'a> Image<'a> {
    /// Reads the output (rule O1, §6.1).
    pub fn read(data: &'a [u8]) -> Result<Image<'a>, ReadError> {
        let header = FileHeader::parse(data).map_err(|e| e.to_string())?;
        let phdrs = read_phdrs(data, &header)?;
        let table = SectionTable::parse(data, &header).map_err(|e| e.to_string())?;
        let mut sections = Vec::with_capacity(table.len() as usize);
        for s in table {
            let name = String::from_utf8_lossy(s.name().map_err(|e| e.to_string())?).into_owned();
            sections.push(OutSection { name, shdr: *s.header() });
        }
        let mut symbols = Vec::new();
        if let Some(symtab) = table.find_symbol_table(SHT_SYMTAB).map_err(|e| e.to_string())? {
            symbols.reserve(symtab.len() as usize);
            for (i, sym) in symtab.iter() {
                let name = String::from_utf8_lossy(symtab.name(&sym).map_err(|e| e.to_string())?)
                    .into_owned();
                symbols.push(OutSym { name, sym, index: i.0 });
            }
        }
        let mut names: HashMap<String, NameEntry> = HashMap::with_capacity_and_hasher(
            symbols.len(),
            std::hash::BuildHasherDefault::default(),
        );
        for (i, s) in symbols.iter().enumerate() {
            if s.name.is_empty() {
                continue;
            }
            let e = names.entry(s.name.clone()).or_default();
            e.count += 1;
            e.first.get_or_insert(i as u32);
            if matches!(s.sym.binding(), STB_GLOBAL | STB_WEAK) {
                e.global.get_or_insert(i as u32);
            }
        }
        let mut loads: Vec<usize> =
            phdrs.iter().enumerate().filter(|(_, p)| p.p_type == PT_LOAD).map(|(i, _)| i).collect();
        loads.sort_by_key(|&i| (phdrs[i].p_vaddr, i));
        Ok(Image { data, header, phdrs, sections, loads, symbols, names })
    }

    /// The file bytes of section `i`, when the file holds them.
    pub fn section_data(&self, i: usize) -> Option<&'a [u8]> {
        let s = self.sections.get(i)?;
        if s.shdr.sh_type == SHT_NOBITS {
            return None;
        }
        let o = usize::try_from(s.shdr.sh_offset).ok()?;
        let n = usize::try_from(s.shdr.sh_size).ok()?;
        self.data.get(o..o.checked_add(n)?)
    }

    /// The non-alloc output section named `name` (§5.8).
    pub fn nonalloc_section(&self, name: &str) -> Option<usize> {
        self.sections.iter().position(|s| s.name == name && !s.alloc())
    }

    /// `len` bytes at address `addr` of non-alloc section `i`, whose
    /// address is `sh_addr` (0 in an executable).
    pub fn nonalloc_bytes(&self, i: usize, addr: u64, len: u64) -> Option<&'a [u8]> {
        let d = self.section_data(i)?;
        let off = usize::try_from(addr.checked_sub(self.sections[i].shdr.sh_addr)?).ok()?;
        d.get(off..off.checked_add(usize::try_from(len).ok()?)?)
    }

    /// The little-endian value of `width` bytes there.
    pub fn nonalloc_value(&self, i: usize, addr: u64, width: u8) -> Option<u64> {
        let b = self.nonalloc_bytes(i, addr, u64::from(width))?;
        Some(b.iter().rev().fold(0u64, |a, &x| a << 8 | u64::from(x)))
    }

    pub fn class(&self) -> Class {
        self.header.class
    }

    /// The `STB_GLOBAL`/`STB_WEAK` symbol named `name`.
    pub fn global(&self, name: &str) -> Option<&OutSym> {
        self.names.get(name).and_then(|e| e.global).map(|i| &self.symbols[i as usize])
    }

    /// Whether a symbol of `.symtab` is named `name`, of any binding.
    pub fn has_name(&self, name: &str) -> bool {
        self.names.contains_key(name)
    }

    /// The one symbol named `name`, of any binding, when exactly one has it.
    pub fn unique(&self, name: &str) -> Option<&OutSym> {
        self.names
            .get(name)
            .filter(|e| e.count == 1)
            .and_then(|e| e.first)
            .map(|i| &self.symbols[i as usize])
    }

    /// Whether an allocated output section holds `addr` or ends there
    /// (an empty input section's place, WP-143).
    pub fn holds_or_ends(&self, addr: u64) -> bool {
        self.sections.iter().any(|s| {
            s.alloc() && s.shdr.sh_addr <= addr && addr <= s.shdr.sh_addr.saturating_add(s.shdr.sh_size)
        })
    }

    /// The `PT_LOAD` whose memory holds `[addr, addr + len)`.
    pub fn load_of(&self, addr: u64, len: u64) -> Option<&Phdr> {
        let end = addr.checked_add(len)?;
        let i = self.loads.partition_point(|&i| self.phdrs[i].p_vaddr <= addr);
        let p = &self.phdrs[*self.loads.get(i.checked_sub(1)?)?];
        (end <= p.p_vaddr.saturating_add(p.p_memsz)).then_some(p)
    }

    /// The bytes at `[addr, addr + len)` if they are in one segment's file
    /// image.
    pub fn bytes(&self, addr: u64, len: u64) -> Option<&'a [u8]> {
        let p = self.load_of(addr, len)?;
        let off = addr - p.p_vaddr;
        if off.checked_add(len)? > p.p_filesz {
            return None;
        }
        let start = usize::try_from(p.p_offset.checked_add(off)?).ok()?;
        let end = start.checked_add(usize::try_from(len).ok()?)?;
        self.data.get(start..end)
    }

    /// Whether `[addr, addr + len)` is mapped and reads as zero (in a
    /// segment's file image, or past it).
    pub fn is_zero(&self, addr: u64, len: u64) -> Option<bool> {
        let p = self.load_of(addr, len)?;
        let off = addr - p.p_vaddr;
        let in_file = p.p_filesz.saturating_sub(off).min(len);
        if in_file == 0 {
            return Some(true);
        }
        Some(self.bytes(addr, in_file)?.iter().all(|&b| b == 0))
    }

    /// A little-endian value of `width` bytes at `addr` as the loaded
    /// program sees it (zero past a segment's file image).
    pub fn value(&self, addr: u64, width: u8) -> Option<u64> {
        let w = u64::from(width);
        let p = self.load_of(addr, w)?;
        let off = addr - p.p_vaddr;
        if off >= p.p_filesz {
            return Some(0);
        }
        let b = self.bytes(addr, w)?;
        Some(match width {
            0 => 0,
            1 => u64::from(b[0]),
            2 => u64::from(u16_at(b, 0)),
            4 => u64::from(u32_at(b, 0)),
            8 => u64_at(b, 0),
            _ => return None,
        })
    }

    /// The instruction at `addr`, if it is in an executable segment.
    pub fn insn(&self, addr: u64) -> Option<u32> {
        let p = self.load_of(addr, 4)?;
        if p.p_flags & PF_X == 0 || !addr.is_multiple_of(4) {
            return None;
        }
        self.value(addr, 4).map(|v| v as u32)
    }

    /// The allocated output section with bytes or not that holds
    /// `[addr, addr + len)`, the first by index.
    pub fn alloc_section_of(&self, addr: u64, len: u64) -> Option<(usize, &OutSection)> {
        let end = addr.checked_add(len)?;
        self.sections.iter().enumerate().find(|(_, s)| {
            s.alloc()
                && s.shdr.sh_addr <= addr
                && end <= s.shdr.sh_addr.saturating_add(s.shdr.sh_size)
                && (len > 0 || s.shdr.sh_size > 0 || s.shdr.sh_addr == addr)
        })
    }
}

fn read_phdrs(data: &[u8], header: &FileHeader) -> Result<Vec<Phdr>, ReadError> {
    let num = usize::from(header.e_phnum);
    if num == 0 {
        return Ok(Vec::new());
    }
    let size = match header.class {
        Class::Elf32 => 32,
        Class::Elf64 => 56,
    };
    if usize::from(header.e_phentsize) != size {
        return Err(format!("e_phentsize is {}, expected {size}", header.e_phentsize));
    }
    let start = usize::try_from(header.e_phoff)
        .map_err(|_| "e_phoff is past the end of the file".to_string())?;
    let end = start
        .checked_add(num * size)
        .filter(|&e| e <= data.len())
        .ok_or_else(|| format!("the program header table (offset {start:#x}, {num} entries) is past the end of the file"))?;
    let table = &data[start..end];
    let mut out = Vec::with_capacity(num);
    for b in table.chunks_exact(size) {
        out.push(match header.class {
            Class::Elf64 => Phdr {
                p_type: u32_at(b, 0),
                p_flags: u32_at(b, 4),
                p_offset: u64_at(b, 8),
                p_vaddr: u64_at(b, 16),
                p_paddr: u64_at(b, 24),
                p_filesz: u64_at(b, 32),
                p_memsz: u64_at(b, 40),
                p_align: u64_at(b, 48),
            },
            Class::Elf32 => Phdr {
                p_type: u32_at(b, 0),
                p_offset: u64::from(u32_at(b, 4)),
                p_vaddr: u64::from(u32_at(b, 8)),
                p_paddr: u64::from(u32_at(b, 12)),
                p_filesz: u64::from(u32_at(b, 16)),
                p_memsz: u64::from(u32_at(b, 20)),
                p_flags: u32_at(b, 24),
                p_align: u64::from(u32_at(b, 28)),
            },
        });
    }
    Ok(out)
}

impl crate::a64::Memory for Image<'_> {
    fn insn(&self, addr: u64) -> Option<u32> {
        Image::insn(self, addr)
    }
    fn u64_at(&self, addr: u64) -> Option<u64> {
        self.value(addr, 8)
    }
}
