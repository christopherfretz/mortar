//! Stage 3's static-pie rules (`spec/plumb.md` §12.4, WP-089): P-D14,
//! what a static-pie owes glibc's self-relocation, and P-D15, how glibc
//! applies its records.
//!
//! glibc 2.41 starts a static-pie in `_dl_relocate_static_pie`
//! (`elf/dl-reloc-static-pie.c`): the load bias is `&__ehdr_start` (the
//! ELF header must be at virtual address 0), the map is read from
//! `_DYNAMIC`, and `ELF_DYNAMIC_RELOCATE` applies the records with lazy 0
//! (every `DT_JMPREL` record now, whatever `-z now` says). Symbol lookup
//! is the map itself and TLS is set up later, so only `RELATIVE`,
//! `IRELATIVE` and `NONE` mean anything. `csu/libc-start.c` then applies
//! `[__rela_iplt_start, __rela_iplt_end)` with `apply_irel`, which adds
//! no load bias: that range must be empty.

use std::collections::BTreeMap;

use mortar_elf::consts::{EM_AARCH64, EM_X86_64, PT_LOAD, PT_PHDR};

use crate::dynout::{
    self, DT_JMPREL, DT_NEEDED, DT_PLTREL, DT_PLTRELSZ, DT_RELA, DT_RELACOUNT, DT_RELASZ, DT_RELR,
    DT_VERDEF, DT_VERNEED, DT_VERNEEDNUM, DT_VERSYM, DynOut,
};
use crate::image::Image;
use crate::loader::{Kind, kind};
use crate::map::Format;
use crate::report::{Finding, Rule};

fn d14(msg: impl Into<String>) -> Finding {
    Finding::new(Rule::Dyn(14), msg)
}

fn d15(msg: impl Into<String>) -> Finding {
    Finding::new(Rule::Dyn(15), msg)
}

/// One record of an allocated `SHT_RELA` section: what the linker wrote,
/// whatever `.dynamic` says.
#[derive(Debug, Clone, Copy)]
pub(crate) struct Rec {
    pub at: u64,
    pub offset: u64,
    pub ty: u32,
    pub sym: u32,
}

/// The records of every allocated `SHT_RELA` section, by address.
pub(crate) fn records(image: &Image<'_>) -> BTreeMap<u64, Rec> {
    let mut m = BTreeMap::new();
    for s in image.sections.iter().filter(|s| s.alloc() && s.shdr.sh_type == 4) {
        let Some(b) = image.bytes(s.shdr.sh_addr, s.shdr.sh_size) else { continue };
        for (i, r) in b.chunks_exact(24).enumerate() {
            let q = |o: usize| {
                let mut a = [0u8; 8];
                a.copy_from_slice(&r[o..o + 8]);
                u64::from_le_bytes(a)
            };
            let at = s.shdr.sh_addr + 24 * i as u64;
            let info = q(8);
            m.insert(
                at,
                Rec { at, offset: q(0), ty: (info & 0xffff_ffff) as u32, sym: (info >> 32) as u32 },
            );
        }
    }
    m
}

/// glibc's ranges (`elf/dynamic-link.h` `_ELF_DYNAMIC_DO_RELOC`): range 0
/// is `DT_RELA`/`DT_RELASZ`; `DT_JMPREL`/`DT_PLTRELSZ` joins it when it
/// starts where range 0 ends, is cut off its end when range 0 ends where
/// it does, and is range 1 otherwise. `(start, size)` each.
pub(crate) fn ranges(out: &DynOut) -> Vec<(u64, u64)> {
    let mut r0 = (out.get(DT_RELA).unwrap_or(0), out.get(DT_RELASZ).unwrap_or(0));
    let mut v = Vec::new();
    if out.has(DT_PLTREL) || out.has(DT_JMPREL) {
        let (start, size) = (out.get(DT_JMPREL).unwrap_or(0), out.get(DT_PLTRELSZ).unwrap_or(0));
        if r0.0 == 0 {
            r0.0 = start;
        }
        if r0.0.wrapping_add(r0.1) == start.wrapping_add(size) {
            r0.1 = r0.1.wrapping_sub(size);
        }
        if r0.0.wrapping_add(r0.1) == start {
            r0.1 = r0.1.wrapping_add(size);
            v.push(r0);
        } else {
            v.push(r0);
            v.push((start, size));
        }
    } else {
        v.push(r0);
    }
    v.retain(|r| r.1 > 0);
    v
}

