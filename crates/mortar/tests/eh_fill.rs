//! WP-177's fill of a `.gcc_except_table` field against a key only a
//! dropped COMDAT copy defines (`spec/resolve.md` §6.3), on assembled
//! AArch64 objects: the field is 0 only where the scan judged it (a
//! mention COMDAT demoted, in an LSDA whose FDE is dead); another file's
//! honest undefined weak mention of such a key is the undefined weak value
//! of every other section, as before WP-177 (and as GNU ld writes it).

use std::path::Path;
use std::process::Command;
use std::time::Duration;

use mortar_testutil::target::Arch;
use mortar_testutil::{Outcome, Scratch, run_with, tools_required};

const MORTAR: &str = env!("CARGO_BIN_EXE_mortar");
const AARCH64: Arch = Arch::Aarch64;

/// `_start` exits 0.
const M: &str = ".text\n.globl _start\n_start:\n  mov x0, #0\n  mov x8, #93\n  svc #0\n";
/// The kept copy of group `g`: `g` only.
const G1: &str = ".section .text.g,\"axG\",@progbits,g,comdat\n.weak g\ng:\n  ret\n";
/// The kept copy of group `h`: `h` only.
const H1: &str = ".section .data.h,\"awG\",@progbits,h,comdat\n.weak h\nh:\n  .xword 0\n";
/// A dropped copy of `h` that also defines `kh`: only it defines `kh`.
const H2: &str =
    ".section .data.h,\"awG\",@progbits,h,comdat\n.weak h\n.weak kh\nh:\nkh:\n  .xword 0\n";

/// A dropped copy of `g` (after `G1`) whose `g` has an FDE with an LSDA at
/// the table's start (the FDE is dead: its code is the dropped copy's),
/// and `hx`, a weak `g` member only this copy defines (its mention is
/// demoted, §6.2). The table: a sentinel, then `fields`, then a sentinel.
fn x(fields: &str) -> String {
    format!(
        ".section .text.g,\"axG\",@progbits,g,comdat\n.weak g\n.weak hx\n.type g, @function\n\
         g:\nhx:\n  .cfi_startproc\n  .cfi_lsda 0x1b, .Lg\n  ret\n  .cfi_endproc\n\
         .weak kh\n\
         .section .gcc_except_table,\"a\",@progbits\n.p2align 3\n.Lg:\n\
         \x20 .xword 0x1111111111111111\n{fields}  .xword 0x2222222222222222\n"
    )
}

/// The demoted mention's field and the honest weak mention's (`kh`, which
/// this file does not define).
const DEMOTED: &str = "  .xword hx + 0x11223344\n";
const HONEST: &str = "  .xword kh + 0x11223344\n";

fn tools() -> bool {
    if AARCH64.tool("gcc").is_none() || AARCH64.tool("ld.bfd").is_none() {
        assert!(!tools_required(), "gcc and ld.bfd are required");
        return false;
    }
    true
}

fn run(c: &mut Command) -> Outcome {
    run_with(c, b"", Duration::from_mins(2))
}

/// Assembles `(name, text)` in `dir`.
fn assemble(dir: &Path, name: &str, text: &str) {
    std::fs::write(dir.join(name), text).unwrap();
    let o = run(Command::new(AARCH64.tool("gcc").unwrap())
        .args(["-c", name, "-o", &format!("{name}.o")])
        .current_dir(dir));
    assert!(o.success(), "gcc -c {name}: {}", o.stderr_text());
}

const LINE: [&str; 9] =
    ["-maarch64linux", "-static", "-e", "_start", "m.s.o", "g1.s.o", "h1.s.o", "h2.s.o", "x.s.o"];

/// The output's `.gcc_except_table`.
fn except_table(path: &Path) -> Vec<u8> {
    let b = std::fs::read(path).expect("the output");
    let h = mortar_elf::FileHeader::parse(&b).expect("an ELF header");
    let t = mortar_elf::SectionTable::parse(&b, &h).expect("section headers");
    let s = t.iter().find(|s| s.name().ok() == Some(b".gcc_except_table".as_slice()));
    let s = s.expect("a .gcc_except_table");
    let sh = s.header();
    b[sh.sh_offset as usize..(sh.sh_offset + sh.sh_size) as usize].to_vec()
}

/// The table's 8-byte words.
fn words(t: &[u8]) -> Vec<u64> {
    t.chunks_exact(8).map(|c| u64::from_le_bytes(c.try_into().unwrap())).collect()
}

/// Links `x(fields)` with mortar (no verifier: §6.3's dead-owner 0 is a
/// verifier finding, `comdat-eh-dead.test.pending`) and, when `bfd`, with
/// GNU ld: the tables' words.
fn link(label: &str, fields: &str, bfd: bool) -> (Vec<u64>, Option<Vec<u64>>) {
    let s = Scratch::new(label);
    for (n, t) in [("m.s", M), ("g1.s", G1), ("h1.s", H1), ("h2.s", H2), ("x.s", &x(fields))] {
        assemble(s.path(), n, t);
    }
    let out = s.join("mortar.out");
    let o = run(Command::new(MORTAR).args(LINE).arg("-o").arg(&out).current_dir(s.path()));
    assert!(o.success(), "{label}: mortar: {}", o.stderr_text());
    let m = words(&except_table(&out));
    let g = bfd.then(|| {
        let out = s.join("bfd.out");
        let ld = AARCH64.tool("ld.bfd").unwrap();
        let o = run(Command::new(ld).args(LINE).arg("-o").arg(&out).current_dir(s.path()));
        assert!(o.success(), "{label}: ld.bfd: {}", o.stderr_text());
        words(&except_table(&out))
    });
    (m, g)
}

#[test]
fn wp177_honest_weak_mention_beside_a_demoted_one() {
    const S1: u64 = 0x1111_1111_1111_1111;
    const S2: u64 = 0x2222_2222_2222_2222;
    if !tools() {
        return;
    }
    // The honest mention alone: S = 0 plus the addend, GNU ld's value and
    // the value before WP-177 (no event, no zero).
    let (alone, bfd) = link("eh-fill-honest", HONEST, true);
    assert_eq!(alone, [S1, 0x1122_3344, S2]);
    assert_eq!(Some(&alone), bfd.as_ref());
    // Beside a demoted mention in the same LSDA, whose FDE is dead: the
    // demoted field is 0 (§6.3, the owner dead), the honest one unchanged.
    let (both, _) = link("eh-fill-both", &format!("{DEMOTED}{HONEST}"), false);
    assert_eq!(both, [S1, 0, 0x1122_3344, S2]);
}
