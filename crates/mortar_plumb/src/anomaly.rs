//! The recorded defects of the reference linkers (`spec/plumb.md` §10.2),
//! by name. The verifier reports them like any other finding; a harness
//! that runs it on a reference linker's output (the corpus tests,
//! `cargo xtask linktest`) accepts a finding only when [`classify`] names
//! it and the test expects that anomaly.
//!
//! A reference linker is named by the map format it writes. Nothing here
//! applies to mortar's own links (`Format::Mortar`).

use crate::map::Format;
use crate::report::{Finding, Rule};

/// The anomaly of §10.2 that `f`, a finding on `output` linked by the
/// linker that writes maps in `linker`'s format, is: its id and a line of
/// text, as in `"A9: ..."`. `None` for anything else.
pub fn classify(linker: Format, output: &[u8], f: &Finding) -> Option<&'static str> {
    let (bfd, mold) = (linker == Format::Gnu, linker == Format::Mold);
    if linker == Format::Mortar {
        return None;
    }
    if f.rule == Rule::GotSlot
        && f.symbol.as_deref().is_some_and(|s| s.starts_with('.'))
        && f.found.zip(f.expected).is_some_and(|(found, want)| found < want)
    {
        return Some(
            "A6: GNU ld, lld and mold fill the GOT slot of a section symbol plus an addend without the addend",
        );
    }
    if mold && f.rule == Rule::Erratum(1) {
        return Some(
            "A4: mold accepts --fix-cortex-a53-843419 and leaves erratum 843419 sites as they are",
        );
    }
    if bfd && f.rule == Rule::Erratum(1) && f.message.ends_with(crate::erratum::AFTER_PATCH) {
        return Some(
            "A18: GNU ld leaves an erratum 843419 site unfixed when stubs placed after its scan move code onto it",
        );
    }
    let lld = linker == Format::Lld;
    if lld && f.rule == Rule::Field && f.message.ends_with("(spec/x64.md W4)") {
        return Some(
            "A12: lld writes the 16-byte local-exec form over the 22-byte general-dynamic sequence (spec/x64.md W4)",
        );
    }
    if mold && f.rule == Rule::Structure(11) && f.message.ends_with("only with padding to 4") {
        return Some(
            "A13: mold puts 4-aligned notes in the PT_NOTE of the 8-aligned .note.gnu.property, with p_align 8",
        );
    }
    if lld
        && f.rule == Rule::SymbolTable(3)
        && f.message.starts_with("common symbol")
        && f.message.ends_with("is not allocated in a writable SHT_NOBITS section")
        && f.found == Some(u64::from(mortar_elf::consts::SHN_ABS))
    {
        return Some(
            "A14: lld makes an x86-64 large common (SHN_X86_64_LCOMMON) absolute and allocates nothing (spec/x64.md W9)",
        );
    }
    if (bfd || mold)
        && f.rule == Rule::Field
        && (f.message.ends_with("(spec/x64.md X-53)")
            || f.message.ends_with("(spec/x86.md X86-62)"))
    {
        return Some(
            "A15: GNU ld and mold write a TLS symbol's template address in .debug_addr where its DTP offset belongs (spec/x64.md W10)",
        );
    }
    if (lld || mold) && f.rule == Rule::Structure(19) && f.found == Some(0xcc) {
        return Some(
            "A16: lld and mold pad between .init or .fini fragments with int3, which traps at start-up (spec/x64.md W8)",
        );
    }
    if mold && f.rule == Rule::Ifunc(5) && f.message.contains("does not start with ENDBR64") {
        return Some(
            "A17: mold marks an output IBT and writes its IPLT entries without endbr64 (spec/x64.md W6)",
        );
    }
    let sym = f.symbol.as_deref().unwrap_or("");
    let y = |n| f.rule == Rule::SymbolTable(n);
    if mold && y(3) && f.message.contains("binding") && f.found == Some(0) {
        return Some("A9: mold makes every global of a static executable STB_LOCAL in .symtab");
    }
    if bfd && y(3) && f.message.starts_with("a label of an empty section") {
        return Some(
            "A10: GNU ld gives a label of an empty output section it removes the index of a section that does not hold it",
        );
    }
    if mold && y(5) && sym == "__bss_start" && f.found == Some(0) {
        return Some("A11: mold's __bss_start is 0 when --gc-sections leaves no .bss");
    }
    if mold && (y(5) || f.rule == Rule::Structure(8)) && sym == "_TLS_MODULE_BASE_" {
        return Some("A3: mold's _TLS_MODULE_BASE_ is an address, or there is no PT_TLS");
    }
    if mold
        && f.rule == Rule::EhFrame(5)
        && f.message.starts_with("fde_count counts the FDEs of address range 0")
    {
        return Some(
            "A19: mold gives an FDE with address range 0 an entry in .eh_frame_hdr's table (spec/synth.md S-59)",
        );
    }
    if let Some(a) = dynamic(linker, output, f) {
        return Some(a);
    }
    if mold { ifunc_alias(output, f) } else { None }
}

