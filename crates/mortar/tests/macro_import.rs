//! `spec/image.md` I-47a (WP-249): a debug reference into a dropped COMDAT
//! copy of a non-alloc group member is filled against the keeper's copy
//! of the same name and size; otherwise I-47's tombstone.
//!
//! - [`i47a_branches`]: one hand-made link per architecture, a word per
//!   branch of I-47a's condition (a section symbol, a named local symbol
//!   at `st_value` != 0, an addend, a 64-bit field; the keeper's group
//!   lacks the name, has it with another size, has two members of the
//!   name, has it `SHF_ALLOC`; the dropped target is code; a group
//!   without `GRP_COMDAT`; the keeper's copy collected by
//!   `--gc-sections`). Each word is named by the tag its target starts
//!   with in the output's `.debug_macro`: no layout is assumed.
//! - [`gcc_g3_imports_name_gnu_lds_units`]: GCC 14 `-g3` (C, C++; DWARF 4
//!   and 5; AArch64 and x86-64), linked by mortar and by GNU ld from the
//!   driver's static line: every `DW_MACRO_import` of mortar's output
//!   names a macro unit whose contents equal those of the unit GNU ld's
//!   import names (readelf's rendering, strings resolved, the
//!   `.debug_line` offset dropped): offsets are compared through what
//!   they name, so the test holds whatever each linker's layout.
//!   `--incremental=full` gives the same units, imports included, as the
//!   plain link (one configuration per architecture).
//! - [`gdb_x86_64_macros`]: gdb-multiarch on `qemu-x86_64 -g`: `info
//!   macro` and `macro expand` in `b_f` answer as on GNU ld's output (the
//!   AArch64 sessions are the lane's, `tests/link/debug/macro-import-*`).
//!
//! The verifier is not run here: its §5.8 demands I-47's tombstone for
//! these references until WP-250 (the lane reports those R1 findings).

mod support;

use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::Duration;

use mortar_testutil::target::Arch;
use mortar_testutil::{Scratch, run_with, tool, tools_required};

const MORTAR: &str = env!("CARGO_BIN_EXE_mortar");

// ---------------------------------------------------------------------
// The ELF facts: one section's bytes, ELFCLASS32 and 64, little-endian.

fn section(path: &Path, want: &str) -> Vec<u8> {
    let b = std::fs::read(path).unwrap_or_else(|e| panic!("{}: {e}", path.display()));
    let is64 = b[4] == 2;
    let u16_at = |o: usize| usize::from(u16::from_le_bytes(b[o..o + 2].try_into().unwrap()));
    let u32_at = |o: usize| u32::from_le_bytes(b[o..o + 4].try_into().unwrap()) as usize;
    let u64_at =
        |o: usize| usize::try_from(u64::from_le_bytes(b[o..o + 8].try_into().unwrap())).unwrap();
    let word = |o: usize| if is64 { u64_at(o) } else { u32_at(o) };
    let (shoff, shentsize, shnum, shstrndx) = if is64 {
        (u64_at(0x28), u16_at(0x3a), u16_at(0x3c), u16_at(0x3e))
    } else {
        (u32_at(0x20), u16_at(0x2e), u16_at(0x30), u16_at(0x32))
    };
    let sh = |i: usize| shoff + i * shentsize;
    let (off_at, size_at) = if is64 { (0x18, 0x20) } else { (0x10, 0x14) };
    let strtab = word(sh(shstrndx) + off_at);
    for i in 1..shnum {
        let name = &b[strtab + u32_at(sh(i))..];
        let end = name.iter().position(|&c| c == 0).unwrap();
        if &name[..end] == want.as_bytes() {
            let (off, size) = (word(sh(i) + off_at), word(sh(i) + size_at));
            return b[off..off + size].to_vec();
        }
    }
    panic!("{}: no {want}", path.display());
}

fn find(hay: &[u8], needle: &[u8]) -> Option<usize> {
    hay.windows(needle.len()).position(|w| w == needle)
}

// ---------------------------------------------------------------------
// The branches, hand-made.

