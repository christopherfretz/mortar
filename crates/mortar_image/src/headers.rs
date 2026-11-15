//! The structural units (I-40): the ELF header, the program header table
//! and the section header table, encoded little-endian in either class.
//! `mortar_elf` has the decoders; this is the writer half of
//! `spec/elf.md` §3 for these three records.

use mortar_elf::Class;

/// The ELF header's fields that vary (`e_ident` is fixed: `ELFDATA2LSB`,
/// `EV_CURRENT`, `ELFOSABI_NONE`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub struct Ehdr {
    pub e_type: u16,
    pub e_machine: u16,
    pub e_entry: u64,
    pub e_phoff: u64,
    pub e_shoff: u64,
    pub e_flags: u32,
    pub e_phnum: u16,
    pub e_shnum: u16,
    pub e_shstrndx: u16,
}

/// One program header.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
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

/// One section header.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
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

/// `(Ehdr, Phdr, Shdr)` sizes of a class.
#[must_use]
pub const fn sizes(class: Class) -> (u64, u64, u64) {
    match class {
        Class::Elf32 => (52, 32, 40),
        Class::Elf64 => (64, 56, 64),
    }
}

struct W<'a> {
    out: &'a mut Vec<u8>,
    is_64: bool,
}

impl W<'_> {
    fn u16(&mut self, v: u16) {
        self.out.extend_from_slice(&v.to_le_bytes());
    }
    fn u32(&mut self, v: u32) {
        self.out.extend_from_slice(&v.to_le_bytes());
    }
    /// An address-sized word (`Elf32_Addr`/`Off`/`Word` flags, or 64-bit).
    fn word(&mut self, v: u64) {
        if self.is_64 {
            self.out.extend_from_slice(&v.to_le_bytes());
        } else {
            self.out.extend_from_slice(&(v as u32).to_le_bytes());
        }
    }
}

/// The ELF header's bytes.
#[must_use]
pub fn ehdr(class: Class, h: &Ehdr) -> Vec<u8> {
    let is_64 = matches!(class, Class::Elf64);
    let (eh, ph, sh) = sizes(class);
    let mut out = Vec::with_capacity(eh as usize);
    out.extend_from_slice(&[0x7f, b'E', b'L', b'F', if is_64 { 2 } else { 1 }, 1, 1, 0]);
    out.extend_from_slice(&[0; 8]);
    let mut w = W { out: &mut out, is_64 };
    w.u16(h.e_type);
    w.u16(h.e_machine);
    w.u32(1);
    w.word(h.e_entry);
    w.word(h.e_phoff);
    w.word(h.e_shoff);
    w.u32(h.e_flags);
    w.u16(eh as u16);
    w.u16(if h.e_phnum == 0 { 0 } else { ph as u16 });
    w.u16(h.e_phnum);
    w.u16(sh as u16);
    w.u16(h.e_shnum);
    w.u16(h.e_shstrndx);
    out
}

/// The program header table's bytes.
#[must_use]
pub fn phdrs(class: Class, rows: &[Phdr]) -> Vec<u8> {
    let is_64 = matches!(class, Class::Elf64);
    let mut out = Vec::with_capacity(rows.len() * sizes(class).1 as usize);
    let mut w = W { out: &mut out, is_64 };
    for p in rows {
        w.u32(p.p_type);
        if is_64 {
            w.u32(p.p_flags);
        }
        w.word(p.p_offset);
        w.word(p.p_vaddr);
        w.word(p.p_paddr);
        w.word(p.p_filesz);
        w.word(p.p_memsz);
        if !is_64 {
            w.u32(p.p_flags);
        }
        w.word(p.p_align);
    }
    out
}

/// The section header table's bytes.
#[must_use]
pub fn shdrs(class: Class, rows: &[Shdr]) -> Vec<u8> {
    let is_64 = matches!(class, Class::Elf64);
    let mut out = Vec::with_capacity(rows.len() * sizes(class).2 as usize);
    let mut w = W { out: &mut out, is_64 };
    for s in rows {
        w.u32(s.sh_name);
        w.u32(s.sh_type);
        w.word(s.sh_flags);
        w.word(s.sh_addr);
        w.word(s.sh_offset);
        w.word(s.sh_size);
        w.u32(s.sh_link);
        w.u32(s.sh_info);
        w.word(s.sh_addralign);
        w.word(s.sh_entsize);
    }
    out
}

/// `.shstrtab`'s bytes and each name's offset (`spec/synth.md` S-90): a
/// zero byte, then each name in section-index order, a repeated name
/// written once.
#[must_use]
pub fn shstrtab(names: &[&[u8]]) -> (Vec<u8>, Vec<u32>) {
    let mut out = vec![0u8];
    let mut seen: std::collections::BTreeMap<&[u8], u32> = std::collections::BTreeMap::new();
    let mut offs = Vec::with_capacity(names.len());
    for &n in names {
        if n.is_empty() {
            offs.push(0);
            continue;
        }
        let off = *seen.entry(n).or_insert_with(|| {
            let off = out.len() as u32;
            out.extend_from_slice(n);
            out.push(0);
            off
        });
        offs.push(off);
    }
    (out, offs)
}
