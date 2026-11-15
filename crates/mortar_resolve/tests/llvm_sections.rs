//! `spec/resolve.md` §10, the `SHT_LLVM_*` rows on real objects (WP-034):
//! `.deplibs` (`SHT_LLVM_DEPENDENT_LIBRARIES`) is consumed, its library
//! specifiers not honoured, as GNU ld and mold do; another `SHT_LLVM_*`
//! type is refused, by its name (`SHT_LLVM_ODRTAB`), not its number.

mod support;

use std::process::Command;

use mortar_core::par::Pool;
use mortar_diag::Style;
use mortar_elf::Machine;
use mortar_resolve::options::{Entry, Options};
use mortar_resolve::resolve;
use mortar_testutil::{Scratch, tool};
use support::inputs::{Item, build};

/// Sets the `sh_type` of the section named `name` of an ELF64
/// little-endian object to `sh_type`.
fn retype(bytes: &mut [u8], name: &[u8], sh_type: u32) {
    let u16_at = |b: &[u8], o: usize| usize::from(u16::from_le_bytes([b[o], b[o + 1]]));
    let u32_at = |b: &[u8], o: usize| u32::from_le_bytes(b[o..o + 4].try_into().unwrap()) as usize;
    let u64_at = |b: &[u8], o: usize| u64::from_le_bytes(b[o..o + 8].try_into().unwrap()) as usize;
    let (shoff, shentsize, shnum, shstrndx) =
        (u64_at(bytes, 0x28), u16_at(bytes, 0x3a), u16_at(bytes, 0x3c), u16_at(bytes, 0x3e));
    let strtab = u64_at(bytes, shoff + shstrndx * shentsize + 0x18);
    let mut changed = 0;
    for i in 0..shnum {
        let sh = shoff + i * shentsize;
        let at = strtab + u32_at(bytes, sh);
        if bytes[at..].starts_with(name) && bytes[at + name.len()] == 0 {
            bytes[sh + 4..sh + 8].copy_from_slice(&sh_type.to_le_bytes());
            changed += 1;
        }
    }
    assert_eq!(changed, 1, "one section {}", String::from_utf8_lossy(name));
}

/// The rendered errors of resolving `object` alone.
fn errors(machine: Machine, object: std::path::PathBuf) -> Vec<String> {
    let inputs = build(machine, &[Item::Object(object)]).unwrap();
    let pool = Pool::new(1);
    let view = mortar_resolve::InputsView::new(&inputs, &pool);
    let opts = Options { entry: Entry::None, ..Options::default() };
    let r = resolve(&view, &opts, &pool);
    mortar_resolve::tables::diagnostics(&inputs, &view, &opts, &r)
        .iter()
        .map(|(_, d)| d.render(&Style::plain("ld")))
        .collect()
}

/// GNU as of the host's machine (AArch64 here; the rows are
/// machine-independent).
fn assemble(src: &str, scratch: &Scratch, name: &str) -> Option<(Machine, Vec<u8>)> {
    let (asm, machine) = if cfg!(target_arch = "aarch64") {
        ("as", Machine::AArch64)
    } else {
        ("x86_64-linux-gnu-as", Machine::X86_64)
    };
    let asm = tool(asm)?;
    let s = scratch.write(&format!("{name}.s"), src);
    let o = scratch.join(format!("{name}.o"));
    let status = Command::new(asm).arg(&s).arg("-o").arg(&o).status().unwrap();
    assert!(status.success());
    Some((machine, std::fs::read(&o).unwrap()))
}

// spec: resolve.md §10 (`SHT_LLVM_DEPENDENT_LIBRARIES` is consumed; the
// other `SHT_LLVM_*` types are refused by name)
#[test]
fn s10_deplibs_consumed_other_llvm_types_refused_by_name() {
    let scratch = Scratch::new("llvm-sections");
    // `.deplibs` as Clang writes it: "MS", entry size 1, one specifier.
    let src = ".section .deplibs,\"MS\",%progbits,1\n.asciz \"pthread\"\n\
               .section .llvm.odrtab,\"\",%progbits\n.byte 0\n";
    let Some((machine, bytes)) = assemble(src, &scratch, "llvm") else { return };

    let mut deplibs = bytes.clone();
    retype(&mut deplibs, b".deplibs", 0x6fff_4c04);
    retype(&mut deplibs, b".llvm.odrtab", 1); // SHT_PROGBITS: links
    let e = errors(machine, scratch.write("deplibs.o", deplibs));
    assert!(e.is_empty(), "{e:?}");

    let mut odrtab = bytes;
    retype(&mut odrtab, b".deplibs", 0x6fff_4c04);
    retype(&mut odrtab, b".llvm.odrtab", 0x6fff_4c00);
    let e = errors(machine, scratch.write("odrtab.o", odrtab));
    assert_eq!(e.len(), 1, "{e:?}");
    assert!(e[0].contains("unsupported section type: SHT_LLVM_ODRTAB"), "{e:?}");
    assert!(e[0].contains(".llvm.odrtab"), "{e:?}");
}
