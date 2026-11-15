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
    if mold { ifunc_alias(output, f) } else { None }
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