/// The anomalies of dynamic outputs (stage 3, `spec/plumb.md` §10.2,
/// A20-A32): the wrong outputs of `spec/dyn.md` §13.2 a test records
/// for a reference, and two found by WP-050's judging of WP-051's cases.
#[allow(clippy::too_many_lines, clippy::many_single_char_names)] // one block per anomaly
fn dynamic(linker: Format, output: &[u8], f: &Finding) -> Option<&'static str> {
    let (bfd, lld, mold) = (linker == Format::Gnu, linker == Format::Lld, linker == Format::Mold);
    let d = |n| f.rule == Rule::Dyn(n);
    let m = f.message.as_str();
    if (lld || mold) && d(10) && m.ends_with("(D-41, H1)") {
        return Some(
            "A20: lld and mold put IRELATIVE records before the JUMP_SLOTs; a resolver that calls through the PLT jumps to an unbound slot (dyn.md H1)",
        );
    }
    if bfd && ((d(11) && m.ends_with("(D-21, W2)")) || (d(7) && m.ends_with("(W2)"))) {
        return Some("A21: GNU ld does not export every alias of a copied variable (dyn.md W2)");
    }
    if bfd && d(4) && m.ends_with("names no -dynamic-linker: a linker default (D-31, W10)") {
        return Some(
            "A22: GNU ld writes its built-in interpreter when the line names no -dynamic-linker, wrong on x86-64 (dyn.md W10)",
        );
    }
    if (lld || mold)
        && ((d(1) && m.ends_with("(no -dynamic-linker, no PT_INTERP: D-31, W10)"))
            || (d(4)
                && m.ends_with("names no -dynamic-linker: the kernel cannot start it (D-31, W10)")))
    {
        return Some(
            "A23: lld and mold write no PT_INTERP when the line names no -dynamic-linker; the program cannot start (dyn.md W10)",
        );
    }
    if (bfd || mold)
        && ((d(9) && m.ends_with("a text relocation (D-47, W6)"))
            || (d(6) && m.ends_with("text relocations (D-47, W6)")))
    {
        return Some(
            "A24: GNU ld and mold write dynamic records into read-only memory (DT_TEXTREL) where D-47 refuses (dyn.md W6)",
        );
    }
    if lld
        && d(9)
        && m.starts_with("view 3 ")
        && m.ends_with("no dynamic record names it (D-25, W12)")
        && mortar_elf::FileHeader::parse(output)
            .is_ok_and(|h| h.e_type == mortar_elf::consts::ET_EXEC)
    {
        return Some(
            "A25: lld exports a -no-pie executable's undefined weak key and writes no record for its uses, which a preloaded definition never reaches (dyn.md D-25, W12)",
        );
    }
    if mold && d(2) && m.ends_with("a shared object after -Bstatic (dyn.md D-19)") {
        return Some(
            "A33: mold links a shared object named after -Bstatic (-static-pie): DT_NEEDED and no PT_INTERP, so nothing loads it and the program faults (dyn.md D-19, D-2)",
        );
    }
    if (lld || mold) && d(6) && m.contains("which D-11 item 3 makes needed") {
        return Some(
            "A26: lld and mold leave an under-linked library's provider out of DT_NEEDED; the load fails (dyn.md D-11 item 3)",
        );
    }
    if (bfd || lld || mold)
        && ((d(6) || d(7) || d(9)) && m.ends_with("(only a STV_HIDDEN entry: W1)"))
    {
        return Some(
            "A27: GNU ld, lld and mold bind a reference to a STV_HIDDEN .dynsym entry; the output fails to load (dyn.md W1)",
        );
    }
    let section = |name: &[u8]| {
        let h = mortar_elf::FileHeader::parse(output).ok()?;
        let table = mortar_elf::SectionTable::parse(output, &h).ok()?;
        table.iter().find(|s| s.name().is_ok_and(|n| n == name)).map(|s| {
            let h = s.header();
            (h.sh_addr, h.sh_addr.saturating_add(h.sh_size))
        })
    };
    // A static-pie: ET_DYN with .dynamic and no .interp (spec/plumb.md
    // §12.4); the machine.
    let header = mortar_elf::FileHeader::parse(output).ok();
    let spie = header.as_ref().is_some_and(|h| h.e_type == mortar_elf::consts::ET_DYN)
        && section(b".dynamic").is_some()
        && section(b".interp").is_none();
    let machine = header.as_ref().map_or(0, |h| h.e_machine);
    if bfd
        && spie
        && machine == mortar_elf::consts::EM_AARCH64
        && d(9)
        && m.starts_with("view 2 ")
        && m.ends_with("(a record moves a value that is not an address of the executable)")
    {
        return Some(
            "A30: GNU ld writes a RELATIVE record on an AArch64 static-pie's GOT slot of an absolute symbol; the program reads the value plus the load bias",
        );
    }
    if bfd && spie && machine == mortar_elf::consts::EM_X86_64 {
        let odd = d(10)
            && m.strip_prefix("the PLT entry jumps through 0x")
                .and_then(|r| r.split(',').next())
                .and_then(|h| u64::from_str_radix(h, 16).ok())
                .is_some_and(|slot| slot % 8 != 0);
        // A call to such an entry (crtbeginS's `__cxa_finalize`, g++'s
        // `_ZTH` init guard of a `thread_local`): its rel32 reaches a
        // `.plt.got` entry whose `jmp *disp(%rip)` reads a misaligned slot.
        let bytes = |addr: u64, n: u64| {
            let h = mortar_elf::FileHeader::parse(output).ok()?;
            let table = mortar_elf::SectionTable::parse(output, &h).ok()?;
            let (base, off) = table
                .iter()
                .map(|sec| {
                    let x = sec.header();
                    (x.sh_type, x.sh_addr, x.sh_size, x.sh_offset)
                })
                .find(|&(ty, lo, size, _)| {
                    ty != mortar_elf::consts::SHT_NOBITS
                        && lo <= addr
                        && addr.saturating_add(n) <= lo.saturating_add(size)
                })
                .map(|(_, lo, _, off)| (lo, off))?;
            let at = usize::try_from(off + (addr - base)).ok()?;
            output.get(at..at + usize::try_from(n).ok()?)
        };
        let rel32 = |addr: u64| {
            bytes(addr, 4).map(|w| i64::from(i32::from_le_bytes([w[0], w[1], w[2], w[3]])))
        };
        let call = f.rule == Rule::Field
            && f.reloc.as_deref() == Some("R_X86_64_PLT32")
            && f.addr.is_some_and(|place| {
                let Some(to) = rel32(place).map(|disp| (place + 4).wrapping_add_signed(disp))
                else {
                    return false;
                };
                section(b".plt.got").is_some_and(|(lo, hi)| (lo..hi).contains(&to))
                    && bytes(to, 2) == Some(&[0xff, 0x25][..])
                    && rel32(to + 2).is_some_and(|disp| (to + 6).wrapping_add_signed(disp) % 8 != 0)
            });
        if odd || call {
            return Some(
                "A31: GNU ld's x86-64 static-pie .plt.got entry of an undefined weak function jumps through its GOT slot plus 1 (dead behind the caller's null test: crtbeginS, a thread_local's init guard)",
            );
        }
    }
    if (bfd || mold)
        && spie
        && machine == mortar_elf::consts::EM_X86_64
        && f.rule == Rule::Ifunc(3)
        && m.starts_with("takes the function its IRELATIVE record resolves to")
    {
        return Some(
            "A32: GNU ld and mold give an x86-64 static-pie IFUNC two addresses: an address word the resolved function, a PC-relative use the IPLT entry (dyn.md W3)",
        );
    }
    if mold
        && f.rule == Rule::EhFrame(3)
        && m.ends_with("differs in its personality routine")
        && let Some(found) = f.found
        && section(b".plt").is_some_and(|(lo, hi)| (lo..hi).contains(&found))
    {
        return Some(
            "A28: mold points a CIE's absolute personality at a PLT entry it does not make canonical (two addresses of the routine)",
        );
    }
    if lld
        && f.rule == Rule::EhFrame(5)
        && m == "the search table is not sorted by initial location"
        && let Some(at) = f.addr
        && section(b".eh_frame_hdr").is_some_and(|(lo, _)| at < lo)
    {
        return Some(
            "A29: lld sorts .eh_frame_hdr's table by unsigned offset: an FDE below the table comes last, where the unwinder's search cannot find it",
        );
    }
    None
}

