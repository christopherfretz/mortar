//! The structure rules S1 to S11 and S13 to S18 (`spec/plumb.md` §6.2): what a loader and
//! a debugger rely on, checked from the output alone.

use mortar_elf::Machine;
use mortar_elf::consts::{
    EM_386, EM_AARCH64, EM_X86_64, ET_EXEC, PF_R, PF_W, PF_X, PT_GNU_EH_FRAME, PT_GNU_PROPERTY,
    PT_GNU_RELRO, PT_GNU_STACK, PT_INTERP, PT_LOAD, PT_NOTE, PT_PHDR, PT_TLS, SHF_ALLOC,
    SHF_COMPRESSED, SHF_EXECINSTR, SHF_TLS, SHF_WRITE, SHN_ABS, SHN_COMMON, SHN_LORESERVE,
    SHN_UNDEF, SHN_XINDEX, SHT_DYNAMIC, SHT_DYNSYM, SHT_FINI_ARRAY, SHT_GNU_HASH, SHT_GNU_VERDEF,
    SHT_GNU_VERNEED, SHT_GNU_VERSYM, SHT_HASH, SHT_INIT_ARRAY, SHT_NOBITS, SHT_NOTE,
    SHT_PREINIT_ARRAY, SHT_PROGBITS, SHT_REL, SHT_RELA, SHT_STRTAB, SHT_SYMTAB, SHT_X86_64_UNWIND,
    STB_GNU_UNIQUE, STT_FILE, STT_GNU_IFUNC, STT_SECTION, STT_TLS,
};

use crate::image::{Image, Phdr};
use crate::map::Format;
use crate::report::{Finding, Rule};

/// AArch64's execute-only section flag (AAELF64 "Section flags").
const SHF_AARCH64_PURECODE: u64 = 0x2000_0000;

fn s(n: u8, msg: impl Into<String>) -> Finding {
    Finding::new(Rule::Structure(n), msg)
}

