//! The structure rules S1 to S11 (`spec/plumb.md` §6.2): what a loader and
//! a debugger rely on, checked from the output alone.

use mortar_elf::Machine;
use mortar_elf::consts::{
    ET_EXEC, PF_W, PF_X, PT_GNU_STACK, PT_LOAD, PT_NOTE, PT_TLS, SHF_ALLOC, SHF_EXECINSTR, SHF_TLS,
    SHF_WRITE, SHN_ABS, SHN_COMMON, SHN_LORESERVE, SHN_UNDEF, SHN_XINDEX, SHT_NOBITS, STT_SECTION,
    STT_TLS,
};

use crate::image::{Image, Phdr};
use crate::report::{Finding, Rule};

fn s(n: u8, msg: impl Into<String>) -> Finding {
    Finding::new(Rule::Structure(n), msg)
}

/// A `.tbss` section occupies no memory of its segment (gABI "Thread-Local
/// Storage").
fn occupies_memory(sh: &mortar_elf::Shdr) -> bool {
    sh.sh_flags & SHF_ALLOC != 0 && !(sh.sh_flags & SHF_TLS != 0 && sh.sh_type == SHT_NOBITS)
}

/// Checks S1 to S11 (§6.2).
#[allow(clippy::too_many_lines)] // one block per rule, in the spec's order
pub(crate) fn check(image: &Image<'_>) -> Vec<Finding> {
    let mut out = Vec::new();
    let h = &image.header;
    // S1: an executable of a supported machine.
    if h.e_type != ET_EXEC {
        out.push(s(
            1,
            format!("e_type is {}, not ET_EXEC (stage 1 checks static executables)", h.e_type),
        ));
    }
    if Machine::of(h).is_err() {
        out.push(s(
            1,
            format!("e_machine {} in this class is not a supported machine", h.e_machine),
        ));
    }
    let loads: Vec<&Phdr> = image.phdrs.iter().filter(|p| p.p_type == PT_LOAD).collect();
    if loads.is_empty() {
        out.push(s(2, "there is no PT_LOAD segment"));
    }
    // S2: each PT_LOAD is congruent and well-formed.
    for p in &loads {
        if p.p_filesz > p.p_memsz {
            out.push(
                s(
                    2,
                    format!(
                        "PT_LOAD at {:#x}: p_filesz {:#x} > p_memsz {:#x}",
                        p.p_vaddr, p.p_filesz, p.p_memsz
                    ),
                )
                .at(p.p_vaddr),
            );
        }
        if p.p_align > 1 {
            if !p.p_align.is_power_of_two() {
                out.push(
                    s(
                        2,
                        format!(
                            "PT_LOAD at {:#x}: p_align {:#x} is not a power of two",
                            p.p_vaddr, p.p_align
                        ),
                    )
                    .at(p.p_vaddr),
                );
            } else if p.p_vaddr % p.p_align != p.p_offset % p.p_align {
                out.push(
                    s(2, format!(
                        "PT_LOAD at {:#x}: p_vaddr and p_offset {:#x} differ modulo p_align {:#x}",
                        p.p_vaddr, p.p_offset, p.p_align
                    ))
                    .at(p.p_vaddr),
                );
            }
        }
        if p.p_offset.checked_add(p.p_filesz).is_none_or(|e| e > image.data.len() as u64) {
            out.push(
                s(
                    2,
                    format!(
                        "PT_LOAD at {:#x}: its file image is past the end of the file",
                        p.p_vaddr
                    ),
                )
                .at(p.p_vaddr),
            );
        }
        if p.p_vaddr.checked_add(p.p_memsz).is_none() {
            out.push(
                s(2, format!("PT_LOAD at {:#x}: its memory wraps the address space", p.p_vaddr))
                    .at(p.p_vaddr),
            );
        }
    }
    // S3: in ascending order of p_vaddr, without overlap.
    for w in loads.windows(2) {
        if w[1].p_vaddr < w[0].p_vaddr {
            out.push(
                s(3, format!("PT_LOAD at {:#x} follows one at {:#x}", w[1].p_vaddr, w[0].p_vaddr))
                    .at(w[1].p_vaddr),
            );
        } else if w[0].p_vaddr.saturating_add(w[0].p_memsz) > w[1].p_vaddr {
            out.push(
                s(
                    3,
                    format!(
                        "PT_LOAD at {:#x} overlaps the one at {:#x}",
                        w[1].p_vaddr, w[0].p_vaddr
                    ),
                )
                .at(w[1].p_vaddr),
            );
        }
    }
    // S4, S5: every allocated section is in one PT_LOAD, at the matching
    // file offset, with the segment's permissions.
    for sec in image.sections.iter().skip(1) {
        let sh = &sec.shdr;
        if !occupies_memory(sh) || sh.sh_size == 0 {
            continue;
        }
        let addr = sh.sh_addr;
        let Some(p) = image.load_of(addr, sh.sh_size) else {
            out.push(
                s(
                    4,
                    format!(
                        "section '{}' [{addr:#x}, +{:#x}) is in no PT_LOAD",
                        sec.name, sh.sh_size
                    ),
                )
                .at(addr),
            );
            continue;
        };
        if sh.sh_type != SHT_NOBITS {
            let off = addr - p.p_vaddr;
            if off.saturating_add(sh.sh_size) > p.p_filesz {
                out.push(
                    s(4, format!("section '{}' has bytes past its PT_LOAD's file image", sec.name))
                        .at(addr),
                );
            } else if p.p_offset.wrapping_add(off) != sh.sh_offset {
                out.push(
                    s(
                        4,
                        format!(
                            "section '{}' is at file offset {:#x}, its PT_LOAD maps {:#x} there",
                            sec.name,
                            sh.sh_offset,
                            p.p_offset.wrapping_add(off)
                        ),
                    )
                    .at(addr),
                );
            }
        }
        if sh.sh_flags & SHF_WRITE != 0 && p.p_flags & PF_W == 0 {
            out.push(
                s(5, format!("section '{}' is writable, its PT_LOAD is not", sec.name)).at(addr),
            );
        }
        if sh.sh_flags & SHF_EXECINSTR != 0 && p.p_flags & PF_X == 0 {
            out.push(
                s(5, format!("section '{}' is executable, its PT_LOAD is not", sec.name)).at(addr),
            );
        }
    }
    // S6: no two sections share memory or file bytes; none shares file
    // bytes with the headers.
    let mut mem: Vec<(u64, u64, usize)> = Vec::new();
    let mut file: Vec<(u64, u64, String)> = Vec::new();
    for (i, sec) in image.sections.iter().enumerate().skip(1) {
        let sh = &sec.shdr;
        if sh.sh_size == 0 {
            continue;
        }
        if occupies_memory(sh) {
            mem.push((sh.sh_addr, sh.sh_addr.saturating_add(sh.sh_size), i));
        }
        if sh.sh_type != SHT_NOBITS {
            file.push((
                sh.sh_offset,
                sh.sh_offset.saturating_add(sh.sh_size),
                format!("section '{}'", sec.name),
            ));
        }
    }
    let ehsize = match image.class() {
        mortar_elf::Class::Elf32 => 52,
        mortar_elf::Class::Elf64 => 64,
    };
    file.push((0, ehsize, "the file header".into()));
    if h.e_phnum > 0 {
        let len = u64::from(h.e_phnum) * u64::from(h.e_phentsize);
        file.push((h.e_phoff, h.e_phoff.saturating_add(len), "the program header table".into()));
    }
    if h.e_shnum > 0 {
        let len = u64::from(h.e_shnum) * u64::from(h.e_shentsize);
        file.push((h.e_shoff, h.e_shoff.saturating_add(len), "the section header table".into()));
    }
    mem.sort_unstable();
    for w in mem.windows(2) {
        if w[0].1 > w[1].0 {
            let (a, b) = (&image.sections[w[0].2].name, &image.sections[w[1].2].name);
            out.push(s(6, format!("sections '{a}' and '{b}' overlap in memory")).at(w[1].0));
        }
    }
    file.sort();
    let mut end: Option<(u64, &str)> = None;
    for (start, stop, what) in &file {
        if let Some((e, prev)) = end
            && e > *start
        {
            out.push(s(6, format!("{prev} and {what} overlap in the file at offset {start:#x}")));
        }
        if end.is_none_or(|(e, _)| *stop > e) {
            end = Some((*stop, what));
        }
    }
    // S7: the entry point is in an executable segment.
    if h.e_type == ET_EXEC {
        match image.load_of(h.e_entry, 4) {
            Some(p) if p.p_flags & PF_X != 0 => {}
            _ => out.push(
                s(7, format!("the entry point {:#x} is in no executable PT_LOAD", h.e_entry))
                    .at(h.e_entry),
            ),
        }
    }
    let tls: Vec<&Phdr> = image.phdrs.iter().filter(|p| p.p_type == PT_TLS).collect();
    // S8: symbol values are inside their sections.
    for sym in &image.symbols {
        let st = &sym.sym;
        let shndx = st.st_shndx;
        if sym.index == 0
            || matches!(shndx, SHN_UNDEF | SHN_ABS | SHN_COMMON | SHN_XINDEX)
            || shndx >= SHN_LORESERVE
        {
            continue;
        }
        let Some(sec) = image.sections.get(usize::from(shndx)) else {
            out.push(s(
                8,
                format!("symbol '{}' names section {shndx}, which does not exist", sym.name),
            ));
            continue;
        };
        if st.sym_type() == STT_TLS {
            let Some(t) = tls.first() else {
                let mut f = s(8, format!("TLS symbol '{}' and no PT_TLS", sym.name));
                f.symbol = Some(sym.name.clone());
                out.push(f);
                continue;
            };
            if st.st_value.saturating_add(st.st_size) > t.p_memsz {
                let mut f = s(
                    8,
                    format!(
                        "TLS symbol '{}' at offset {:#x} is outside PT_TLS",
                        sym.name, st.st_value
                    ),
                );
                f.symbol = Some(sym.name.clone());
                out.push(f);
            }
            continue;
        }
        // A symbol with an extent lies inside its section. A label (size
        // 0) is checked against its definition instead (Y1).
        if sec.shdr.sh_flags & SHF_ALLOC == 0 || st.sym_type() == STT_SECTION || st.st_size == 0 {
            continue;
        }
        let (lo, hi) = (sec.shdr.sh_addr, sec.shdr.sh_addr.saturating_add(sec.shdr.sh_size));
        if st.st_value < lo || st.st_value.saturating_add(st.st_size) > hi {
            let mut f = s(
                8,
                format!(
                    "symbol '{}' [{:#x}, +{:#x}) is outside its section '{}' [{lo:#x}, {hi:#x}]",
                    sym.name, st.st_value, st.st_size, sec.name
                ),
            )
            .at(st.st_value);
            f.symbol = Some(sym.name.clone());
            out.push(f);
        }
    }
    // S9: at most one PT_TLS, inside a PT_LOAD, holding every TLS section.
    if tls.len() > 1 {
        out.push(s(9, format!("{} PT_TLS segments", tls.len())));
    }
    if let Some(t) = tls.first() {
        match image.load_of(t.p_vaddr, t.p_filesz) {
            Some(p) if t.p_vaddr - p.p_vaddr + t.p_filesz <= p.p_filesz => {}
            _ => out.push(
                s(9, "the PT_TLS initialization image is in no PT_LOAD's file image").at(t.p_vaddr),
            ),
        }
        if t.p_align > 1 && !t.p_align.is_power_of_two() {
            out.push(
                s(9, format!("PT_TLS p_align {:#x} is not a power of two", t.p_align))
                    .at(t.p_vaddr),
            );
        } else if t.p_align > 1 && !t.p_vaddr.is_multiple_of(t.p_align) {
            // The TP offsets of §4.7 assume an aligned block.
            out.push(
                s(9, format!("PT_TLS p_vaddr is not a multiple of p_align {:#x}", t.p_align))
                    .at(t.p_vaddr),
            );
        }
        for sec in &image.sections {
            let sh = &sec.shdr;
            if sh.sh_flags & SHF_TLS != 0
                && sh.sh_size > 0
                && (sh.sh_addr < t.p_vaddr
                    || sh.sh_addr.saturating_add(sh.sh_size) > t.p_vaddr.saturating_add(t.p_memsz))
            {
                out.push(
                    s(9, format!("TLS section '{}' is outside PT_TLS", sec.name)).at(sh.sh_addr),
                );
            }
        }
    } else if image.sections.iter().any(|x| x.shdr.sh_flags & SHF_TLS != 0 && x.shdr.sh_size > 0) {
        out.push(s(9, "TLS sections and no PT_TLS"));
    }
    // S10: at most one PT_GNU_STACK.
    let stacks = image.phdrs.iter().filter(|p| p.p_type == PT_GNU_STACK).count();
    if stacks > 1 {
        out.push(s(10, format!("{stacks} PT_GNU_STACK segments")));
    }
    // S11: each PT_NOTE is in the file and is a sequence of whole notes.
    for p in image.phdrs.iter().filter(|p| p.p_type == PT_NOTE) {
        let bytes = usize::try_from(p.p_offset)
            .ok()
            .zip(usize::try_from(p.p_filesz).ok())
            .and_then(|(o, n)| image.data.get(o..o.checked_add(n)?));
        let Some(bytes) = bytes else {
            out.push(
                s(11, format!("PT_NOTE at offset {:#x} is past the end of the file", p.p_offset))
                    .at(p.p_vaddr),
            );
            continue;
        };
        let align: usize = if p.p_align == 8 { 8 } else { 4 };
        if !notes_ok(bytes, align) {
            out.push(
                s(11, format!("PT_NOTE at {:#x} is not a sequence of whole notes", p.p_vaddr))
                    .at(p.p_vaddr),
            );
        }
    }
    out
}

/// Whether `b` is a sequence of whole notes (gABI "Note Section").
fn notes_ok(b: &[u8], align: usize) -> bool {
    let up = |x: usize| x.div_ceil(align).checked_mul(align);
    let mut at = 0usize;
    while at < b.len() {
        let Some(h) = b.get(at..at + 12) else { return false };
        let namesz = u32::from_le_bytes([h[0], h[1], h[2], h[3]]) as usize;
        let descsz = u32::from_le_bytes([h[4], h[5], h[6], h[7]]) as usize;
        let Some(next) = up(namesz)
            .and_then(|n| n.checked_add(up(descsz)?))
            .and_then(|n| n.checked_add(12))
            .and_then(|n| at.checked_add(n))
        else {
            return false;
        };
        if next > b.len() {
            return false;
        }
        at = next;
    }
    true
}
