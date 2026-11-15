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
    if !mold || f.rule != Rule::Structure(8) {
        return None;
    }
    let addr = f.addr?;
    if addr >= 1 << 32 {
        return Some("A2: mold gives an alias of an IFUNC the value 2^32 + x");
    }
    let h = mortar_elf::FileHeader::parse(output).ok()?;
    let table = mortar_elf::SectionTable::parse(output, &h).ok()?;
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