/// Whether the output is a static-pie: `ET_DYN` with `PT_DYNAMIC` and no
/// `PT_INTERP` (D-1).
pub(crate) fn is_static_pie(image: &Image<'_>) -> bool {
    use mortar_elf::consts::{ET_DYN, PT_DYNAMIC, PT_INTERP};
    image.header.e_type == ET_DYN
        && image.phdrs.iter().any(|p| p.p_type == PT_DYNAMIC)
        && !image.phdrs.iter().any(|p| p.p_type == PT_INTERP)
}

/// P-D14 and P-D15.
#[allow(clippy::too_many_lines)] // one block per property
pub(crate) fn check(
    image: &Image<'_>,
    out: &DynOut,
    entry: Option<&str>,
    format: Format,
    v: &mut Vec<Finding>,
) {
    let machine = image.header.e_machine;
    let mortar = format == Format::Mortar;
    // P-D14, headers: glibc takes the bias from &__ehdr_start.
    let first =
        image.phdrs.iter().filter(|p| p.p_type == PT_LOAD).min_by_key(|p| p.p_vaddr).copied();
    if first.is_none_or(|p| p.p_vaddr != 0 || p.p_offset != 0) {
        v.push(d14(
            "the first PT_LOAD does not map the ELF header at address 0: glibc takes the load bias from &__ehdr_start (D-1)",
        ));
    }
    if let Some(e) = image.global("__ehdr_start").or_else(|| image.unique("__ehdr_start"))
        && e.sym.st_shndx != 0
        && e.sym.st_value != 0
    {
        v.push(d14(format!("__ehdr_start is {:#x}, not 0 (D-1)", e.sym.st_value)));
    }
    // The entry: executable, and the entry symbol's value (`-e`, else
    // `_start`) when .symtab defines it (S7 judges ET_EXEC only).
    let e = image.header.e_entry;
    let insn = if machine == EM_AARCH64 { 4 } else { 1 };
    if image.load_of(e, insn).is_none_or(|p| p.p_flags & mortar_elf::consts::PF_X == 0) {
        v.push(d14(format!("the entry point {e:#x} is in no executable PT_LOAD")).at(e));
    }
    let name = entry.unwrap_or("_start");
    if let Some(s) = image.global(name).or_else(|| image.unique(name))
        && s.sym.st_shndx != 0
        && s.sym.st_value != e
    {
        let mut f = d14(format!("the entry point is not '{name}'")).at(e);
        f.expected = Some(s.sym.st_value);
        f.found = Some(e);
        v.push(f);
    }
    if mortar && image.phdrs.iter().any(|p| p.p_type == PT_PHDR) {
        v.push(d14("PT_PHDR in a static-pie (D-1; layout L-67)"));
    }
    // P-D14, .dynamic: nothing a loader reads.
    for (tag, why) in [
        (DT_NEEDED, "a static-pie loads no library (D-1)"),
        (DT_VERSYM, "glibc's self-relocation would take the versioned path (D-1)"),
        (DT_VERNEED, "a static-pie binds no version (D-1)"),
        (DT_VERNEEDNUM, "a static-pie binds no version (D-1)"),
        (DT_VERDEF, "a static-pie defines no version (D-1)"),
        (DT_RELR, "stage 3 does not model DT_RELR"),
    ] {
        if out.has(tag) {
            v.push(d14(format!("{} in a static-pie: {why}", dynout::tag_name(tag))));
        }
    }
    // P-D14, .dynsym: exports nothing (no -E, which is outside stage 3).
    for s in out.syms.iter().skip(1) {
        // GNU ld's AArch64 section symbols name nothing a lookup finds.
        if s.defined() && (mortar || s.sym_type() != mortar_elf::consts::STT_SECTION) {
            v.push(d14(format!(
                "'{}' is exported by a static-pie: nothing can look it up (D-1)",
                s.name
            )));
        } else if mortar {
            v.push(d14(format!(
                "'{}' in a static-pie's .dynsym: D-1 writes the null entry only",
                s.name
            )));
        }
    }
    // P-D14, records: only what the self-relocation means.
    let recs = records(image);
    for r in recs.values() {
        let k = kind(machine, r.ty);
        match k {
            Kind::Relative | Kind::Irelative if r.sym == 0 => {}
            Kind::None if !mortar => {}
            Kind::None => v.push(d14("an R_*_NONE record (D-40)").at(r.at)),
            Kind::DtpMod | Kind::DtpOff | Kind::TpOff | Kind::TlsDesc => v.push(
                d14(format!(
                    "a TLS record (type {}) in a static-pie: glibc sets the thread pointer up after self-relocation",
                    r.ty
                ))
                .at(r.offset),
            ),
            _ => v.push(
                d14(format!(
                    "a record of type {} (symbol {}) in a static-pie: the self-relocation applies only RELATIVE and IRELATIVE without a symbol (D-1)",
                    r.ty, r.sym
                ))
                .at(r.offset),
            ),
        }
    }
    // P-D14, the GOT's _DYNAMIC word (D-32): link-time, no record.
    if let Some((dyn_at, _)) = out.dynamic {
        if let Some(g) =
            image.global("_GLOBAL_OFFSET_TABLE_").or_else(|| image.unique("_GLOBAL_OFFSET_TABLE_"))
            && g.sym.st_shndx != 0
        {
            let at = g.sym.st_value;
            let w = image.value(at, 8);
            if w != Some(dyn_at) {
                let mut f =
                    d14("the word at _GLOBAL_OFFSET_TABLE_ does not hold _DYNAMIC (D-32)").at(at);
                f.expected = Some(dyn_at);
                f.found = w;
                v.push(f);
            }
            if recs.values().any(|r| r.offset == at && kind(machine, r.ty) != Kind::None) {
                v.push(
                    d14("a record writes the GOT's _DYNAMIC word: it holds the link-time address (D-32)")
                        .at(at),
                );
            }
        }
        if mortar {
            let head = match machine {
                EM_X86_64 => image.sections.iter().find(|s| s.name == ".got.plt"),
                EM_AARCH64 => image.sections.iter().find(|s| s.name == ".got"),
                _ => None,
            };
            if let Some(h) = head
                && h.shdr.sh_size >= 8
                && image.value(h.shdr.sh_addr, 8) != Some(dyn_at)
            {
                v.push(
                    d14(format!("{}[0] does not hold _DYNAMIC (D-32)", h.name)).at(h.shdr.sh_addr),
                );
            }
        }
    }
    // P-D15: each record applied as glibc applies it.
    let rs = ranges(out);
    let mut applied: BTreeMap<u64, u32> = BTreeMap::new();
    let mut seq: Vec<(u64, Kind)> = Vec::new();
    let nrel = out.get(DT_RELACOUNT).unwrap_or(0);
    for (ri, &(start, size)) in rs.iter().enumerate() {
        let mut run = Vec::new();
        let mut at = start;
        let end = start.saturating_add(size);
        while at < end {
            let Some(r) = recs.get(&at) else {
                v.push(
                    d15(format!(
                        "glibc's relocation range [{start:#x}, +{size:#x}) covers {at:#x}, which is not a record of an allocated SHT_RELA section"
                    ))
                    .at(at),
                );
                break;
            };
            *applied.entry(at).or_default() += 1;
            let k = if ri == 0 && (at - start) / 24 < nrel {
                Kind::Relative
            } else {
                kind(machine, r.ty)
            };
            run.push((at, k));
            at += 24;
        }
        // x86-64 (ELF_MACHINE_IRELATIVE) defers a range's IRELATIVE
        // records to its end; AArch64 applies the table in order (both
        // measured, spec/plumb.md §12.4).
        if machine == EM_X86_64 {
            seq.extend(run.iter().filter(|x| x.1 != Kind::Irelative).copied());
            seq.extend(run.iter().filter(|x| x.1 == Kind::Irelative).copied());
        } else {
            seq.extend(run);
        }
    }
    for r in recs.values() {
        let k = kind(machine, r.ty);
        if !matches!(k, Kind::Relative | Kind::Irelative) {
            continue;
        }
        match applied.get(&r.at).copied().unwrap_or(0) {
            1 => {}
            0 => v.push(
                d15("a record glibc's self-relocation does not apply (outside DT_RELA and DT_JMPREL)")
                    .at(r.offset),
            ),
            n => v.push(d15(format!("a record glibc applies {n} times")).at(r.offset)),
        }
    }
    if let Some(i) = seq.iter().position(|x| x.1 == Kind::Irelative)
        && let Some(&(at, _)) = seq[i..].iter().find(|x| x.1 == Kind::Relative)
    {
        let r = recs[&at];
        v.push(
            d15("a RELATIVE record is applied after an IRELATIVE: a resolver runs before every address word is relocated")
                .at(r.offset),
        );
    }
    // The C runtime's range: empty.
    let val = |n: &str| {
        image
            .global(n)
            .or_else(|| image.unique(n))
            .filter(|o| o.sym.st_shndx != 0)
            .map(|o| o.sym.st_value)
    };
    if let (Some(lo), Some(hi)) = (val("__rela_iplt_start"), val("__rela_iplt_end"))
        && lo != hi
    {
        let twice = recs
            .range(lo..hi.max(lo))
            .any(|(a, r)| kind(machine, r.ty) == Kind::Irelative && applied.contains_key(a));
        v.push(
            d15(format!(
                "__rela_iplt_start..__rela_iplt_end is [{lo:#x}, {hi:#x}): the C runtime applies it without the load bias{}",
                if twice { ", and the self-relocation applies its records too: twice" } else { "" }
            ))
            .at(lo),
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn out(tags: &[(u64, u64)]) -> DynOut {
        DynOut { entries: tags.to_vec(), ..DynOut::default() }
    }

    // spec/plumb.md §12.4: glibc's ranges (_ELF_DYNAMIC_DO_RELOC).
    #[test]
    fn s12_4_glibc_ranges() {
        // lld and mold: one table.
        assert_eq!(ranges(&out(&[(DT_RELA, 0x300), (DT_RELASZ, 0x60)])), [(0x300, 0x60)]);
        // GNU ld: DT_JMPREL right after .rela.dyn: one range.
        let gnu = [(DT_RELA, 0x300), (DT_RELASZ, 0x60), (DT_PLTREL, 7), (DT_JMPREL, 0x360)];
        let mut t = gnu.to_vec();
        t.push((DT_PLTRELSZ, 0x30));
        assert_eq!(ranges(&out(&t)), [(0x300, 0x90)]);
        // DT_RELASZ covering DT_JMPREL at its end: cut, then joined.
        let t = [
            (DT_RELA, 0x300),
            (DT_RELASZ, 0x90),
            (DT_PLTREL, 7),
            (DT_JMPREL, 0x360),
            (DT_PLTRELSZ, 0x30),
        ];
        assert_eq!(ranges(&out(&t)), [(0x300, 0x90)]);
        // Apart: two ranges, range 0 first.
        let t = [
            (DT_RELA, 0x300),
            (DT_RELASZ, 0x60),
            (DT_PLTREL, 7),
            (DT_JMPREL, 0x900),
            (DT_PLTRELSZ, 0x30),
        ];
        assert_eq!(ranges(&out(&t)), [(0x300, 0x60), (0x900, 0x30)]);
        // No DT_RELA: range 0 is DT_JMPREL's.
        let t = [(DT_PLTREL, 7), (DT_JMPREL, 0x900), (DT_PLTRELSZ, 0x30)];
        assert_eq!(ranges(&out(&t)), [(0x900, 0x30)]);
        // Overlapping, not at the end: applied twice (both ranges).
        let t = [
            (DT_RELA, 0x300),
            (DT_RELASZ, 0x90),
            (DT_PLTREL, 7),
            (DT_JMPREL, 0x318),
            (DT_PLTRELSZ, 0x18),
        ];
        assert_eq!(ranges(&out(&t)), [(0x300, 0x90), (0x318, 0x18)]);
    }
}
