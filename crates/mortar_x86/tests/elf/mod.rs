//! A minimal reader of a linked ELF32 little-endian i386 image (or a
//! relocatable object, and the members of an `ar` archive), for the tests
//! that compare with reference linkers. Fields are widened to `u64` so the
//! harnesses read like x64's.
#![allow(dead_code)]

use std::collections::BTreeMap;
use std::path::Path;

use mortar_arch::TlsBlock;

pub fn u16_at(b: &[u8], o: usize) -> u16 {
    u16::from_le_bytes(b[o..o + 2].try_into().unwrap())
}
pub fn u32_at(b: &[u8], o: usize) -> u32 {
    u32::from_le_bytes(b[o..o + 4].try_into().unwrap())
}
/// A 32-bit word, widened.
pub fn w32_at(b: &[u8], o: usize) -> u64 {
    u64::from(u32_at(b, o))
}

#[derive(Clone)]
pub struct Shdr {
    pub name: String,
    pub kind: u32,
    pub flags: u64,
    pub addr: u64,
    pub offset: usize,
    pub size: usize,
    pub info: u32,
    pub link: u32,
}

#[derive(Clone)]
pub struct Sym {
    pub name: String,
    pub info: u8,
    pub shndx: u16,
    pub value: u64,
    pub size: u64,
}

/// One `Elf32_Rel` record: `r_offset`, type, symbol index.
#[derive(Clone, Copy, Debug)]
pub struct Rel {
    pub offset: u64,
    pub r_type: u32,
    pub sym: u32,
}

pub struct Image {
    pub bytes: Vec<u8>,
    pub sections: Vec<Shdr>,
    pub tls: Option<TlsBlock>,
    pub e_type: u16,
    pub e_flags: u32,
}

pub const SHT_SYMTAB: u32 = 2;
pub const SHT_RELA: u32 = 4;
pub const SHT_NOBITS: u32 = 8;
pub const SHT_REL: u32 = 9;
pub const SHF_ALLOC: u64 = 2;
pub const SHF_WRITE: u64 = 1;
pub const SHF_EXECINSTR: u64 = 4;
pub const SHF_TLS: u64 = 0x400;
pub const STB_WEAK: u8 = 2;
pub const PT_TLS: u32 = 7;
pub const STT_SECTION: u8 = 3;
pub const STT_TLS: u8 = 6;
pub const STT_GNU_IFUNC: u8 = 10;
pub const SHN_UNDEF: u16 = 0;
pub const SHN_ABS: u16 = 0xFFF1;
pub const SHN_COMMON: u16 = 0xFFF2;

/// Whether `bytes` start an ELFCLASS32 little-endian file.
pub fn is_elf32(bytes: &[u8]) -> bool {
    bytes.starts_with(b"\x7fELF\x01\x01")
}

impl Image {
    pub fn read(path: &Path) -> Self {
        let bytes = std::fs::read(path).unwrap();
        assert!(is_elf32(&bytes), "{}", path.display());
        Self::parse(bytes)
    }

