//! A minimal reader of a linked ELF64 little-endian AArch64 image (or a
//! relocatable object, and the members of an `ar` archive), for the tests
//! that compare with reference linkers.
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
pub fn u64_at(b: &[u8], o: usize) -> u64 {
    u64::from_le_bytes(b[o..o + 8].try_into().unwrap())
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
}

pub struct Image {
    pub bytes: Vec<u8>,
    pub sections: Vec<Shdr>,
    pub tls: Option<TlsBlock>,
}

pub const SHT_SYMTAB: u32 = 2;
pub const SHT_RELA: u32 = 4;
pub const SHT_NOBITS: u32 = 8;
pub const SHF_ALLOC: u64 = 2;
pub const SHF_TLS: u64 = 0x400;
pub const PT_TLS: u32 = 7;
pub const STT_SECTION: u8 = 3;
pub const STT_TLS: u8 = 6;
pub const STT_GNU_IFUNC: u8 = 10;
pub const SHN_ABS: u16 = 0xFFF1;

impl Image {
    pub fn read(path: &Path) -> Self {
        let bytes = std::fs::read(path).unwrap();
        assert_eq!(&bytes[..5], b"\x7fELF\x02", "{}", path.display());
        Self::parse(bytes)
    }

    /// A linked image or a relocatable object (no program headers).
    pub fn parse(bytes: Vec<u8>) -> Self {
        assert_eq!(u16_at(&bytes, 18), 183);
        let shoff = usize::try_from(u64_at(&bytes, 0x28)).unwrap();
        let shnum = usize::from(u16_at(&bytes, 0x3C));
        let shstrndx = usize::from(u16_at(&bytes, 0x3E));
        let raw: Vec<[u64; 8]> = (0..shnum)
            .map(|i| {
                let o = shoff + 64 * i;
                [
                    u64::from(u32_at(&bytes, o)),
                    u64::from(u32_at(&bytes, o + 4)),
                    u64_at(&bytes, o + 8),
                    u64_at(&bytes, o + 0x10),
                    u64_at(&bytes, o + 0x18),
                    u64_at(&bytes, o + 0x20),
                    u64::from(u32_at(&bytes, o + 0x28)),
                    u64::from(u32_at(&bytes, o + 0x2C)),
                ]
            })
            .collect();
        let strtab = usize::try_from(raw[shstrndx][4]).unwrap();
        let name = |off: u64| {
            let s = &bytes[strtab + usize::try_from(off).unwrap()..];
            String::from_utf8_lossy(&s[..s.iter().position(|&b| b == 0).unwrap()]).into_owned()
        };
        let sections = raw
            .iter()
            .map(|r| Shdr {
                name: name(r[0]),
                kind: r[1] as u32,
                flags: r[2],
                addr: r[3],
                offset: r[4] as usize,
                size: r[5] as usize,
                info: r[7] as u32,
                link: r[6] as u32,
            })
            .collect();
        let phoff = usize::try_from(u64_at(&bytes, 0x20)).unwrap();
        let phnum = usize::from(u16_at(&bytes, 0x38));
        let tls =
            (0..phnum).map(|i| phoff + 56 * i).find(|&o| u32_at(&bytes, o) == PT_TLS).map(|o| {
                TlsBlock {
                    start: u64_at(&bytes, o + 0x10),
                    size: u64_at(&bytes, o + 0x28),
                    align: u64_at(&bytes, o + 0x30),
                }
            });
        Self { bytes, sections, tls }
    }

    pub fn data(&self, s: &Shdr) -> &[u8] {
        if s.kind == SHT_NOBITS { &[] } else { &self.bytes[s.offset..s.offset + s.size] }
    }

    pub fn symbols(&self) -> Vec<Sym> {
        let Some(st) = self.sections.iter().find(|s| s.kind == SHT_SYMTAB) else {
            return Vec::new();
        };
        let names = self.data(&self.sections[st.link as usize]);
        self.data(st)
            .chunks_exact(24)
            .map(|e| {
                let n = &names[u32_at(e, 0) as usize..];
                let name =
                    String::from_utf8_lossy(&n[..n.iter().position(|&b| b == 0).unwrap_or(0)])
                        .into_owned();
                Sym { name, info: e[4], shndx: u16_at(e, 6), value: u64_at(e, 8) }
            })
            .collect()
    }

    /// Every 8-byte word of `.got`, by value.
    pub fn got_words(&self) -> BTreeMap<u64, Vec<u64>> {
        let mut out: BTreeMap<u64, Vec<u64>> = BTreeMap::new();
        for s in self.sections.iter().filter(|s| s.name == ".got") {
            for (i, w) in self.data(s).chunks_exact(8).enumerate() {
                out.entry(u64_at(w, 0)).or_default().push(s.addr + 8 * i as u64);
            }
        }
        out
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
        if data.starts_with(b"\x7fELF\x02") {
            out.push((name, data.to_vec()));
        }
        o += 60 + size + (size & 1);
    }
    out
}
