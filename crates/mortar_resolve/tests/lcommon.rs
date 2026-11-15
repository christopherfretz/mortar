//! `spec/resolve.md` §3.3 (`SHN_X86_64_LCOMMON`, request R-16 of
//! `spec/x64.md`): the index is x86-64's large common on x86-64 only, and
//! staged there as a named refusal until `.lbss` exists (`spec/x64.md`
//! X-52's staging note); on AArch64 it is any reserved index (§2).

mod support;

use std::process::Command;

use mortar_core::par::Pool;
use mortar_diag::Style;
use mortar_elf::Machine;
use mortar_resolve::options::{Entry, Options};
use mortar_resolve::resolve;
use mortar_testutil::target::Arch;
use mortar_testutil::{Scratch, tool};
use support::inputs::{Item, build};

/// Rewrites every `SHN_COMMON` symbol of an ELF64 little-endian object to
/// `SHN_X86_64_LCOMMON` (0xff02).
fn to_reserved(bytes: &mut [u8]) {
    let u16_at = |b: &[u8], o: usize| u16::from_le_bytes([b[o], b[o + 1]]);
    let u32_at = |b: &[u8], o: usize| u32::from_le_bytes(b[o..o + 4].try_into().unwrap());
    let u64_at = |b: &[u8], o: usize| u64::from_le_bytes(b[o..o + 8].try_into().unwrap()) as usize;
    let (shoff, shentsize, shnum) =
        (u64_at(bytes, 0x28), usize::from(u16_at(bytes, 0x3a)), usize::from(u16_at(bytes, 0x3c)));
    let mut changed = 0;
    for i in 0..shnum {
        let sh = shoff + i * shentsize;
        if u32_at(bytes, sh + 4) != 2 {
            continue; // SHT_SYMTAB
        }
        let (off, size) = (u64_at(bytes, sh + 0x18), u64_at(bytes, sh + 0x20));
        for s in (off..off + size).step_by(24) {
            if u16_at(bytes, s + 6) == 0xfff2 {
                bytes[s + 6..s + 8].copy_from_slice(&0xff02u16.to_le_bytes());
                changed += 1;
            }
        }
    }
    assert_eq!(changed, 1, "one common symbol");
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

fn assemble(asm: &str, src: &str, scratch: &Scratch, name: &str) -> Option<std::path::PathBuf> {
    let asm = tool(asm)?;
    let s = scratch.write(&format!("{name}.s"), src);
    let o = scratch.join(format!("{name}.o"));
    let status = Command::new(asm).arg(&s).arg("-o").arg(&o).status().unwrap();
    assert!(status.success());
    Some(o)
}

// spec: resolve.md §3.3 (`SHN_X86_64_LCOMMON` machine-gated; refused by
// name on x86-64 until `.lbss` exists)
#[test]
fn s3_3_lcommon_is_x86_64_only() {
    let scratch = Scratch::new("lcommon");
    if let Some(x) = assemble(&Arch::X86_64.gnu("as"), ".largecomm lc,8,8\n", &scratch, "x64") {
        let e = errors(Machine::X86_64, x);
        assert_eq!(e.len(), 1, "{e:?}");
        assert!(e[0].contains("unsupported feature: SHN_X86_64_LCOMMON"), "{e:?}");
    }
    if let Some(a) = assemble(&Arch::Aarch64.gnu("as"), ".comm lc,8,8\n", &scratch, "a64") {
        let mut bytes = std::fs::read(&a).unwrap();
        to_reserved(&mut bytes);
        let a = scratch.write("a64r.o", bytes);
        let e = errors(Machine::AArch64, a);
        assert_eq!(e.len(), 1, "{e:?}");
        assert!(e[0].contains("unsupported feature: SHN 0xff02"), "{e:?}");
    }
}