/// `.section` of a group member: `flags` beside `G`, `comdat` or not, a
/// GAS `unique` id for a second section of one name in one group.
fn grp(name: &str, flags: &str, sig: &str, comdat: bool, unique: Option<u32>) -> String {
    let c = if comdat { ",comdat" } else { "" };
    let u = unique.map_or(String::new(), |u| format!(",unique,{u}"));
    format!("    .section {name},\"{flags}G\",@progbits,{sig}{c}{u}\n")
}

/// A member: an 8-byte tag, 8 zero bytes (16 bytes), `pad` more zeros, a
/// label `l` at its start and, for the named case, a local `named` at 8.
fn member(head: &str, l: &str, tag: &str, pad: u32, named: Option<&str>) -> String {
    assert_eq!(tag.len(), 8);
    let named = named.map_or(String::new(), |n| format!("{n}:\n"));
    let pad = if pad == 0 { String::new() } else { format!("    .zero {pad}\n") };
    format!("{head}{l}:\n    .ascii \"{tag}\"\n{named}    .8byte 0\n{pad}")
}

/// The keeper's object: the first, so every group of `d.o` is dropped
/// but `g_nc`'s (no `GRP_COMDAT`: both copies stay).
fn keeper(arch: Arch) -> String {
    let start = match arch {
        Arch::Aarch64 => "    mov x8, #93\n    svc #0\n",
        Arch::X86_64 => "    movl $60, %eax\n    syscall\n",
        Arch::I386 => "    movl $1, %eax\n    int $0x80\n",
    };
    let mut s = format!(
        "    .text\n    .globl _start\n_start:\n{start}\
         \x20   .section .debug_macro,\"\",@progbits\n    .ascii \"PADPADPADPADPAD.\"\n"
    );
    s += &member(&grp(".debug_macro", "", "g_same", true, None), ".Lk1", "SAMEkeep", 0, None);
    s += &member(&grp(".debug_macro", "", "g_named", true, None), ".Lk2", "NAMEkeep", 0, Some("named"));
    s += &member(&grp(".debug_types", "", "g_lack", true, None), ".Lk3", "LACKtype", 0, None);
    s += &member(&grp(".debug_macro", "", "g_size", true, None), ".Lk4", "SIZEkeep", 0, None);
    s += &member(&grp(".debug_macro", "", "g_two", true, Some(1)), ".Lk5", "TWO1keep", 0, None);
    s += &member(&grp(".debug_macro", "", "g_two", true, Some(2)), ".Lk6", "TWO2keep", 0, None);
    s += &member(&grp(".debug_wpk", "a", "g_kalloc", true, None), ".Lk7", "KALLkeep", 0, None);
    s += &member(&grp(".text.code", "ax", "g_code", true, None), ".Lk8", "CODEkeep", 0, None);
    s += &member(&grp(".debug_macro", "", "g_code", true, None), ".Lk9", "CODMkeep", 0, None);
    s += &member(&grp(".debug_macro", "", "g_nc", false, None), ".Lk10", "NOCOkeep", 0, None);
    // `g_gc`: an alloc member nothing reaches; under `--gc-sections` the
    // group dies with it (I-7), so its debug member is not live.
    s += &member(&grp(".text.gc", "ax", "g_gc", true, None), ".Lk11", "GCTXkeep", 0, None);
    s += &member(&grp(".debug_macro", "", "g_gc", true, None), ".Lk12", "GCMAkeep", 0, None);
    s
}