const A2: &str = "A2: mold gives an alias of an IFUNC the value 2^32 + x";

/// A1 and A2: mold's IFUNC symbols (spec/plumb.md §10.2).
fn ifunc_alias(output: &[u8], f: &Finding) -> Option<&'static str> {
    let ifunc_value = f.rule == Rule::SymbolValue && f.message.starts_with("IFUNC symbol");
    if ifunc_value && f.found.is_some_and(|v| v >= 1 << 32) {
        return Some(A2);
    }
    if !ifunc_value && f.rule != Rule::Structure(8) {
        return None;
    }
    let addr = if ifunc_value { f.found? } else { f.addr? };
    if addr >= 1 << 32 {
        return Some(A2);
    }
    let h = mortar_elf::FileHeader::parse(output).ok()?;
    let table = mortar_elf::SectionTable::parse(output, &h).ok()?;
    // ELF32: 2^32 + x is x, 16 bytes before .plt.got (spec/plumb.md §10.2).
    let plt_got =
        table.iter().find(|s| s.name().is_ok_and(|n| n == b".plt.got")).map(|s| s.header().sh_addr);
    if h.class == mortar_elf::Class::Elf32 && plt_got.is_some_and(|p| p.wrapping_sub(16) == addr) {
        return Some(A2);
    }
    if ifunc_value {
        return None;
    }
    let in_plt = table.iter().any(|s| {
        s.name().is_ok_and(|n| n == b".plt.got" || n == b".iplt" || n == b".plt")
            && (s.header().sh_addr..s.header().sh_addr.saturating_add(s.header().sh_size))
                .contains(&addr)
    });
    in_plt.then_some(
        "A1: mold's canonical IFUNC symbol is in .plt.got but names .text with the resolver's size",
    )
}

/// The id of an anomaly text: `"A9"` of `"A9: ..."`.
pub fn id(text: &str) -> &str {
    text.split_once(':').map_or(text, |(id, _)| id)
}