    /// A linked image or a relocatable object (no program headers).
    pub fn parse(bytes: Vec<u8>) -> Self {
        assert_eq!(u16_at(&bytes, 18), 3, "EM_386");
        let e_type = u16_at(&bytes, 16);
        let e_flags = u32_at(&bytes, 0x24);
        let shoff = u32_at(&bytes, 0x20) as usize;
        let shnum = usize::from(u16_at(&bytes, 0x30));
        let shstrndx = usize::from(u16_at(&bytes, 0x32));
        let raw: Vec<[u32; 10]> = (0..shnum)
            .map(|i| {
                let o = shoff + 40 * i;
                std::array::from_fn(|k| u32_at(&bytes, o + 4 * k))
            })
            .collect();
        let strtab = raw[shstrndx][4] as usize;
        let name = |off: u32| {
            let s = &bytes[strtab + off as usize..];
            String::from_utf8_lossy(&s[..s.iter().position(|&b| b == 0).unwrap()]).into_owned()
        };
        // sh_name, sh_type, sh_flags, sh_addr, sh_offset, sh_size, sh_link,
        // sh_info, sh_addralign, sh_entsize.
        let sections = raw
            .iter()
            .map(|r| Shdr {
                name: name(r[0]),
                kind: r[1],
                flags: u64::from(r[2]),
                addr: u64::from(r[3]),
                offset: r[4] as usize,
                size: r[5] as usize,
                link: r[6],
                info: r[7],
            })
            .collect();
        let phoff = u32_at(&bytes, 0x1C) as usize;
        let phnum = usize::from(u16_at(&bytes, 0x2C));
        // p_type, p_offset, p_vaddr, p_paddr, p_filesz, p_memsz, p_flags,
        // p_align.
        let tls = (0..phnum).map(|i| phoff + 32 * i).find(|&o| u32_at(&bytes, o) == PT_TLS).map(
            |o| TlsBlock {
                start: w32_at(&bytes, o + 8),
                size: w32_at(&bytes, o + 0x14),
                align: w32_at(&bytes, o + 0x1C),
            },
        );
        Self { bytes, sections, tls, e_type, e_flags }
    }

    pub fn data(&self, s: &Shdr) -> &[u8] {
        if s.kind == SHT_NOBITS { &[] } else { &self.bytes[s.offset..s.offset + s.size] }
    }

    pub fn symbols(&self) -> Vec<Sym> {
        let Some(st) = self.sections.iter().find(|s| s.kind == SHT_SYMTAB) else {
            return Vec::new();
        };
        let names = self.data(&self.sections[st.link as usize]);
        // st_name, st_value, st_size, st_info, st_other, st_shndx.
        self.data(st)
            .chunks_exact(16)
            .map(|e| {
                let n = &names[u32_at(e, 0) as usize..];
                let name =
                    String::from_utf8_lossy(&n[..n.iter().position(|&b| b == 0).unwrap_or(0)])
                        .into_owned();
                Sym {
                    name,
                    info: e[12],
                    shndx: u16_at(e, 14),
                    value: w32_at(e, 4),
                    size: w32_at(e, 8),
                }
            })
            .collect()
    }

    /// The records of an `SHT_REL` section.
    pub fn rels(&self, s: &Shdr) -> Vec<Rel> {
        assert_eq!(s.kind, SHT_REL);
        self.data(s)
            .chunks_exact(8)
            .map(|e| {
                let info = u32_at(e, 4);
                Rel { offset: w32_at(e, 0), r_type: info & 0xff, sym: info >> 8 }
            })
            .collect()
    }

    /// Every 4-byte word of `.got` and `.got.plt`, by value.
    pub fn got_words(&self) -> BTreeMap<u64, Vec<u64>> {
        let mut out: BTreeMap<u64, Vec<u64>> = BTreeMap::new();
        for s in self.sections.iter().filter(|s| s.name == ".got" || s.name == ".got.plt") {
            for (i, w) in self.data(s).chunks_exact(4).enumerate() {
                out.entry(w32_at(w, 0)).or_default().push(s.addr + 4 * i as u64);
            }
        }
        out
    }

    /// The section with this name.
    pub fn section(&self, name: &str) -> Option<&Shdr> {
        self.sections.iter().find(|s| s.name == name)
    }
}

/// The ELF members of a (non-thin) `ar` archive, by name.
pub fn ar_members(bytes: &[u8]) -> Vec<(String, Vec<u8>)> {
    let mut out = Vec::new();
    if !bytes.starts_with(b"!<arch>\n") {
        return out;
    }
    let mut o = 8;
    while o + 60 <= bytes.len() {
        let h = &bytes[o..o + 60];
        let name = String::from_utf8_lossy(&h[..16]).trim_end().to_string();
        let size: usize = String::from_utf8_lossy(&h[48..58]).trim().parse().unwrap();
        let data = &bytes[o + 60..o + 60 + size];
        if is_elf32(data) {
            out.push((name, data.to_vec()));
        }
        o += 60 + size + (size & 1);
    }
    out
}
