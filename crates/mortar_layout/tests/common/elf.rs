//! A minimal ELF reader and writer for the tests, independent of
//! `mortar_elf` and of mortar's own layout code: it reads outputs of the
//! reference linkers, and writes a placement as a skeleton file (file
//! header, program headers, section headers, `.shstrtab`; contents zero)
//! for the [G] checker and the page simulator.
#![allow(dead_code, clippy::cast_possible_truncation, clippy::too_many_lines)]

use mortar_core::assignment::Assignment;
use mortar_core::layout::{ChunkKind, Placement};
use mortar_elf::Machine;

/// The parts of an ELF file the tests read.
#[derive(Debug, Clone)]
pub struct Elf {
    pub class64: bool,
    pub ehsize: u64,
    pub phoff: u64,
    pub phentsize: u64,
    pub phnum: u64,
    pub shentsize: u64,
    pub shnum: u64,
    pub shoff: u64,
    pub sections: Vec<Shdr>,
    pub segments: Vec<Phdr>,
}

#[derive(Debug, Clone)]
pub struct Shdr {
    pub name: String,
    pub ty: u32,
    pub flags: u64,
    pub addr: u64,
    pub offset: u64,
    pub size: u64,
    pub align: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Phdr {
    pub ty: u32,
    pub flags: u32,
    pub offset: u64,
    pub vaddr: u64,
    pub filesz: u64,
    pub memsz: u64,
    pub align: u64,
}

pub const PT_LOAD: u32 = 1;
pub const PT_PHDR: u32 = 6;
pub const PT_TLS: u32 = 7;
pub const PT_GNU_STACK: u32 = 0x6474_e551;
pub const PT_GNU_RELRO: u32 = 0x6474_e552;
pub const PT_GNU_PROPERTY: u32 = 0x6474_e553;
const SHF_WRITE: u64 = 1;
const SHF_ALLOC: u64 = 2;
const SHF_TLS: u64 = 0x400;
const SHT_NOBITS: u32 = 8;
const PF_W: u32 = 2;

fn le(b: &[u8], at: u64, n: usize) -> u64 {
    let at = at as usize;
    b[at..at + n].iter().rev().fold(0, |v, &x| (v << 8) | u64::from(x))
}

/// Reads the headers of an ELF file (little-endian).
pub fn read_elf(b: &[u8]) -> Elf {
    let c64 = b[4] == 2;
    let (phoff, shoff) =
        if c64 { (le(b, 32, 8), le(b, 40, 8)) } else { (le(b, 28, 4), le(b, 32, 4)) };
    let base = if c64 { 52 } else { 40 };
    let ehsize = le(b, base, 2);
    let phentsize = le(b, base + 2, 2);
    let phnum = le(b, base + 4, 2);
    let shentsize = le(b, base + 6, 2);
    let shnum = le(b, base + 8, 2);
    let shstrndx = le(b, base + 10, 2);
    let mut raw = Vec::new();
    for i in 0..shnum {
        let o = shoff + i * shentsize;
        let f = |k: u64, n: usize| le(b, o + k, n);
        raw.push(if c64 {
            (f(0, 4), f(4, 4) as u32, f(8, 8), f(16, 8), f(24, 8), f(32, 8), f(48, 8))
        } else {
            (f(0, 4), f(4, 4) as u32, f(8, 4), f(12, 4), f(16, 4), f(20, 4), f(32, 4))
        });
    }
    let stroff = raw.get(shstrndx as usize).map_or(0, |r| r.4);
    let name_at = |n: u64| {
        let s = &b[(stroff + n) as usize..];
        let end = s.iter().position(|&c| c == 0).unwrap_or(0);
        String::from_utf8_lossy(&s[..end]).into_owned()
    };
    let sections = raw
        .iter()
        .map(|&(n, ty, flags, addr, offset, size, align)| Shdr {
            name: name_at(n),
            ty,
            flags,
            addr,
            offset,
            size,
            align,
        })
        .collect();
    let mut segments = Vec::new();
    for i in 0..phnum {
        let o = phoff + i * phentsize;
        let f = |k: u64, n: usize| le(b, o + k, n);
        segments.push(if c64 {
            Phdr {
                ty: f(0, 4) as u32,
                flags: f(4, 4) as u32,
                offset: f(8, 8),
                vaddr: f(16, 8),
                filesz: f(32, 8),
                memsz: f(40, 8),
                align: f(48, 8),
            }
        } else {
            Phdr {
                ty: f(0, 4) as u32,
                offset: f(4, 4),
                vaddr: f(8, 4),
                filesz: f(16, 4),
                memsz: f(20, 4),
                flags: f(24, 4) as u32,
                align: f(28, 4),
            }
        });
    }
    Elf {
        class64: c64,
        ehsize,
        phoff,
        phentsize,
        phnum,
        shentsize,
        shnum,
        shoff,
        sections,
        segments,
    }
}

fn put(out: &mut Vec<u8>, at: usize, v: u64, n: usize) {
    if out.len() < at + n {
        out.resize(at + n, 0);
    }
    for i in 0..n {
        out[at + i] = (v >> (8 * i)) as u8;
    }
}

/// A placement as an ELF skeleton: the headers where the placement puts
/// them, one section header per output section index (its chunks
/// combined), `.shstrtab` and the section header table after the last
/// byte any chunk occupies. Contents are zero.
pub fn write_skeleton(
    placement: &Placement,
    assignment: &Assignment<'_>,
    machine: Machine,
    class64: bool,
) -> Vec<u8> {
    // The output sections by header index.
    let mut secs: std::collections::BTreeMap<u32, Shdr> = std::collections::BTreeMap::new();
    let mut file_end = 0u64;
    let mut phoff = 0u64;
    for &c in &placement.order {
        let ch = &placement.chunks[c];
        let end = ch.offset + if ch.in_file() { ch.size } else { 0 };
        if !matches!(ch.kind, ChunkKind::SectionHeaders) {
            file_end = file_end.max(end);
        }
        if matches!(ch.kind, ChunkKind::ProgramHeaders) {
            phoff = ch.offset;
        }
        let Some(ndx) = ch.shndx else { continue };
        let name = ch.name(assignment).map(|n| String::from_utf8_lossy(n).into_owned());
        let addr = ch.addr.unwrap_or(0);
        let e = secs.entry(ndx).or_insert_with(|| Shdr {
            name: name.unwrap_or_default(),
            ty: ch.sh_type,
            flags: ch.flags,
            addr,
            offset: ch.offset,
            size: 0,
            align: 1,
        });
        e.align = e.align.max(1 << ch.align);
        let start = if ch.is_alloc() { e.addr } else { e.offset };
        let here = if ch.is_alloc() { addr } else { ch.offset };
        e.size = e.size.max(here + ch.size - start);
    }
    let mut out = vec![0u8; file_end as usize];
    // .shstrtab and the table.
    let mut strtab = vec![0u8];
    let mut names = Vec::new();
    for s in secs.values() {
        names.push(strtab.len() as u64);
        strtab.extend(s.name.as_bytes());
        strtab.push(0);
    }
    let shstr_name = strtab.len() as u64;
    strtab.extend(b".shstrtab\0");
    let str_off = file_end as usize;
    out.extend(&strtab);
    let max_ndx = secs.keys().max().copied().unwrap_or(0);
    let shnum = u64::from(max_ndx) + 2;
    let shentsize: u64 = if class64 { 64 } else { 40 };
    let shoff = (out.len() as u64).div_ceil(8) * 8;
    out.resize((shoff + shnum * shentsize) as usize, 0);
    let w = if class64 { 8 } else { 4 };
    let mut sh = |i: u64, name: u64, s: &Shdr| {
        let o = (shoff + i * shentsize) as usize;
        put(&mut out, o, name, 4);
        put(&mut out, o + 4, u64::from(s.ty), 4);
        put(&mut out, o + 8, s.flags, w);
        put(&mut out, o + 8 + w, s.addr, w);
        put(&mut out, o + 8 + 2 * w, s.offset, w);
        put(&mut out, o + 8 + 3 * w, s.size, w);
        put(&mut out, o + 8 + 4 * w + 8, s.align, w);
    };
    for (k, (ndx, s)) in secs.iter().enumerate() {
        sh(u64::from(*ndx), names[k], s);
    }
    let strsec = Shdr {
        name: String::new(),
        ty: 3,
        flags: 0,
        addr: 0,
        offset: str_off as u64,
        size: strtab.len() as u64,
        align: 1,
    };
    sh(shnum - 1, shstr_name, &strsec);
    // The program headers.
    let phentsize: u64 = if class64 { 56 } else { 32 };
    for (i, (_, s)) in placement.segments.iter().enumerate() {
        let o = (phoff + i as u64 * phentsize) as usize;
        let align = 1u64 << s.align;
        if class64 {
            put(&mut out, o, u64::from(s.p_type), 4);
            put(&mut out, o + 4, u64::from(s.flags), 4);
            put(&mut out, o + 8, s.offset, 8);
            put(&mut out, o + 16, s.addr, 8);
            put(&mut out, o + 24, s.addr, 8);
            put(&mut out, o + 32, s.filesz, 8);
            put(&mut out, o + 40, s.memsz, 8);
            put(&mut out, o + 48, align, 8);
        } else {
            put(&mut out, o, u64::from(s.p_type), 4);
            put(&mut out, o + 4, s.offset, 4);
            put(&mut out, o + 8, s.addr, 4);
            put(&mut out, o + 12, s.addr, 4);
            put(&mut out, o + 16, s.filesz, 4);
            put(&mut out, o + 20, s.memsz, 4);
            put(&mut out, o + 24, u64::from(s.flags), 4);
            put(&mut out, o + 28, align, 4);
        }
    }
    // The file header.
    out[..4].copy_from_slice(b"\x7fELF");
    out[4] = if class64 { 2 } else { 1 };
    out[5] = 1;
    out[6] = 1;
    let em = match machine {
        Machine::AArch64 => 183,
        Machine::X86_64 => 62,
        Machine::I386 => 3,
    };
    put(&mut out, 16, 2, 2);
    put(&mut out, 18, em, 2);
    put(&mut out, 20, 1, 4);
    put(&mut out, 24, placement.entry.unwrap_or(0), w);
    put(&mut out, 24 + w, phoff, w);
    put(&mut out, 24 + 2 * w, shoff, w);
    let base = 24 + 3 * w + 4;
    put(&mut out, base, if class64 { 64 } else { 52 }, 2);
    put(&mut out, base + 2, phentsize, 2);
    put(&mut out, base + 4, u64::from(placement.segments.len()), 2);
    put(&mut out, base + 6, shentsize, 2);
    put(&mut out, base + 8, shnum, 2);
    put(&mut out, base + 10, shnum - 1, 2);
    out
}

fn down(x: u64, p: u64) -> u64 {
    x - x % p
}

/// The [G] rules of `spec/layout.md` §2-§6 that a file's headers show,
/// checked from the file alone (L-1 to L-5, L-7 to L-11, L-15 to L-18,
/// L-23), plus no overlap of allocated sections. The violations, by
/// rule id.
pub fn check_g(elf: &Elf) -> Vec<String> {
    let mut v = Vec::new();
    let loads: Vec<&Phdr> = elf.segments.iter().filter(|p| p.ty == PT_LOAD).collect();
    let first_load = elf.segments.iter().position(|p| p.ty == PT_LOAD).unwrap_or(usize::MAX);
    // L-1
    for (i, p) in loads.iter().enumerate() {
        if !p.align.is_power_of_two() || p.align < 0x1000 {
            v.push(format!("L-1: PT_LOAD {i} p_align {:#x}", p.align));
        } else if (p.vaddr % p.align) != (p.offset % p.align) {
            v.push(format!("L-1: PT_LOAD {i} vaddr {:#x} offset {:#x}", p.vaddr, p.offset));
        }
        if p.filesz > p.memsz {
            v.push(format!("L-1: PT_LOAD {i} filesz > memsz"));
        }
    }
    // Loads in ascending, disjoint address order.
    for w in loads.windows(2) {
        if w[0].vaddr + w[0].memsz > w[1].vaddr {
            v.push(format!("PT_LOADs overlap at {:#x}", w[1].vaddr));
        }
    }
    // L-2
    for (i, p) in elf.segments.iter().enumerate() {
        if p.ty == PT_PHDR && i > first_load {
            v.push("L-2: PT_PHDR after a PT_LOAD".into());
        }
    }
    if elf.segments.iter().filter(|p| p.ty == PT_PHDR).count() > 1 {
        v.push("L-2: two PT_PHDR".into());
    }
    let in_image = |off: u64, len: u64| {
        loads.iter().any(|p| off >= p.offset && off + len <= p.offset + p.filesz)
    };
    // L-3
    if !in_image(elf.phoff, elf.phnum * elf.phentsize) {
        v.push("L-3: the program header table is not in a PT_LOAD".into());
    }
    // L-15
    if !loads.iter().any(|p| p.offset == 0 && p.filesz >= elf.ehsize) {
        v.push("L-15: the file header is not in a PT_LOAD at offset 0".into());
    }
    let load_of = |addr: u64| loads.iter().find(|p| addr >= p.vaddr && addr < p.vaddr + p.memsz);
    let alloc: Vec<&Shdr> =
        elf.sections.iter().filter(|s| s.flags & SHF_ALLOC != 0 && s.size > 0).collect();
    for s in &alloc {
        let tbss = s.flags & SHF_TLS != 0 && s.ty == SHT_NOBITS;
        // L-5
        if s.align > 1 && s.addr % s.align != 0 {
            v.push(format!("L-5: {} at {:#x} align {:#x}", s.name, s.addr, s.align));
        }
        if tbss {
            continue;
        }
        let Some(p) = load_of(s.addr) else {
            v.push(format!("{} at {:#x} is in no PT_LOAD", s.name, s.addr));
            continue;
        };
        if s.addr + s.size > p.vaddr + p.memsz {
            v.push(format!("{} ends beyond its PT_LOAD", s.name));
        }
        if s.ty == SHT_NOBITS {
            // L-4
            if s.addr < p.vaddr + p.filesz {
                v.push(format!("L-4: {} at {:#x} inside p_filesz", s.name, s.addr));
            }
        } else if s.addr + s.size > p.vaddr + p.filesz || s.offset != p.offset + (s.addr - p.vaddr)
        {
            v.push(format!("{}: bytes not where its PT_LOAD maps them", s.name));
        }
    }
    // No overlap in memory (.tbss aside) nor in the file.
    let mut mem: Vec<&&Shdr> =
        alloc.iter().filter(|s| !(s.flags & SHF_TLS != 0 && s.ty == SHT_NOBITS)).collect();
    mem.sort_by_key(|s| s.addr);
    for w in mem.windows(2) {
        if w[0].addr + w[0].size > w[1].addr {
            v.push(format!("{} and {} overlap in memory", w[0].name, w[1].name));
        }
    }
    let mut file: Vec<&Shdr> =
        elf.sections.iter().filter(|s| s.ty != SHT_NOBITS && s.ty != 0 && s.size > 0).collect();
    file.sort_by_key(|s| s.offset);
    for w in file.windows(2) {
        if w[0].offset + w[0].size > w[1].offset {
            v.push(format!("{} and {} overlap in the file", w[0].name, w[1].name));
        }
    }
    // L-9: each PT_LOAD's memory ends at the end of a section (or of the
    // headers).
    for (i, p) in loads.iter().enumerate() {
        let end = p.vaddr + p.memsz;
        let headers = p.offset == 0 && p.vaddr + elf.phoff + elf.phnum * elf.phentsize == end;
        if !headers && !alloc.iter().any(|s| s.addr + s.size == end) {
            v.push(format!("L-9: PT_LOAD {i} ends at {end:#x}, not a section end"));
        }
    }
    // L-7, L-8
    let relros: Vec<&Phdr> = elf.segments.iter().filter(|p| p.ty == PT_GNU_RELRO).collect();
    if relros.len() > 1 {
        v.push("L-7: two PT_GNU_RELRO".into());
    }
    if let Some(r) = relros.first() {
        let (s, e) = (r.vaddr, r.vaddr + r.memsz);
        let mut at = s;
        while at < e {
            match load_of(at) {
                Some(p) if p.flags & PF_W != 0 => at = p.vaddr + p.memsz,
                _ => {
                    v.push(format!("L-7: RELRO byte {at:#x} not in a PF_W PT_LOAD"));
                    break;
                }
            }
        }
        let max_p = loads.iter().map(|p| p.align).min().unwrap_or(0x1000);
        for page in [0x1000u64, 0x4000, 0x1_0000] {
            if page > max_p {
                continue;
            }
            let (lo, hi) = (down(s, page), down(e, page));
            for x in &alloc {
                if x.flags & SHF_WRITE == 0 || (x.flags & SHF_TLS != 0 && x.ty == SHT_NOBITS) {
                    continue;
                }
                let inside = x.addr >= s && x.addr + x.size <= e;
                if !inside && x.addr < hi && x.addr + x.size > lo {
                    v.push(format!("L-8: P={page:#x}: {} made read-only by RELRO", x.name));
                }
            }
        }
    }
    // L-10, L-11, L-23
    let tlses: Vec<&Phdr> = elf.segments.iter().filter(|p| p.ty == PT_TLS).collect();
    if tlses.len() > 1 {
        v.push("L-10: two PT_TLS".into());
    }
    let tls_secs: Vec<&Shdr> = elf.sections.iter().filter(|s| s.flags & SHF_TLS != 0).collect();
    if let Some(t) = tlses.first() {
        if !t.align.is_power_of_two() || t.filesz > t.memsz {
            v.push("L-10: PT_TLS p_align or sizes".into());
        }
        if t.vaddr % t.align.max(1) != 0 {
            v.push(format!("L-23: PT_TLS at {:#x} align {:#x}", t.vaddr, t.align));
        }
        let mut seen_nobits = false;
        for s in &tls_secs {
            if s.align > t.align {
                v.push(format!("L-10: {} align above PT_TLS's", s.name));
            }
            if s.addr < t.vaddr || s.addr + s.size > t.vaddr + t.memsz {
                v.push(format!("L-10/L-11: {} outside PT_TLS", s.name));
            }
            if s.ty == SHT_NOBITS {
                seen_nobits = true;
            } else {
                if seen_nobits {
                    v.push("L-10: PROGBITS TLS after NOBITS TLS".into());
                }
                if s.addr + s.size > t.vaddr + t.filesz {
                    v.push(format!("L-11: {} beyond p_filesz", s.name));
                }
                if s.offset != t.offset + (s.addr - t.vaddr) {
                    v.push(format!("L-10: {} not in the TLS image", s.name));
                }
            }
        }
    } else if tls_secs.iter().any(|s| s.size > 0) {
        v.push("L-10: TLS sections and no PT_TLS".into());
    }
    // L-16
    if elf.segments.iter().filter(|p| p.ty == PT_GNU_STACK).count() > 1 {
        v.push("L-16: two PT_GNU_STACK".into());
    }
    // L-17
    for p in elf.segments.iter().filter(|p| p.ty == PT_GNU_PROPERTY) {
        let ok = elf
            .sections
            .iter()
            .any(|s| s.name == ".note.gnu.property" && s.addr == p.vaddr && s.size == p.memsz);
        if !ok {
            v.push("L-17: PT_GNU_PROPERTY is not .note.gnu.property".into());
        }
    }
    // L-18
    for s in &elf.sections {
        if s.flags & SHF_ALLOC == 0 && s.ty != SHT_NOBITS && s.ty != 0 && s.size > 0 {
            let overlap = loads.iter().any(|p| {
                p.filesz > 0 && s.offset < p.offset + p.filesz && s.offset + s.size > p.offset
            });
            if overlap {
                v.push(format!("L-18: {} in a PT_LOAD's file image", s.name));
            }
        }
    }
    v
}
