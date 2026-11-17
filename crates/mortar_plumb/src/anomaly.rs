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
    if let Rule::Rel(n) = f.rule {
        return relocatable(linker, n, &f.message);
    }
    if f.rule == Rule::GotSlot
        && f.symbol.as_deref().is_some_and(|s| s.starts_with('.'))
        && f.found.zip(f.expected).is_some_and(|(found, want)| found < want)
    {
        return Some(
            "A6: GNU ld, lld and mold fill the GOT slot of a section symbol plus an addend without the addend",
        );
    }
    if bfd && a59(output, f) {
        return Some(
            "A59: GNU ld defines the program's own _end, _edata, __bss_start a needed library defines with the library's type and size (STT_OBJECT, 1 byte past the end of its section) (dyn.md D-21 item 2, §13.1)",
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
    if let Some(a) = lld.then(|| wrap_lld(f)).or_else(|| bfd.then(|| defsym_bfd(f))).flatten() {
        return Some(a);
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
    if mold
        && f.rule == Rule::Field
        && f.reloc.as_deref() == Some("R_X86_64_PLT32")
        && f.message.ends_with("(spec/plumb.md §5.1a)")
    {
        return Some(
            "A35: mold links an x86-64 PLT32 call to an absolute symbol in a PIE or static-pie as a PC-relative call, right at load bias 0 only (spec/plumb.md §5.1a)",
        );
    }
    // §5.8, a non-alloc word against a name only a shared library defines
    // (WP-104): each reference's convention, from that reference only.
    {
        use crate::reloc::dsoword as w;
        let x64 = output.get(18..20) == Some(&[62, 0][..]);
        let exec = mortar_elf::FileHeader::parse(output)
            .is_ok_and(|h| h.e_type == mortar_elf::consts::ET_EXEC);
        match w::holds(f) {
            Some(h) if bfd && (x64 || exec) && h == w::HOLDS_PLT => {
                return Some(
                    "A37: GNU ld writes a function's ordinary PLT entry (x86-64; AArch64 -no-pie) into a non-alloc word against a function or IFUNC only a shared library defines (spec/plumb.md §5.8)",
                );
            }
            Some(h) if lld && h == w::HOLDS_NO_ADDEND => {
                return Some(
                    "A38: lld drops the addend of a non-alloc absolute word against a symbol only a shared library defines (spec/plumb.md §5.8)",
                );
            }
            Some(h) if mold && h == w::HOLDS_ST_VALUE => {
                return Some(
                    "A39: mold writes the library's st_value into a non-alloc word against a symbol only a shared library defines (spec/plumb.md §5.8)",
                );
            }
            Some(h) if mold && h == w::HOLDS_PLT => {
                return Some(
                    "A40: mold writes a function's PLT entry into a non-alloc word against a function or IFUNC only a shared library defines (spec/plumb.md §5.8)",
                );
            }
            Some(h) if (bfd || mold) && h == w::HOLDS_TLS_BASE => {
                return Some(
                    "A42: GNU ld and mold write A minus the output's PT_TLS address into a non-alloc DTP-offset word against a shared library's TLS symbol (spec/plumb.md §5.8)",
                );
            }
            _ => {}
        }
        if (lld || mold)
            && f.rule == Rule::Dyn(12)
            && f.message.starts_with(w::TLS_WORD)
            && f.message.ends_with(w::TLS_WORD_END)
        {
            return Some(
                "A41: lld and mold link a non-TLS data word in a non-alloc section against a shared library's STT_TLS symbol, which GNU ld and mortar refuse (spec/plumb.md §5.8)",
            );
        }
        if lld && f.rule == Rule::Field && f.message.ends_with(w::WEAK_ZERO) {
            return Some(
                "A43: lld writes A - P (S = 0) into a non-alloc AArch64 PC-relative word against an undefined weak, where AAELF64 makes S the place (spec/plumb.md §4.4, §5.8)",
            );
        }
    }
    if lld && d(6) && m == crate::dynverify::SYMBOLIC {
        return Some(
            "A44: lld sets DF_SYMBOLIC in an executable's DT_FLAGS under -Bsymbolic, a flag the loader ignores there (dyn.md D-30)",
        );
    }
    if (lld || mold) && f.rule == Rule::Dyn(6) && f.message.ends_with("(D-13: the link is refused)")
    {
        return Some(
            "A36: lld and mold link a used shared object's strong reference that no member of N ∪ U satisfies and leave the library that defines it out of DT_NEEDED; the program fails at load (dyn.md D-13, W16)",
        );
    }
    if (lld || mold) && d(7) && m.ends_with(crate::dyncheck::ROOT_IMPORT) {
        return Some(
            "A47: lld and mold write no .dynsym entry for a root (-u, EXTERN) that only a needed shared library defines; GNU ld and mortar import it (dyn.md D-21 item 1)",
        );
    }
    if mold && f.rule == crate::inset::EXTRA && m.ends_with(crate::inset::VERSIONED_PLAIN) {
        return Some(
            "A49: mold extracts an archive member for a shared object's versioned undefined reference (`name@V`) by the member's plain `name`; GNU ld, lld and mortar do not (resolve §9.2)",
        );
    }
    if lld
        && ((f.rule == crate::inset::EXTRA && m.ends_with(crate::inset::UNNEEDED_AS_NEEDED))
            || ((d(6) || d(7)) && m.ends_with(crate::dyncheck::UNREACHED_NEEDS)))
    {
        return Some(
            "A70: lld extracts an archive member for the references of an --as-needed shared object not in N (unneeded, or used), and what the member needs follows; GNU ld, mold and mortar do not (dyn.md D-12)",
        );
    }
    if lld && d(6) && m.ends_with(crate::dyncheck::KEPT_NEEDED) {
        return Some(
            "A71: lld drops an --as-needed library needed at stage 1 when a later stage's member defines what it gave; GNU ld, mold and mortar keep it (dyn.md D-12 row 2a: N only grows)",
        );
    }
    if mold && d(6) && m.ends_with(crate::dyncheck::ROOT_NEEDED) {
        return Some(
            "A48: mold makes an --as-needed library needed when only a root (-u, -e, EXTERN) names a definition of it (dyn.md D-11 item 2)",
        );
    }
    if bfd && d(25) && m == crate::shared::SPARE_NONE {
        return Some(
            "A46: GNU ld writes a spare R_AARCH64_NONE record (all fields 0) in .rela.dyn for each local general-dynamic pair of an AArch64 shared object; glibc skips type 0 (spec/plumb.md §5.8)",
        );
    }
    if d(24) && m.ends_with("(D-43a, W30)") {
        return Some(
            "A45: GNU ld, lld and mold copy writable data a library binds to itself into the executable: two objects (dyn.md D-43a, W30)",
        );
    }
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
    // WP-165: D-25's branch cells (WP-155's measurement).
    if mold
        && d(7)
        && (m.ends_with(crate::weakclass::STRICT_DYN)
            || m.ends_with(crate::weakclass::STRICT_SPLIT))
    {
        return Some(
            "A55: mold writes no .dynsym entry for an undefined weak key reached by branches (alone, or beside an unfillable use): the call goes to 0 or is skipped whatever a later library defines (dyn.md D-25, §13.1)",
        );
    }
    if bfd
        && d(7)
        && m.ends_with(crate::weakclass::STRICT_SPLIT)
        && mortar_elf::FileHeader::parse(output)
            .is_ok_and(|h| h.e_machine == mortar_elf::consts::EM_X86_64)
    {
        return Some(
            "A56: GNU ld on x86-64 writes no .dynsym entry for an undefined weak key reached by a branch and an unfillable use in -no-pie (its -z nodynamic-undefined-weak form): the call goes to 0 under LD_PRELOAD too (dyn.md D-25, §13.1's named difference)",
        );
    }
    // D-21 item 2's seven names (D-110 item 2, WP-150): the name a
    // finding is about.
    let quoted = |m: &str| m.split('\'').nth(1).map(str::to_string);
    let seven = |n: Option<String>| n.is_some_and(|n| crate::shared::SEVEN.contains(&n.as_str()));
    if mold
        && ((d(19) && m.ends_with("is in the export set and not defined in .dynsym (D-110)"))
            || (d(7)
                && (m.contains("is defined by the program and referenced by a needed or used object, and is not exported")
                    || m.contains("an object references it and the line's -E or --dynamic-list exports it, and it is not exported")))
            || (d(17) && m.starts_with("view B (an interposer defines '")))
        && seven(f.symbol.clone().or_else(|| quoted(m)))
    {
        return Some(
            "A57: mold exports none of D-21 item 2's seven linker-defined names (_end, _edata, __bss_start, ...): its library binds a referenced _end to its own and its executable exports none a library references or defines (dyn.md D-110 item 2, D-21 item 2, §13.1, §17.6)",
        );
    }
    let gnu_own = |n: Option<String>| {
        n.is_some_and(|n| {
            matches!(
                n.as_str(),
                "__etext" | "__end__" | "__bss_start__" | "__bss_end__" | "_bss_end__"
            )
        })
    };
    if bfd
        && ((d(19) && m.contains(") is in .dynsym and not in the export set"))
            || (d(25) && m.ends_with("which the line binds (D-112)"))
            || (d(7) && m.ends_with("which does not hold it")))
        && gnu_own(f.symbol.clone().or_else(|| quoted(m)))
        && !pie_flag(output)
    {
        return Some(
            "A58: GNU ld's shared object exports __etext and its AArch64 names (__end__, __bss_start__, ...) an object references, preemptible, where mortar and lld keep them unexported (dyn.md D-110 item 2, §17.6)",
        );
    }
    let start_stop = |n: Option<String>| {
        n.is_some_and(|n| {
            n.strip_prefix("__start_")
                .or_else(|| n.strip_prefix("__stop_"))
                .is_some_and(|x| !x.is_empty())
        })
    };
    if lld
        && d(19)
        && m.contains(") is in .dynsym and not in the export set (a version script or --exclude-libs makes it local)")
        && start_stop(f.symbol.clone().or_else(|| quoted(m)))
    {
        return Some(
            "A61: lld keeps __start_SEC and __stop_SEC exported (GLOBAL PROTECTED) under a version script's local: * (dyn.md D-114)",
        );
    }
    if bfd
        && d(7)
        && m.contains("is exported under -E, and no needed library references or defines it and no object's reference")
        && seven(quoted(m))
    {
        return Some(
            "A60: GNU ld's executable exports _edata, __bss_start and _end under -E with no reference (dyn.md D-21 item 2, §13.1's named difference)",
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
    // `DF_1_PIE`: a shared object (no `.interp` either) is no static-pie
    // (WP-166: GNU ld's libraries drew A30/A34 for the GOT slot of `_end`).
    let spie = header.as_ref().is_some_and(|h| h.e_type == mortar_elf::consts::ET_DYN)
        && section(b".dynamic").is_some()
        && section(b".interp").is_none()
        && pie_flag(output);
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
    if bfd && spie && exports(output) {
        let tls = machine == mortar_elf::consts::EM_X86_64
            && d(14)
            && m.starts_with("a TLS record (type 18) in a static-pie");
        let abs = d(9)
            && (m.ends_with("(a record moves a value that is not an address of the executable)")
                || m == "a RELATIVE record on a word of an input section that no input relocation makes an address of the executable");
        if tls || abs {
            return Some(
                "A34: GNU ld's static-pie with --export-dynamic: RELATIVE records on words that hold an exported absolute symbol (glibc's _nl_current_*_used), and on x86-64 TPOFF64 records the self-relocation applies before the thread pointer exists (SIGSEGV at start-up)",
            );
        }
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

/// Whether `output`'s `.dynsym` (ELF64) holds a defined entry that is not
/// a section symbol: a reference's static-pie linked with exports.
fn exports(output: &[u8]) -> bool {
    let Ok(h) = mortar_elf::FileHeader::parse(output) else { return false };
    let Ok(table) = mortar_elf::SectionTable::parse(output, &h) else { return false };
    let Some(d) = table.iter().find(|s| s.name().is_ok_and(|n| n == b".dynsym")) else {
        return false;
    };
    let x = d.header();
    let (Ok(at), Ok(size)) = (usize::try_from(x.sh_offset), usize::try_from(x.sh_size)) else {
        return false;
    };
    let Some(b) = output.get(at..at.saturating_add(size)) else { return false };
    b.chunks_exact(24).skip(1).any(|e| {
        let shndx = u16::from_le_bytes([e[6], e[7]]);
        shndx != 0 && e[4] & 0xf != mortar_elf::consts::STT_SECTION
    })
}

/// A59's shape: "... is outside its section" on one of the seven that
/// `.dynsym` defines as `STT_OBJECT`.
fn a59(output: &[u8], f: &Finding) -> bool {
    f.rule == Rule::Structure(8)
        && f.message.contains(") is outside its section")
        && f.message
            .split('\'')
            .nth(1)
            .is_some_and(|n| crate::shared::SEVEN.contains(&n) && dyn_sym_object(output, n))
}

/// `.dynsym` defines `name` as `STT_OBJECT` (A59's form).
fn dyn_sym_object(output: &[u8], name: &str) -> bool {
    let Ok(h) = mortar_elf::FileHeader::parse(output) else { return false };
    let Ok(table) = mortar_elf::SectionTable::parse(output, &h) else { return false };
    let find =
        |n: &[u8]| table.iter().find(|s| s.name().is_ok_and(|x| x == n)).map(|s| *s.header());
    let (Some(d), Some(st)) = (find(b".dynsym"), find(b".dynstr")) else { return false };
    let slice = |o: u64, n: u64| {
        let (Ok(o), Ok(n)) = (usize::try_from(o), usize::try_from(n)) else { return None };
        output.get(o..o.saturating_add(n))
    };
    let (Some(b), Some(strs)) = (slice(d.sh_offset, d.sh_size), slice(st.sh_offset, st.sh_size))
    else {
        return false;
    };
    b.chunks_exact(24).skip(1).any(|e| {
        let at = u32::from_le_bytes([e[0], e[1], e[2], e[3]]) as usize;
        let shndx = u16::from_le_bytes([e[6], e[7]]);
        let n = strs.get(at..).and_then(|r| r.split(|&c| c == 0).next());
        shndx != 0 && e[4] & 0xf == mortar_elf::consts::STT_OBJECT && n == Some(name.as_bytes())
    })
}

/// `DT_FLAGS_1` has `DF_1_PIE` (an executable, a static-pie among them).
fn pie_flag(output: &[u8]) -> bool {
    let Ok(h) = mortar_elf::FileHeader::parse(output) else { return false };
    let Ok(table) = mortar_elf::SectionTable::parse(output, &h) else { return false };
    let Some(d) = table.iter().find(|s| s.name().is_ok_and(|n| n == b".dynamic")) else {
        return false;
    };
    let x = d.header();
    let (Ok(at), Ok(size)) = (usize::try_from(x.sh_offset), usize::try_from(x.sh_size)) else {
        return false;
    };
    let Some(b) = output.get(at..at.saturating_add(size)) else { return false };
    b.chunks_exact(16).any(|e| {
        let tag = u64::from_le_bytes(e[..8].try_into().unwrap_or_default());
        let val = u64::from_le_bytes(e[8..].try_into().unwrap_or_default());
        tag == crate::dynout::DT_FLAGS_1 && val & crate::dynout::DF_1_PIE != 0
    })
}

/// lld's forms of `--wrap` (§4.4a, WP-170), by the tags the rules add.
fn wrap_lld(f: &Finding) -> Option<&'static str> {
    use crate::wrap::{E13, ENTRY_WRAPPED, REAL_DROPPED, REAL_STRONG, WRAPPED_ONLY};
    let m = f.message.as_str();
    if (matches!(f.rule, Rule::Field | Rule::Overflow | Rule::Misaligned)
        || (f.rule == Rule::Dyn(17) && m.starts_with("view B (an interposer defines '")))
        && m.ends_with(E13)
    {
        return Some(
            "A80: lld wraps a reference in SYM's defining object under --wrap=SYM: the call binds to __wrap_SYM (resolve.md §7.1 E13)",
        );
    }
    if matches!(f.rule, Rule::Dyn(4 | 14)) && m.ends_with(ENTRY_WRAPPED) {
        return Some(
            "A82: lld makes the entry point __wrap_ENTRY under --wrap=ENTRY: it redirects the root -e (resolve.md §7.1, layout.md L-102)",
        );
    }
    if f.rule == Rule::Map(12) && m.ends_with(WRAPPED_ONLY) {
        return Some(
            "A83: lld extracts an archive member that defines SYM though only references --wrap=SYM binds to __wrap_SYM name it (resolve.md §7.1)",
        );
    }
    if f.rule == Rule::SymbolTable(3) && m.ends_with(REAL_DROPPED) {
        return Some(
            "A84: lld leaves an input's definition of __real_SYM out of .symtab under --wrap=SYM (resolve.md §7.1)",
        );
    }
    if f.rule == Rule::SymbolTable(3) && m.ends_with(REAL_STRONG) {
        return Some(
            "A81: lld writes a weak SYM STB_GLOBAL in .symtab when a strong undefined __real_SYM binds to it under --wrap=SYM (resolve.md §7.1)",
        );
    }
    None
}

/// GNU ld's forms of `--defsym` (§4.4b, WP-170 part 2), by the tags the
/// rules add.
fn defsym_bfd(f: &Finding) -> Option<&'static str> {
    use crate::defsym::{NUMBER_MOVED, OFFSET_ABS, OFFSET_UNMOVED, REPLACED_SIZE};
    let m = f.message.as_str();
    if (f.rule == Rule::SymbolTable(3) && m.ends_with(OFFSET_ABS))
        || (f.rule == Rule::Dyn(9) && m.ends_with(OFFSET_UNMOVED))
    {
        return Some(
            "A85: GNU ld writes --defsym SYM=B+N (or B-N) as SHN_ABS with B's type, not in B's section as STT_NOTYPE, and in a PIE gives a word holding its address no record: the program reads B+N without the load bias (resolve.md §7.2, §16)",
        );
    }
    if f.rule == Rule::SymbolTable(3) && m.ends_with(REPLACED_SIZE) {
        return Some(
            "A86: GNU ld gives a --defsym alias the size of the input definition it replaces, not 0 (resolve.md §7.2, §16)",
        );
    }
    if f.rule == Rule::SymbolTable(3) && m.ends_with(crate::defsym::EARLIER_TYPE) {
        return Some(
            "A88: GNU ld gives a SYM assigned by several --defsym the type of an earlier one, not the last one's (resolve.md §7.2)",
        );
    }
    if matches!(f.rule, Rule::Field | Rule::Dyn(9)) && m.ends_with(NUMBER_MOVED) {
        return Some(
            "A87: GNU ld adds the load base to a --defsym number in a PIE or static-pie: the program reads the number plus the bias (resolve.md §7.2, §16)",
        );
    }
    None
}

/// The id of an anomaly text: `"A9"` of `"A9: ..."`.
pub fn id(text: &str) -> &str {
    text.split_once(':').map_or(text, |(id, _)| id)
}

/// The anomalies of the references' `-r` outputs (stage R, §14; §10.2
/// A90-A102, A120-A122), by the rule and the message's form.
fn relocatable(linker: Format, n: u8, m: &str) -> Option<&'static str> {
    let (lld, mold) = (linker == Format::Lld, linker == Format::Mold);
    let a = match n {
        3 if mold && m.contains(" has an output record against symbol 0, not ") => {
            "A97: at -r mold writes a debug record whose target is a dropped copy against symbol 0"
        }
        3 if mold && m.contains("_NONE against ") && m.contains(".llvm.call-graph-profile") => {
            "A102: at -r mold copies .llvm.call-graph-profile and drops its relocation section"
        }
        4 if mold && m.starts_with("common ") && m.contains(" is SHN_ABS, not SHN_COMMON") => {
            "A90: at -r mold turns a common into an SHN_ABS definition of its alignment"
        }
        4 if mold && m.ends_with(" is not in .symtab") && m.contains('@') => {
            "A95: at -r mold strips .symver names to their base"
        }
        4 if mold && m.contains(" is defined in ") && m.ends_with("; no live input defines it") => {
            "A95: at -r mold strips .symver names to their base"
        }
        4 if lld && (m.starts_with("local wi.") || m.starts_with("local wt.")) && m.contains(" at 65521+") => {
            "A126: at -r lld keeps a dropped COMDAT copy's local group signature as an SHN_ABS local"
        }
        4 if mold && m.ends_with(" is not in .symtab") => {
            "A94: at -r mold drops an STB_GNU_UNIQUE member symbol"
        }
        4 if mold && m.contains(" is in .symtab 2 times") => {
            "A96: at -r mold writes two strong definitions of one name"
        }
        5 if lld && m.starts_with("output FDE at ") && m.contains(" comes from a removed FDE of ") => {
            "A91: at -r lld keeps the FDE of a dropped COMDAT copy"
        }
        6 if mold
            && ((m.starts_with("group ") && m.ends_with(" is not in the output"))
                || m.ends_with(" has SHF_GROUP and no group lists it")) =>
        {
            "A92: at -r mold drops non-COMDAT groups"
        }
        6 if mold && m.contains(" .rel") && m.ends_with(" is a group member without SHF_GROUP") => {
            "A121: at -r mold lists a member's relocation section in its group without SHF_GROUP"
        }
        7 if (lld || mold) && m.starts_with("EI_OSABI is 0, not 3") => {
            "A122: at -r lld and mold write STB_GNU_UNIQUE or STT_GNU_IFUNC symbols with EI_OSABI ELFOSABI_NONE"
        }
        2 if mold && m.contains(":(.ctors") && m.contains(" .init_array, of another name") => {
            "A123: at -r mold moves .ctors pieces into .init_array, dropping their priority suffix"
        }
        11 if mold && m.contains(" .init_array: its name is not its pieces' name") => {
            "A123: at -r mold moves .ctors pieces into .init_array, dropping their priority suffix"
        }
        4 if mold && m.starts_with("local ") && m.contains(" at 65521+") => {
            "A124: at -r mold makes the local signature symbols of groups SHN_ABS"
        }
        3 if lld && m.starts_with("output record R_") && m.contains("_NONE at ") && m.contains(" .eh_frame+") => {
            "A125: at -r lld leaves an R_*_NONE record in .rela.eh_frame where it removed an FDE"
        }
        10 if lld && m.starts_with("an input asks for an executable stack (") && m.ends_with(" is N") => {
            "A93: at -r lld writes .note.GNU-stack without SHF_EXECINSTR over an input that has it"
        }
        10 if mold && m.starts_with("the line asks for a build id (") => {
            "A98: at -r mold writes no .note.gnu.build-id under --build-id"
        }
        11 if mold && m.contains(" holds the SHF_GNU_RETAIN piece ") => {
            "A99: at -r mold merges an SHF_GNU_RETAIN piece into a section without SHF_GNU_RETAIN"
        }
        11 if lld && m.ends_with(": SHF_MERGE with sh_entsize 0") => {
            "A100: at -r lld merges SHF_MERGE pieces of different sh_entsize with sh_entsize 0"
        }
        11 if mold && (m.ends_with(": sh_link is 0") || m.contains(": SHF_LINK_ORDER pieces link to ")) => {
            "A101: at -r mold writes sh_link 0 on an SHF_LINK_ORDER section"
        }
        11 if lld && m.ends_with(" COMMON comes from no input section") => {
            "A120: at -r lld writes an empty NOBITS COMMON section that holds no symbol"
        }
        _ => return None,
    };
    Some(a)
}