/// The words of `d.o`'s referrer, its own `.debug_macro` (a debug name),
/// in order: what each word must hold, by the tag it names.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Want {
    /// The keeper's member that starts with this tag, plus this offset.
    Kept(&'static [u8; 8], u64),
    /// `d.o`'s own member (a group without `GRP_COMDAT`: not dropped).
    Own(&'static [u8; 8]),
    /// I-47's tombstone (all ones in ELFCLASS64, 0 in ELFCLASS32 for
    /// `.debug_macro`).
    Tombstone,
    /// Kept without `--gc-sections`, the tombstone with it.
    KeptUnlessGc(&'static [u8; 8]),
}

struct Word {
    what: &'static str,
    width: u8,
    expr: &'static str,
    want: Want,
}

const WORDS: &[Word] = &[
    Word { what: "section symbol", width: 4, expr: ".Ld1", want: Want::Kept(b"SAMEkeep", 0) },
    Word { what: "addend", width: 4, expr: ".Ld1+4", want: Want::Kept(b"SAMEkeep", 4) },
    Word { what: "named local, st_value 8", width: 4, expr: "named", want: Want::Kept(b"NAMEkeep", 8) },
    Word { what: "keeper's group lacks the name", width: 4, expr: ".Ld3", want: Want::Tombstone },
    Word { what: "another size", width: 4, expr: ".Ld4", want: Want::Tombstone },
    Word { what: "two members of the name", width: 4, expr: ".Ld5", want: Want::Tombstone },
    Word { what: "keeper's member SHF_ALLOC", width: 4, expr: ".Ld7", want: Want::Tombstone },
    Word { what: "code (I-47 stands)", width: 8, expr: ".Ld8", want: Want::Tombstone },
    Word { what: "debug member beside code", width: 4, expr: ".Ld9", want: Want::KeptUnlessGc(b"CODMkeep") },
    Word { what: "no GRP_COMDAT", width: 4, expr: ".Ld10", want: Want::Own(b"NOCOdrop") },
    Word { what: "keeper's copy collected", width: 4, expr: ".Ld12", want: Want::KeptUnlessGc(b"GCMAkeep") },
    Word { what: "64-bit field", width: 8, expr: ".Ld1", want: Want::Kept(b"SAMEkeep", 0) },
];

fn dropped() -> String {
    let mut s = String::new();
    s += &member(&grp(".debug_macro", "", "g_same", true, None), ".Ld1", "SAMEdrop", 0, None);
    s += &member(&grp(".debug_macro", "", "g_named", true, None), ".Ld2", "NAMEdrop", 0, Some("named"));
    s += &member(&grp(".debug_macro", "", "g_lack", true, None), ".Ld3", "LACKdrop", 0, None);
    s += &member(&grp(".debug_macro", "", "g_size", true, None), ".Ld4", "SIZEdrop", 8, None);
    s += &member(&grp(".debug_macro", "", "g_two", true, None), ".Ld5", "TWO.drop", 0, None);
    s += &member(&grp(".debug_wpk", "", "g_kalloc", true, None), ".Ld7", "KALLdrop", 0, None);
    s += &member(&grp(".text.code", "ax", "g_code", true, None), ".Ld8", "CODEdrop", 0, None);
    s += &member(&grp(".debug_macro", "", "g_code", true, None), ".Ld9", "CODMdrop", 0, None);
    s += &member(&grp(".debug_macro", "", "g_nc", false, None), ".Ld10", "NOCOdrop", 0, None);
    s += &member(&grp(".text.gc", "ax", "g_gc", true, None), ".Ld11", "GCTXdrop", 0, None);
    s += &member(&grp(".debug_macro", "", "g_gc", true, None), ".Ld12", "GCMAdrop", 0, None);
    s += "    .section .debug_macro,\"\",@progbits\n    .ascii \"WP249REF\"\n";
    for w in WORDS {
        s += &format!("    .{}byte {}\n", w.width, w.expr);
    }
    s
}

fn assemble(arch: Arch, dir: &Path, name: &str, src: &str) -> Option<PathBuf> {
    let Some(cc) = arch.tool("gcc") else {
        assert!(!tools_required(), "{} is required", arch.gnu("gcc"));
        return None;
    };
    let s = dir.join(format!("{name}.s"));
    std::fs::write(&s, src).unwrap();
    let o = dir.join(format!("{name}.o"));
    let r = run_with(
        Command::new(cc).arg("-c").arg("-o").arg(&o).arg(&s).current_dir(dir),
        b"",
        Duration::from_mins(1),
    );
    assert!(r.success(), "{} {name}.s: {}", arch.name(), r.stderr_text());
    Some(o)
}

fn emulation(arch: Arch) -> &'static str {
    match arch {
        Arch::Aarch64 => "aarch64linux",
        Arch::X86_64 => "elf_x86_64",
        Arch::I386 => "elf_i386",
    }
}

fn mortar(args: &[&std::ffi::OsStr], dir: &Path, label: &str) {
    let o = run_with(Command::new(MORTAR).args(args).current_dir(dir), b"", Duration::from_mins(5));
    assert!(o.success(), "{label}: mortar: {}", o.stderr_text());
    assert_eq!(o.stderr_text(), "", "{label}: mortar warns");
}

fn branches(arch: Arch) {
    let dir = Scratch::new(&format!("i47a-{}", arch.name()));
    let Some(k) = assemble(arch, dir.path(), "k", &keeper(arch)) else { return };
    let mut d_src = dropped();
    if arch == Arch::I386 {
        // No 64-bit absolute type: the 8-byte words are 4-byte ones.
        d_src = d_src.replace(".8byte .Ld", ".4byte .Ld");
    }
    let Some(d) = assemble(arch, dir.path(), "d", &d_src) else { return };
    let is64 = arch != Arch::I386;
    for gc in [false, true] {
        let out = dir.join(if gc { "out-gc" } else { "out" });
        let words = ["-m", emulation(arch), "-static", "-e", "_start", "-o"];
        let mut args: Vec<&std::ffi::OsStr> = words.iter().map(std::ffi::OsStr::new).collect();
        args.push(out.as_os_str());
        args.extend([k.as_os_str(), d.as_os_str()]);
        if gc {
            args.push("--gc-sections".as_ref());
        }
        let label = format!("{} gc={gc}", arch.name());
        mortar(&args, dir.path(), &label);
        let macro_ = section(&out, ".debug_macro");
        let mut at =
            find(&macro_, b"WP249REF").unwrap_or_else(|| panic!("{label}: no referrer")) + 8;
        for w in WORDS {
            let width = if is64 { usize::from(w.width) } else { 4 };
            let mut bytes = [0u8; 8];
            bytes[..width].copy_from_slice(&macro_[at..at + width]);
            let got = u64::from_le_bytes(bytes);
            at += width;
            let tag = |t: &[u8; 8]| {
                find(&macro_, t).unwrap_or_else(|| panic!("{label}: {}: no tag", w.what)) as u64
            };
            let ones = if width == 8 { u64::MAX } else { u64::from(u32::MAX) };
            let tomb = if is64 { ones } else { 0 };
            let want = match w.want {
                Want::Kept(t, n) => tag(t) + n,
                Want::Own(t) => tag(t),
                Want::Tombstone => tomb,
                Want::KeptUnlessGc(_) if gc => tomb,
                Want::KeptUnlessGc(t) => tag(t),
            };
            assert_eq!(got, want, "{label}: {} ({:?})", w.what, w.want);
            // Not a dropped copy's bytes: the keeper's only.
            assert!(find(&macro_, b"SAMEdrop").is_none(), "{label}: a dropped copy is in the output");
        }
    }
}

/// I-47a's branches on AArch64.
#[test]
fn i47a_branches_aarch64() {
    branches(Arch::Aarch64);
}

/// I-47a's branches on x86-64.
#[test]
fn i47a_branches_x86_64() {
    branches(Arch::X86_64);
}

/// I-47a's branches on i386 (`R_386_32`, the addend in place; the
/// ELFCLASS32 tombstone of `.debug_macro` is 0).
#[test]
fn i47a_branches_i386() {
    branches(Arch::I386);
}

// ---------------------------------------------------------------------
// GCC -g3 against GNU ld.

const H: &str = "#ifndef WP249_H
#define WP249_H
#define WP249_ANSWER 42
#define WP249_TWICE(x) ((x) * 2)
struct wp249_point { int x; long y; };
int b_f(int);
int c_f(int);
extern int g_b;
#endif
";
const A: &str = "#include <stdio.h>
#include \"h.h\"
#define WP249_A_ONLY 1 /* a macro of a.c only: not defined in b */
int main(void) {
  struct wp249_point p = { 1, 2 };
  int r = b_f(p.x) + c_f((int)p.y);
  printf(\"%d %d %d\\n\", r, g_b, WP249_TWICE(WP249_ANSWER));
  return 0;
}
";
const B: &str = "#include <stdio.h>
#include \"h.h\"
int g_b = 7;
int b_f(int x) {
  struct wp249_point q = { x, WP249_ANSWER };
  puts(\"b\");
  return q.x + (int)q.y;
}
";
const C: &str = "#include <string.h>
#include \"h.h\"
int c_f(int x) {
  struct wp249_point q = { WP249_TWICE(x), 3 };
  return q.x + (int)strlen(\"c\");
}
";
const HH: &str = "#pragma once
#define WP249_ANSWER 42
#define WP249_TWICE(x) ((x) * 2)
struct Point { int x; long y; };
template <typename T> struct Box {
  T v;
  T get() const { return v + 1; }
};
inline int twice(int x) { return x * 2; }
int b_f(int);
int c_f(int);
extern int g_b;
";
const AA: &str = "#include <cstdio>
#include \"h.h\"
#define WP249_A_ONLY 1 /* a macro of a.cc only: not defined in b */
int main() {
  Point p{1, 2};
  Box<int> bx{p.x};
  int r = b_f(bx.get()) + c_f(twice((int)p.y));
  std::printf(\"%d %d %d\\n\", r, g_b, WP249_TWICE(WP249_ANSWER));
  return 0;
}
";
const BB: &str = "#include <cstdio>
#include \"h.h\"
int g_b = 7;
int b_f(int x) {
  Point q{x, WP249_ANSWER};
  Box<long> bl{q.y};
  std::puts(\"b\");
  return twice(q.x) + (int)bl.get();
}
";
const CC: &str = "#include <cstring>
#include \"h.h\"
int c_f(int x) {
  Point q{WP249_TWICE(x), 3};
  Box<int> bx{q.x};
  return bx.get() + (int)std::strlen(\"c\");
}
";

/// Objects of the C or C++ program by `arch`'s GCC with `flags`, and
/// the driver's static link line.
fn build(arch: Arch, cxx: bool, flags: &[&str], scratch: &Scratch) -> Option<support::Built> {
    let (h, srcs, driver) = if cxx {
        (HH, [("a.cc", AA), ("b.cc", BB), ("c.cc", CC)], "g++")
    } else {
        (H, [("a.c", A), ("b.c", B), ("c.c", C)], "gcc")
    };
    scratch.write("h.h", h);
    let sources: Vec<support::Source> = srcs
        .iter()
        .map(|(n, t)| support::Source { name: n, text: t })
        .collect();
    let d = arch.gnu(driver);
    if tool(&d).is_none() {
        assert!(!tools_required(), "{d} is required");
        return None;
    }
    let mut all = vec!["-I", scratch.path().to_str().unwrap()];
    all.extend_from_slice(flags);
    support::build_by(scratch, &d, &all, &sources)
}

/// readelf's `.debug_macro` units: offset -> rendered lines, without the
/// unit's `.debug_line` offset (each linker's layout).
fn units(readelf: &Path, prog: &Path, label: &str) -> Vec<(u64, Vec<String>)> {
    let o = run_with(
        Command::new(readelf).arg("--debug-dump=macro").arg(prog),
        b"",
        Duration::from_mins(2),
    );
    assert!(o.success(), "{label}: readelf: {}", o.stderr_text());
    assert!(!o.stderr_text().to_lowercase().contains("warn"), "{label}: readelf: {}", o.stderr_text());
    let mut units: Vec<(u64, Vec<String>)> = Vec::new();
    for line in o.stdout_text().lines() {
        let t = line.trim();
        if let Some(off) = t.strip_prefix("Offset:") {
            let off = off.trim();
            let v = off.strip_prefix("0x").map_or_else(|| off.parse(), |h| u64::from_str_radix(h, 16));
            units.push((v.unwrap_or_else(|_| panic!("{label}: offset {off}")), Vec::new()));
        } else if t.is_empty() || t.starts_with("Offset into .debug_line") || t.starts_with("Contents of") {
        } else if let Some(u) = units.last_mut() {
            u.1.push(t.to_owned());
        }
    }
    assert!(!units.is_empty(), "{label}: no macro unit");
    units
}

/// Each unit rendered with every import replaced by what it names (the
/// target unit's lines, its own imports left as offsets): sorted.
fn rendered(units: &[(u64, Vec<String>)], label: &str) -> (Vec<String>, usize) {
    let mut imports = 0;
    let by_off: std::collections::BTreeMap<u64, &Vec<String>> =
        units.iter().map(|(o, l)| (*o, l)).collect();
    let mut out: Vec<String> = units
        .iter()
        .map(|(_, lines)| {
            lines
                .iter()
                .map(|l| match l.strip_prefix("DW_MACRO_import - offset : 0x") {
                    Some(h) => {
                        imports += 1;
                        let v = u64::from_str_radix(h.trim(), 16).unwrap();
                        let t = by_off.get(&v).unwrap_or_else(|| {
                            panic!("{label}: import of {v:#x} names no unit")
                        });
                        format!("import [{}]", t.join(" | "))
                    }
                    None => l.clone(),
                })
                .collect::<Vec<_>>()
                .join("\n")
        })
        .collect();
    out.sort();
    (out, imports)
}

/// I-47a on real `DW_MACRO_import`s: mortar's imports name the units GNU
/// ld's name (by contents), for C and C++, DWARF 4 and 5, both 64-bit
/// targets; one incremental full link per target equals the plain one.
fn gcc_g3(arch: Arch) {
    let (Some(ld), Some(readelf)) = (arch.tool("ld.bfd"), arch.tool("readelf")) else {
        assert!(!tools_required(), "ld.bfd and readelf are required");
        return;
    };
    for cxx in [false, true] {
        for v in ["-gdwarf-4", "-gdwarf-5"] {
            let label = format!("{} {} {v}", arch.name(), if cxx { "C++" } else { "C" });
            let scratch = Scratch::new("i47a-gcc");
            let Some(built) = build(arch, cxx, &["-O0", "-g3", v], &scratch) else { return };
            let ours = scratch.join("mortar.out");
            let theirs = scratch.join("bfd.out");
            let args = support::with_output(&built.args, &ours);
            let a: Vec<&std::ffi::OsStr> = args.iter().map(|s| s.as_os_str()).collect();
            mortar(&a, scratch.path(), &label);
            let b = run_with(
                Command::new(&ld).args(support::with_output(&built.args, &theirs)).current_dir(scratch.path()),
                b"",
                Duration::from_mins(5),
            );
            assert!(b.success(), "{label}: ld.bfd: {}", b.stderr_text());
            let (o, n) = rendered(&units(&readelf, &ours, &label), &label);
            let (t, m) = rendered(&units(&readelf, &theirs, &label), &label);
            assert!(n > 20, "{label}: {n} imports: not GCC's -g3");
            assert_eq!(n, m, "{label}: import counts");
            if o != t {
                let first = o.iter().zip(&t).find(|(x, y)| x != y);
                panic!("{label}: the macro units differ from GNU ld's; first: {first:#?}");
            }
            if v == "-gdwarf-5" && !cxx {
                // Item 4: the incremental mode's full link fills the same.
                let incr = scratch.join("incr.out");
                let mut args = support::with_output(&built.args, &incr);
                args.push("--incremental=full".into());
                let a: Vec<&std::ffi::OsStr> = args.iter().map(|s| s.as_os_str()).collect();
                mortar(&a, scratch.path(), &format!("{label} --incremental=full"));
                // Its debug contributions are laid out in size classes
                // (`spec/incr.md` IN-30): the units are compared by what
                // their imports name, as against GNU ld.
                let (i, k) = rendered(&units(&readelf, &incr, &label), &label);
                assert_eq!(k, n, "{label}: --incremental=full: import counts");
                assert!(i == o, "{label}: --incremental=full's macro units differ from the plain link's");
            }
        }
    }
}

#[test]
fn gcc_g3_imports_name_gnu_lds_units_aarch64() {
    gcc_g3(Arch::Aarch64);
}

#[test]
fn gcc_g3_imports_name_gnu_lds_units_x86_64() {
    gcc_g3(Arch::X86_64);
}

// ---------------------------------------------------------------------
// gdb on x86-64 (qemu's gdb stub).

/// gdb's answers in `b_f`: `info macro` of a header macro, of a.c's own
/// macro (none in b.c), `macro expand`; the session's lines that state
/// them.
fn gdb_session(gdb: &Path, prog: &Path, dir: &Path, label: &str) -> Vec<String> {
    let port = std::net::TcpListener::bind("127.0.0.1:0")
        .and_then(|l| l.local_addr())
        .map(|a| a.port())
        .unwrap_or_else(|e| panic!("a port: {e}"));
    let (qemu, mut qargs) = Arch::X86_64.runner(prog);
    let program = qargs.pop().unwrap();
    let mut q = Command::new(qemu);
    q.args(&qargs).arg("-g").arg(port.to_string()).arg(program);
    q.current_dir(dir).stdin(std::process::Stdio::null());
    q.stdout(std::process::Stdio::null()).stderr(std::process::Stdio::null());
    let mut child = q.spawn().unwrap_or_else(|e| panic!("{label}: qemu: {e}"));
    std::thread::sleep(Duration::from_millis(500));
    let cmds = [
        "set pagination off".to_owned(),
        "set debuginfod enabled off".to_owned(),
        format!("file {}", prog.display()),
        format!("target remote 127.0.0.1:{port}"),
        "break b_f".to_owned(),
        "continue".to_owned(),
        "info macro WP249_ANSWER".to_owned(),
        "macro expand WP249_TWICE(WP249_ANSWER)".to_owned(),
        "info macro WP249_A_ONLY".to_owned(),
        "info macro __GNUC__".to_owned(),
        "kill".to_owned(),
    ];
    let mut c = Command::new(gdb);
    c.args(["-nx", "-q", "-batch"]);
    for cmd in &cmds {
        c.arg("-ex").arg(cmd);
    }
    c.current_dir(dir).env("DEBUGINFOD_URLS", "");
    let o = run_with(&mut c, b"", Duration::from_mins(2));
    let _ = child.kill();
    let _ = child.wait();
    let all = format!("{}{}", o.stdout_text(), o.stderr_text());
    assert!(!all.contains("This is a bug") && !all.contains("Fatal signal"), "{label}: gdb: {all}");
    let keep: Vec<String> = all
        .lines()
        .filter(|l| {
            l.starts_with("Defined at")
                || l.starts_with("#define")
                || l.starts_with("-D")
                || l.starts_with("expands to:")
                || l.contains("has no definition")
        })
        .map(|l| l.split(prog.to_str().unwrap()).collect::<String>())
        .collect();
    assert!(keep.iter().any(|l| l == "#define WP249_ANSWER 42"), "{label}: gdb: {all}");
    assert!(keep.iter().any(|l| l == "expands to: ((42) * 2)"), "{label}: gdb: {all}");
    assert!(keep.iter().any(|l| l.contains("WP249_A_ONLY") && l.contains("has no definition")), "{label}: gdb: {all}");
    keep
}

/// gdb-multiarch on qemu-x86_64: mortar's output answers as GNU ld's.
#[test]
fn gdb_x86_64_macros() {
    let arch = Arch::X86_64;
    let (Some(ld), Some(gdb)) = (arch.tool("ld.bfd"), tool(arch.gdb())) else {
        assert!(!tools_required(), "ld.bfd and gdb are required");
        return;
    };
    if tool(arch.qemu()).is_none() && !arch.is_host() {
        assert!(!tools_required(), "{} is required", arch.qemu());
        return;
    }
    for v in ["-gdwarf-4", "-gdwarf-5"] {
        let label = format!("x86_64 C {v}");
        let scratch = Scratch::new("i47a-gdb");
        let Some(built) = build(arch, false, &["-O0", "-g3", v], &scratch) else { return };
        let ours = scratch.join("mortar.out");
        let theirs = scratch.join("bfd.out");
        let args = support::with_output(&built.args, &ours);
        let a: Vec<&std::ffi::OsStr> = args.iter().map(|s| s.as_os_str()).collect();
        mortar(&a, scratch.path(), &label);
        let b = run_with(
            Command::new(&ld).args(support::with_output(&built.args, &theirs)).current_dir(scratch.path()),
            b"",
            Duration::from_mins(5),
        );
        assert!(b.success(), "{label}: ld.bfd: {}", b.stderr_text());
        let o = gdb_session(&gdb, &ours, scratch.path(), &format!("{label} mortar"));
        let t = gdb_session(&gdb, &theirs, scratch.path(), &format!("{label} bfd"));
        assert_eq!(o, t, "{label}: gdb's answers differ from GNU ld's output's");
    }
}