/// Whether the output has an `STT_GNU_IFUNC` or `STB_GNU_UNIQUE` symbol
/// in `.symtab` or `.dynsym` (dyn.md D-23), or an `IRELATIVE` record in an
/// allocated relocation section.
fn gnu_extension(image: &Image<'_>) -> bool {
    if image.symbols.iter().any(|x| {
        x.index != 0 && (x.sym.sym_type() == STT_GNU_IFUNC || x.sym.binding() == STB_GNU_UNIQUE)
    }) {
        return true;
    }
    // `.dynsym`: st_info is byte 4 of an ELFCLASS64 entry (24 bytes),
    // byte 12 of an ELFCLASS32 one (16 bytes).
    let dynsym = image.sections.iter().any(|x| {
        let sh = &x.shdr;
        let (size, at) = if image.header.e_machine == EM_386 { (16, 12) } else { (24, 4) };
        if sh.sh_type != SHT_DYNSYM {
            return false;
        }
        let (Ok(o), Ok(n)) = (usize::try_from(sh.sh_offset), usize::try_from(sh.sh_size)) else {
            return false;
        };
        image.data.get(o..o.saturating_add(n)).is_some_and(|b| {
            b.chunks_exact(size).skip(1).any(|e| {
                let info = e[at];
                info & 0xf == STT_GNU_IFUNC || info >> 4 == STB_GNU_UNIQUE
            })
        })
    });
    if dynsym {
        return true;
    }
    let (irelative, rela, size) = match image.header.e_machine {
        EM_386 => (42, false, 8),
        EM_X86_64 => (37, true, 24),
        _ => (1032, true, 24),
    };
    image.sections.iter().any(|x| {
        let sh = &x.shdr;
        let want = if rela { SHT_RELA } else { SHT_REL };
        if sh.sh_flags & SHF_ALLOC == 0 || sh.sh_type != want {
            return false;
        }
        let (Ok(o), Ok(n)) = (usize::try_from(sh.sh_offset), usize::try_from(sh.sh_size)) else {
            return false;
        };
        image.data.get(o..o.saturating_add(n)).is_some_and(|b| {
            b.chunks_exact(size).any(|e| {
                let info = if rela { &e[8..12] } else { &e[4..5] };
                let ty = info.iter().rev().fold(0u32, |a, &v| a << 8 | u32::from(v));
                ty == irelative
            })
        })
    })
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
    // Stage 3 (§12, P-D1) judges an `ET_DYN` with `PT_DYNAMIC`.
    let dynamic = image.phdrs.iter().any(|p| p.p_type == mortar_elf::consts::PT_DYNAMIC);
    if h.e_type != ET_EXEC && !(dynamic && h.e_type == mortar_elf::consts::ET_DYN) {
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
    // S1: the x86 psABIs define no e_flags (spec/x86.md X86-1, §2.4); a
    // Linux output's EI_OSABI is ELFOSABI_NONE or ELFOSABI_GNU.
    if matches!(h.e_machine, EM_386 | EM_X86_64) && h.e_flags != 0 {
        out.push(s(1, format!("e_flags is {:#x}; the psABI defines none", h.e_flags)));
    }
    if !matches!(h.ei_osabi, 0 | 3) {
        out.push(s(
            1,
            format!("EI_OSABI is {}, neither ELFOSABI_NONE nor ELFOSABI_GNU", h.ei_osabi),
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
        // An empty file image `[p_offset, p_offset)` lies in any file,
        // whatever `p_offset` (§6.2 S2; GNU ld's AArch64 `.bss`-only
        // segment, WP-059's F1).
        if p.p_filesz > 0
            && p.p_offset.checked_add(p.p_filesz).is_none_or(|e| e > image.data.len() as u64)
        {
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
        // ELFCLASS32: the address space ends at 2^32 (gABI "Data
        // Representation": Elf32_Addr).
        let top = if image.class() == mortar_elf::Class::Elf32 { 1u64 << 32 } else { u64::MAX };
        if p.p_vaddr.checked_add(p.p_memsz).is_none_or(|e| e > top) {
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
    // S5: a PT_LOAD holding an allocated section is readable (a text
    // segment without PF_R is execute-only where the hardware allows it,
    // and its literal pools fault; WP-028d), unless every section in it
    // asks to be execute-only: AArch64's SHF_AARCH64_PURECODE (AAELF64
    // "Section flags"; lld 22 makes its `.iplt` so).
    for p in &loads {
        let secs = sections_in(image, p);
        let pure =
            h.e_machine == 183 && secs.iter().all(|sh| sh.sh_flags & SHF_AARCH64_PURECODE != 0);
        if p.p_flags & PF_R == 0 && !secs.is_empty() && !pure {
            out.push(
                s(5, format!("PT_LOAD at {:#x} holds sections and is not PF_R", p.p_vaddr))
                    .at(p.p_vaddr),
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
    // S7: the entry point is in an executable segment, or is 0: the gABI's
    // "no associated entry point" (a link with no entry symbol warns and
    // writes 0, spec/layout.md L-103).
    // The instruction there: 4 bytes on AArch64, at least 1 on x86.
    if h.e_type == ET_EXEC && h.e_entry != 0 {
        let insn = if h.e_machine == 183 { 4 } else { 1 };
        match image.load_of(h.e_entry, insn) {
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
        // The block is exactly the TLS sections (gABI "Thread-Local
        // Storage": the initialization image is .tdata, the block .tdata
        // and .tbss; its alignment is theirs). lld rounds p_memsz up to
        // p_align (variant 2 places the TP after the block), so p_memsz may
        // be anywhere from their span to it rounded up (WP-028d).
        let secs: Vec<&mortar_elf::Shdr> = image
            .sections
            .iter()
            .map(|x| &x.shdr)
            .filter(|sh| {
                sh.sh_flags & SHF_TLS != 0 && sh.sh_flags & SHF_ALLOC != 0 && sh.sh_size > 0
            })
            .collect();
        if let (Some(lo), Some(hi)) = (
            secs.iter().map(|sh| sh.sh_addr).min(),
            secs.iter().map(|sh| sh.sh_addr.saturating_add(sh.sh_size)).max(),
        ) {
            let file_hi = secs
                .iter()
                .filter(|sh| sh.sh_type != SHT_NOBITS)
                .map(|sh| sh.sh_addr.saturating_add(sh.sh_size))
                .max()
                .unwrap_or(lo);
            let align = secs.iter().map(|sh| sh.sh_addralign.max(1)).max().unwrap_or(1);
            let span = hi - lo;
            let up = span.next_multiple_of(align.max(1));
            let mut bad = Vec::new();
            if t.p_vaddr != lo {
                bad.push(format!("p_vaddr {:#x}, the first TLS section is at {lo:#x}", t.p_vaddr));
            }
            if t.p_filesz != file_hi.saturating_sub(lo) {
                bad.push(format!(
                    "p_filesz {:#x}, the initialized TLS sections span {:#x}",
                    t.p_filesz,
                    file_hi.saturating_sub(lo)
                ));
            }
            if t.p_memsz < span || t.p_memsz > up {
                bad.push(format!("p_memsz {:#x}, the TLS sections span {span:#x}", t.p_memsz));
            }
            if t.p_align.max(1) != align {
                bad.push(format!(
                    "p_align {:#x}, the TLS sections' alignment is {align:#x}",
                    t.p_align
                ));
            }
            for b in bad {
                out.push(s(9, format!("PT_TLS: {b}")).at(t.p_vaddr));
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
        // The segment is whole note sections, aligned as they are: a
        // reader takes the padding from p_align (glibc ignores a property
        // note under a p_align other than 8), and the memory image is the
        // file image (WP-028d).
        let (lo, hi) = (p.p_vaddr, p.p_vaddr.saturating_add(p.p_filesz));
        let notes: Vec<&mortar_elf::Shdr> = image
            .sections
            .iter()
            .map(|x| &x.shdr)
            .filter(|sh| {
                sh.sh_type == SHT_NOTE
                    && sh.sh_flags & SHF_ALLOC != 0
                    && sh.sh_size > 0
                    && sh.sh_addr >= lo
                    && sh.sh_addr < hi
            })
            .collect();
        if p.p_memsz != p.p_filesz {
            out.push(
                s(
                    11,
                    format!(
                        "PT_NOTE at {:#x}: p_memsz {:#x} is not p_filesz {:#x}",
                        p.p_vaddr, p.p_memsz, p.p_filesz
                    ),
                )
                .at(p.p_vaddr),
            );
        }
        if !notes.is_empty() {
            let want = notes.iter().map(|sh| sh.sh_addralign.max(1)).max().unwrap_or(1).max(4);
            let first = notes.iter().map(|sh| sh.sh_addr).min().unwrap_or(lo);
            let last =
                notes.iter().map(|sh| sh.sh_addr.saturating_add(sh.sh_size)).max().unwrap_or(hi);
            if p.p_align != want {
                out.push(
                    s(
                        11,
                        format!(
                            "PT_NOTE at {:#x}: p_align {:#x}, its notes are aligned to {want:#x}",
                            p.p_vaddr, p.p_align
                        ),
                    )
                    .at(p.p_vaddr),
                );
            }
            if first != lo || last != hi {
                out.push(
                    s(11, format!("PT_NOTE [{lo:#x}, {hi:#x}) is not its note sections [{first:#x}, {last:#x})"))
                        .at(p.p_vaddr),
                );
            }
        }
        let align: usize = if p.p_align == 8 { 8 } else { 4 };
        if !notes_ok(bytes, align) {
            // A segment of notes padded to 4 under a p_align of 8: what a
            // reader that pads by p_align (glibc's PT_NOTE scan) misreads.
            let msg = if align == 8 && notes_ok(bytes, 4) {
                format!(
                    "PT_NOTE at {:#x} has p_align 8 but is a sequence of whole notes only with padding to 4",
                    p.p_vaddr
                )
            } else {
                format!("PT_NOTE at {:#x} is not a sequence of whole notes", p.p_vaddr)
            };
            out.push(s(11, msg).at(p.p_vaddr));
        }
    }
    // S11: PT_GNU_PROPERTY is exactly `.note.gnu.property` (the Linux
    // "Program Property" extension: the loader reads the properties
    // through it), with the note's alignment (WP-028d).
    for p in image.phdrs.iter().filter(|p| p.p_type == PT_GNU_PROPERTY) {
        let note = image.sections.iter().find(|x| x.name == ".note.gnu.property");
        let ok = note.is_some_and(|x| {
            let sh = &x.shdr;
            sh.sh_addr == p.p_vaddr
                && sh.sh_size == p.p_filesz
                && p.p_memsz == p.p_filesz
                && sh.sh_offset == p.p_offset
                && p.p_align == sh.sh_addralign.max(1)
        });
        if !ok {
            out.push(
                s(
                    11,
                    format!(
                        "PT_GNU_PROPERTY [{:#x}, +{:#x}) (p_memsz {:#x}, p_align {:#x}) is not .note.gnu.property",
                        p.p_vaddr, p.p_filesz, p.p_memsz, p.p_align
                    ),
                )
                .at(p.p_vaddr),
            );
        }
    }
    out
}

/// The allocated sections with memory that lie in the `PT_LOAD` `p`.
fn sections_in<'s>(image: &'s Image<'_>, p: &Phdr) -> Vec<&'s mortar_elf::Shdr> {
    let (lo, hi) = (p.p_vaddr, p.p_vaddr.saturating_add(p.p_memsz));
    image
        .sections
        .iter()
        .skip(1)
        .map(|x| &x.shdr)
        .filter(|sh| occupies_memory(sh) && sh.sh_size > 0 && sh.sh_addr >= lo && sh.sh_addr < hi)
        .collect()
}

/// Whether `b` is a sequence of whole notes (gABI "Note Section").
fn notes_ok(b: &[u8], align: usize) -> bool {
    let up = |x: usize| x.div_ceil(align).checked_mul(align);
    let mut at = 0usize;
    while at < b.len() {
        let Some(h) = b.get(at..at + 12) else { return false };
        let namesz = u32::from_le_bytes([h[0], h[1], h[2], h[3]]) as usize;
        let descsz = u32::from_le_bytes([h[4], h[5], h[6], h[7]]) as usize;
        // The descriptor starts at the next multiple of the alignment after
        // the name, and the next note after the descriptor (an 8-aligned
        // property note pads its 4-byte name to 4 only, gABI and the Linux
        // "Program Property" extension).
        let Some(end) = namesz
            .checked_add(12)
            .and_then(|n| at.checked_add(n))
            .and_then(up)
            .and_then(|d| d.checked_add(descsz))
        else {
            return false;
        };
        // The padding after a descriptor is there "to ensure 4-byte
        // alignment for the next note entry" (gABI "Note Section"): the
        // last note of the segment may end at its descriptor (GNU ld, lld
        // and mold write a 6-byte build id so, p_filesz 22).
        if end == b.len() {
            return true;
        }
        let Some(next) = up(end) else { return false };
        if next > b.len() {
            return false;
        }
        at = next;
    }
    true
}

/// S13 to S18 and mortar's S2 clause (§6.2, from `spec/layout.md` §20.3):
/// the layout rules a loader, strip and glibc's start-up rely on.
#[allow(clippy::too_many_lines)] // one block per rule, in the spec's order
pub(crate) fn check_layout(image: &Image<'_>, map: &crate::Map) -> Vec<Finding> {
    let format = map.format;
    let mut out = Vec::new();
    let h = &image.header;
    let loads: Vec<&Phdr> = image.phdrs.iter().filter(|p| p.p_type == PT_LOAD).collect();
    let in_file_image = |off: u64, len: u64| {
        loads.iter().any(|p| {
            off >= p.p_offset && off.saturating_add(len) <= p.p_offset.saturating_add(p.p_filesz)
        })
    };
    // S1 (mortar): EI_OSABI is ELFOSABI_GNU exactly when `.symtab` or
    // `.dynsym` holds an STT_GNU_IFUNC or STB_GNU_UNIQUE symbol, or the
    // output has an IRELATIVE record
    // (spec/image.md §5.4; a64.md Q11; x64.md X-49; x86.md X86-64).
    if format == Format::Mortar {
        let gnu = gnu_extension(image);
        if (h.ei_osabi == 3) != gnu {
            out.push(s(
                1,
                format!(
                    "EI_OSABI is {} in an output {} a GNU extension (IFUNC, UNIQUE, IRELATIVE)",
                    h.ei_osabi,
                    if gnu { "with" } else { "without" }
                ),
            ));
        }
    }
    let opts = crate::opts::of(map);
    // S2 (mortar): every PT_LOAD has p_align P = max(maxpage, the largest
    // alignment of a section in it), maxpage being the line's last
    // `-z max-page-size` (0: the default, 64 KiB on AArch64, 4 KiB on
    // x86-64 and i386); a non-PIE executable starts at
    // align_down(image base, P1) on i386, 0x08048000 (layout.md L-67, L-68,
    // L-74, x86.md X86-65).
    // `-z common-page-size` changes nothing (L-75). PT_GNU_STACK has
    // p_align 16 on i386 (X86-66).
    if let Some(o) = &opts {
        let default = if h.e_machine == EM_AARCH64 { 0x1_0000 } else { 0x1000 };
        let maxpage = o.max_page.filter(|&n| n != 0).unwrap_or(default);
        let want = |p: &Phdr| {
            sections_in(image, p).iter().map(|sh| sh.sh_addralign).max().unwrap_or(1).max(maxpage)
        };
        for p in &loads {
            let w = want(p);
            if p.p_align != w {
                out.push(
                    s(
                        2,
                        format!(
                            "PT_LOAD at {:#x}: p_align {:#x}, not {w:#x} (maxpage {maxpage:#x} \
                             and its sections' alignment, L-67, L-74)",
                            p.p_vaddr, p.p_align
                        ),
                    )
                    .at(p.p_vaddr),
                );
            }
        }
        // The base on i386 only: the corpus tests read lld's 64-bit
        // outputs (base 0x200000) under mortar's map (WP-131).
        if h.e_type == ET_EXEC
            && h.e_machine == EM_386
            && let Some(first) = loads.iter().min_by_key(|p| p.p_vaddr)
        {
            let image_base: u64 = 0x0804_8000;
            let base = image_base & !(want(first) - 1);
            if first.p_vaddr != base {
                out.push(s(
                    2,
                    format!(
                        "the first PT_LOAD is at {:#x}, not align_down({image_base:#x}, {:#x}) = \
                         {base:#x} (L-68)",
                        first.p_vaddr,
                        want(first)
                    ),
                ));
            }
        }
        if h.e_machine == EM_386 {
            for p in image.phdrs.iter().filter(|p| p.p_type == PT_GNU_STACK && p.p_align != 16) {
                out.push(s(10, format!("PT_GNU_STACK has p_align {:#x}, not 16", p.p_align)));
            }
        }
    }
    // S2 (mortar): p_align is at least 4 KiB (L-1).
    if format == Format::Mortar {
        for p in loads.iter().filter(|p| p.p_align < 4096) {
            out.push(
                s(
                    2,
                    format!("PT_LOAD at {:#x}: p_align {:#x} is below 4 KiB", p.p_vaddr, p.p_align),
                )
                .at(p.p_vaddr),
            );
        }
    }
    if format == Format::Mortar {
        out.extend(present(image, opts.as_ref()));
    }
    // S13: PT_PHDR and PT_INTERP, at most one each, before every PT_LOAD;
    // PT_PHDR and the program header table in a PT_LOAD's file image
    // (L-2, L-3).
    let first_load = image.phdrs.iter().position(|p| p.p_type == PT_LOAD);
    for (ty, name) in [(PT_PHDR, "PT_PHDR"), (PT_INTERP, "PT_INTERP")] {
        let at: Vec<usize> = image
            .phdrs
            .iter()
            .enumerate()
            .filter(|(_, p)| p.p_type == ty)
            .map(|(i, _)| i)
            .collect();
        if at.len() > 1 {
            out.push(s(13, format!("{} {name} headers", at.len())));
        }
        if let (Some(&i), Some(l)) = (at.first(), first_load)
            && i > l
        {
            out.push(s(13, format!("{name} comes after a PT_LOAD")));
        }
        if ty == PT_PHDR
            && let Some(&i) = at.first()
            && !in_file_image(image.phdrs[i].p_offset, image.phdrs[i].p_filesz)
        {
            out.push(s(13, "PT_PHDR is in no PT_LOAD's file image").at(image.phdrs[i].p_vaddr));
        }
        // PT_PHDR is the program header table (gABI "Program Header"): a
        // loader takes AT_PHDR and AT_PHNUM from it (WP-028d).
        if ty == PT_PHDR
            && let Some(&i) = at.first()
        {
            let p = &image.phdrs[i];
            let len = u64::from(h.e_phnum) * u64::from(h.e_phentsize);
            // lld may leave room for more headers than it writes.
            if p.p_offset != h.e_phoff || p.p_filesz < len || p.p_memsz != p.p_filesz {
                out.push(
                    s(
                        13,
                        format!(
                            "PT_PHDR (offset {:#x}, p_filesz {:#x}, p_memsz {:#x}) is not the program header table (offset {:#x}, {len:#x} bytes)",
                            p.p_offset, p.p_filesz, p.p_memsz, h.e_phoff
                        ),
                    )
                    .at(p.p_vaddr),
                );
            }
        }
    }
    if h.e_phnum > 0 {
        let len = u64::from(h.e_phnum) * u64::from(h.e_phentsize);
        if !in_file_image(h.e_phoff, len) {
            out.push(s(13, "the program header table is in no PT_LOAD's file image"));
        }
    }
    for sec in image.sections.iter().skip(1) {
        let sh = &sec.shdr;
        // S14: an allocated NOBITS section is past its PT_LOAD's file image (L-4).
        if occupies_memory(sh)
            && sh.sh_type == SHT_NOBITS
            && sh.sh_size > 0
            && let Some(p) = image.load_of(sh.sh_addr, sh.sh_size)
            && sh.sh_addr < p.p_vaddr.saturating_add(p.p_filesz)
        {
            out.push(
                s(14, format!("NOBITS section '{}' lies in its PT_LOAD's file image", sec.name))
                    .at(sh.sh_addr),
            );
        }
        // S15: sh_addr is a multiple of sh_addralign (L-5).
        if sh.sh_addralign > 1 && !sh.sh_addr.is_multiple_of(sh.sh_addralign) {
            out.push(
                s(15, format!("section '{}' is not aligned to {:#x}", sec.name, sh.sh_addralign))
                    .at(sh.sh_addr),
            );
        }
        // S17: a non-allocated section is outside every PT_LOAD's file
        // image (L-18).
        if sh.sh_flags & SHF_ALLOC == 0 && sh.sh_type != SHT_NOBITS && sh.sh_size > 0 {
            let (lo, hi) = (sh.sh_offset, sh.sh_offset.saturating_add(sh.sh_size));
            // An empty file image overlaps nothing (WP-059's F1).
            if loads.iter().any(|p| {
                p.p_filesz > 0 && lo < p.p_offset.saturating_add(p.p_filesz) && p.p_offset < hi
            }) {
                out.push(s(
                    17,
                    format!("non-allocated section '{}' lies in a PT_LOAD's file image", sec.name),
                ));
            }
        }
    }
    // S16: RELRO safety for each page size up to the PT_LOADs' p_align (L-8).
    let max_align = loads.iter().map(|p| p.p_align).max().unwrap_or(0);
    for r in image.phdrs.iter().filter(|p| p.p_type == PT_GNU_RELRO) {
        let (lo, hi) = (r.p_vaddr, r.p_vaddr.saturating_add(r.p_memsz));
        for page in [0x1000u64, 0x4000, 0x10000].into_iter().filter(|&p| p <= max_align.max(0x1000))
        {
            // The protected range is [align_down(s), align_down(e)); its
            // part at or past s is RELRO by the linker's own choice (GNU ld
            // puts the reserved head of .got.plt there), so a breach is a
            // writable byte in [align_down(s), s).
            let (plo, phi) = (lo & !(page - 1), hi & !(page - 1));
            let below = lo.min(phi);
            for sec in image.sections.iter().skip(1) {
                let sh = &sec.shdr;
                let (a, b) = (sh.sh_addr, sh.sh_addr.saturating_add(sh.sh_size));
                if occupies_memory(sh)
                    && sh.sh_flags & SHF_WRITE != 0
                    && sh.sh_size > 0
                    && a < below
                    && plo < b
                {
                    out.push(
                        s(
                            16,
                            format!(
                                "writable section '{}' is made read-only with RELRO under {} KiB pages",
                                sec.name,
                                page / 1024
                            ),
                        )
                        .at(a),
                    );
                }
            }
        }
    }
    // S20: PT_GNU_RELRO ends at a section's end: a section that starts in
    // it ends in it (GNU ld's .got.plt aside, S16), and the sections the
    // gABI and every reference make read-only after relocation lie in it
    // (a RELRO cut short leaves the end of .got writable under a page
    // boundary; WP-028d).
    for r in image.phdrs.iter().filter(|p| p.p_type == PT_GNU_RELRO) {
        let (lo, hi) = (r.p_vaddr, r.p_vaddr.saturating_add(r.p_memsz));
        for sec in image.sections.iter().skip(1) {
            let sh = &sec.shdr;
            let (a, b) = (sh.sh_addr, sh.sh_addr.saturating_add(sh.sh_size));
            if !occupies_memory(sh) || sh.sh_size == 0 || sec.name == ".got.plt" {
                continue;
            }
            let relro = matches!(
                sec.name.as_str(),
                ".init_array" | ".fini_array" | ".preinit_array" | ".got" | ".dynamic"
            ) || sec.name.starts_with(".data.rel.ro");
            let starts_in = a >= lo && a < hi;
            if (starts_in || relro) && !(a >= lo && b <= hi) {
                out.push(
                    s(
                        20,
                        format!(
                            "section '{}' [{a:#x}, {b:#x}) is not inside PT_GNU_RELRO [{lo:#x}, {hi:#x})",
                            sec.name
                        ),
                    )
                    .at(a),
                );
            }
        }
    }
    // S20: PT_GNU_RELRO ends at the end of a section, at a 4 KiB boundary
    // (GNU ld pads it to the page), or (GNU ld) inside the reserved head
    // of `.got.plt` (three words, S16); elsewhere it is cut short or runs
    // into writable data (WP-028d).
    for r in image.phdrs.iter().filter(|p| p.p_type == PT_GNU_RELRO && p.p_memsz > 0) {
        let hi = r.p_vaddr.saturating_add(r.p_memsz);
        let word: u64 = if image.class() == mortar_elf::Class::Elf64 { 8 } else { 4 };
        let ok = hi.is_multiple_of(0x1000)
            || image.sections.iter().skip(1).any(|x| {
                let sh = &x.shdr;
                occupies_memory(sh)
                    && (sh.sh_addr.saturating_add(sh.sh_size) == hi
                        || x.name == ".got.plt" && hi > sh.sh_addr && hi <= sh.sh_addr + 3 * word)
            });
        if !ok {
            out.push(
                s(20, format!("PT_GNU_RELRO ends at {hi:#x}, which is no section's end"))
                    .at(r.p_vaddr),
            );
        }
    }
    // S21: PT_GNU_EH_FRAME is exactly `.eh_frame_hdr` (the unwinder finds
    // the search table through it; LSB "Exception Frames", WP-028d).
    for p in image.phdrs.iter().filter(|p| p.p_type == PT_GNU_EH_FRAME) {
        let ok = image.sections.iter().find(|x| x.name == ".eh_frame_hdr").is_some_and(|x| {
            let sh = &x.shdr;
            sh.sh_addr == p.p_vaddr
                && sh.sh_offset == p.p_offset
                && sh.sh_size == p.p_filesz
                && p.p_memsz == p.p_filesz
        });
        if !ok {
            out.push(
                s(
                    21,
                    format!(
                        "PT_GNU_EH_FRAME [{:#x}, +{:#x}) (p_memsz {:#x}) is not .eh_frame_hdr",
                        p.p_vaddr, p.p_filesz, p.p_memsz
                    ),
                )
                .at(p.p_vaddr),
            );
        }
    }
    // S7 (mortar): a nonzero entry point is where a symbol other than a
    // mapping symbol is, when the output has a symbol table with
    // functions: the entry symbol's value (L-103: mortar writes the entry
    // symbol's value or 0; the references fall back to the start of
    // `.text`; WP-028d). The entry symbol may be local: a hidden `_start`
    // is made local in the output (gABI STV_HIDDEN).
    let labelled = |v: u64| {
        image.symbols.iter().any(|x| {
            x.index != 0
                && x.sym.st_shndx != SHN_UNDEF
                && !x.name.starts_with('$')
                && !matches!(x.sym.sym_type(), STT_SECTION | STT_FILE | STT_TLS)
                && x.sym.st_value == v
        })
    };
    let functions = image.symbols.iter().any(|x| x.sym.sym_type() == mortar_elf::consts::STT_FUNC);
    if format == Format::Mortar
        && h.e_type == ET_EXEC
        && h.e_entry != 0
        && functions
        && !labelled(h.e_entry)
    {
        out.push(
            s(7, format!("the entry point {:#x} is no symbol's value", h.e_entry)).at(h.e_entry),
        );
    }
    // S5: a PT_LOAD is PF_W only when it holds a writable section, PF_X
    // only when it holds an executable one (L-6, strengthened: no
    // permission the program does not need; WP-028d for mortar, every
    // format since WP-088: the references' outputs obey it too).
    {
        for p in &loads {
            let secs = sections_in(image, p);
            if secs.is_empty() {
                continue;
            }
            for (bit, flag, name) in [(PF_W, SHF_WRITE, "PF_W"), (PF_X, SHF_EXECINSTR, "PF_X")] {
                if p.p_flags & bit != 0 && !secs.iter().any(|sh| sh.sh_flags & flag != 0) {
                    out.push(
                        s(
                            5,
                            format!(
                                "PT_LOAD at {:#x} is {name} and holds no section that needs it",
                                p.p_vaddr
                            ),
                        )
                        .at(p.p_vaddr),
                    );
                }
            }
        }
    }
    // S18 (mortar): every PT_LOAD's memory ends at a section's end (L-9).
    if format == Format::Mortar {
        let ehsize: u64 = if image.class() == mortar_elf::Class::Elf64 { 64 } else { 52 };
        let phend = h.e_phoff.saturating_add(u64::from(h.e_phnum) * u64::from(h.e_phentsize));
        for p in &loads {
            let end = p.p_vaddr.saturating_add(p.p_memsz);
            let header_end = (p.p_offset == 0).then(|| p.p_vaddr + phend.max(ehsize));
            let ends_section = image.sections.iter().skip(1).any(|x| {
                occupies_memory(&x.shdr) && x.shdr.sh_addr.saturating_add(x.shdr.sh_size) == end
            });
            if p.p_memsz > 0 && !ends_section && header_end != Some(end) {
                out.push(
                    s(
                        18,
                        format!(
                            "PT_LOAD at {:#x} ends at {end:#x}, which is no section's end",
                            p.p_vaddr
                        ),
                    )
                    .at(p.p_vaddr),
                );
            }
        }
    }
    out
}

/// The headers and tables mortar always writes, absent (WP-131 V5; §6.2):
/// S10 one `PT_GNU_STACK` (L-84); S11 every allocated note in a `PT_NOTE`
/// (L-67); S20 a `PT_GNU_RELRO` when there is a `.pad.relro`, ending
/// where it ends (L-64, L-67); Y2 a `.symtab` exactly when the
/// line has no `-s` (synth.md S-84); S1 the special sections have their
/// gABI types (a reader finds them by type), and none is `SHF_COMPRESSED`
/// (mortar writes `--compress-debug-sections=none` only; WP-131 V4).
fn present(image: &Image<'_>, opts: Option<&crate::opts::Options>) -> Vec<Finding> {
    let mut out = Vec::new();
    let stacks = image.phdrs.iter().filter(|p| p.p_type == PT_GNU_STACK).count();
    if stacks == 0 {
        out.push(s(10, "no PT_GNU_STACK, which mortar always writes (L-84)"));
    }
    let notes: Vec<&Phdr> = image.phdrs.iter().filter(|p| p.p_type == PT_NOTE).collect();
    for x in image.sections.iter().skip(1) {
        let sh = &x.shdr;
        if sh.sh_type != SHT_NOTE || sh.sh_flags & SHF_ALLOC == 0 || sh.sh_size == 0 {
            continue;
        }
        let end = sh.sh_addr.saturating_add(sh.sh_size);
        if !notes
            .iter()
            .any(|p| sh.sh_addr >= p.p_vaddr && end <= p.p_vaddr.saturating_add(p.p_memsz))
        {
            out.push(
                s(11, format!("no PT_NOTE covers {}, an allocated note (L-67)", x.name))
                    .at(sh.sh_addr),
            );
        }
    }
    let relro: Vec<&Phdr> = image.phdrs.iter().filter(|p| p.p_type == PT_GNU_RELRO).collect();
    match (image.sections.iter().find(|x| x.name == ".pad.relro"), relro.as_slice()) {
        (Some(pad), []) => out.push(
            s(20, "a .pad.relro and no PT_GNU_RELRO: rank 7 stays writable (L-64, L-67)")
                .at(pad.shdr.sh_addr),
        ),
        (Some(pad), [r]) => {
            let end = pad.shdr.sh_addr.saturating_add(pad.shdr.sh_size);
            if r.p_vaddr.saturating_add(r.p_memsz) != end {
                out.push(
                    s(
                        20,
                        format!(
                            "PT_GNU_RELRO ends at {:#x}, not at the end of .pad.relro {end:#x} (L-67)",
                            r.p_vaddr.saturating_add(r.p_memsz)
                        ),
                    )
                    .at(r.p_vaddr),
                );
            }
        }
        _ => {}
    }
    if let Some(o) = opts {
        let symtab = image.sections.iter().any(|x| x.shdr.sh_type == SHT_SYMTAB);
        if symtab == o.strip_all {
            let msg = if o.strip_all {
                "a .symtab, and the line says -s (synth.md S-84)"
            } else {
                "no .symtab (SHT_SYMTAB), and the line does not say -s (synth.md S-84)"
            };
            out.push(Finding::new(Rule::SymbolTable(2), msg));
        }
    }
    for x in image.sections.iter().skip(1).filter(|x| x.shdr.sh_flags & SHF_COMPRESSED != 0) {
        out.push(
            s(1, format!("{} is SHF_COMPRESSED, which mortar does not write (image.md)", x.name))
                .at(x.shdr.sh_addr),
        );
    }
    for x in image.sections.iter().skip(1) {
        let want: &[u32] = match x.name.as_str() {
            ".eh_frame" if image.header.e_machine == EM_X86_64 => {
                &[SHT_PROGBITS, SHT_X86_64_UNWIND]
            }
            ".interp" | ".eh_frame_hdr" | ".eh_frame" => &[SHT_PROGBITS],
            ".dynamic" => &[SHT_DYNAMIC],
            ".dynsym" => &[SHT_DYNSYM],
            ".symtab" => &[SHT_SYMTAB],
            ".dynstr" | ".strtab" | ".shstrtab" => &[SHT_STRTAB],
            ".gnu.hash" => &[SHT_GNU_HASH],
            ".hash" => &[SHT_HASH],
            ".init_array" => &[SHT_INIT_ARRAY],
            ".fini_array" => &[SHT_FINI_ARRAY],
            ".preinit_array" => &[SHT_PREINIT_ARRAY],
            ".gnu.version" => &[SHT_GNU_VERSYM],
            ".gnu.version_r" => &[SHT_GNU_VERNEED],
            ".gnu.version_d" => &[SHT_GNU_VERDEF],
            n if n.starts_with(".note") => &[SHT_NOTE],
            _ => continue,
        };
        if !want.contains(&x.shdr.sh_type) {
            out.push(
                s(
                    1,
                    format!(
                        "{} has sh_type {:#x}, not its gABI type {:#x}",
                        x.name, x.shdr.sh_type, want[0]
                    ),
                )
                .at(x.shdr.sh_addr),
            );
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::notes_ok;

    /// One note: `GNU\0`, type 3, a `desc`-byte descriptor, padded to 4
    /// when `pad`.
    fn note(desc: usize, pad: bool) -> Vec<u8> {
        let mut b = Vec::new();
        for w in [4u32, desc as u32, 3] {
            b.extend_from_slice(&w.to_le_bytes());
        }
        b.extend_from_slice(b"GNU\0");
        b.extend(std::iter::repeat_n(0xab, desc));
        if pad {
            b.resize(b.len().next_multiple_of(4), 0);
        }
        b
    }

    // spec/plumb.md §6.2 S11 (WP-028d): the gABI pads a descriptor "to
    // ensure 4-byte alignment for the next note entry", so the last note
    // may end at its descriptor (the reference linkers' 6-byte build id,
    // p_filesz 22); a note that follows an unpadded one is misread.
    #[test]
    fn s6_2_s11_the_last_note_needs_no_padding() {
        assert!(notes_ok(&note(6, true), 4));
        assert!(notes_ok(&note(6, false), 4), "the last note, unpadded");
        assert!(notes_ok(&[note(8, true), note(6, false)].concat(), 4));
        assert!(!notes_ok(&[note(6, false), note(8, true)].concat(), 4), "padding before a note");
        let mut cut = note(6, false);
        cut.pop();
        assert!(!notes_ok(&cut, 4), "a descriptor cut short");
    }
}
