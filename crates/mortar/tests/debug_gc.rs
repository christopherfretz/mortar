//! Debug information with `--gc-sections` and dead code on all three
//! targets (WP-030d): the references from `.debug_*` into collected
//! sections, dropped COMDAT copies and collected merge pieces get the
//! tombstones of `spec/image.md` I-47 and the aranges length of I-112,
//! and the output is held to:
//!
//! - the verifier (`mortar_plumb`, zero findings with mortar's map);
//! - the program's output against GNU ld's `--gc-sections` output of the
//!   same objects (under qemu for i386 and x86-64);
//! - `llvm-dwarfdump --verify`: no more errors than on GNU ld's output, or
//!   on ELFCLASS32 than on lld's (I-47's ELFCLASS32 column is lld's 0:
//!   a collected function's `DW_AT_low_pc` 0 makes the same `[0, len)`
//!   overlaps there; GNU ld redirects a dropped COMDAT copy instead,
//!   which I-47 rejects);
//! - gdb: `info line` and `break` on a kept function, and `info line` on a
//!   collected one, read the same as on GNU ld's output.
//!
//! The gap it closes: `tests/x86.rs` and `tests/x64.rs` judged every
//! output with the verifier, but no link there had `-g`, `--gc-sections`
//! and a collected function with debug information, so the verifier's
//! ELFCLASS32 I-112 false positive went unseen until another package's
//! outputs met it (`spec/landed/x86/WP-030d.md`).

mod support;

use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::Duration;

use mortar_testutil::{Outcome, Scratch, run_with, target, tool, tools_required};
use support::{Built, Source, build_by, with_output};

const MORTAR: &str = env!("CARGO_BIN_EXE_mortar");

/// C: a collected function (with a static it writes), a collected string
/// piece (`.rodata.str1.1`, I-45's merge case), a collected data object.
const C_SOURCES: &[Source] = &[Source {
    name: "dg.c",
    text: r#"#include <stdio.h>
static int counter;
int dead_data[16] = {1, 2, 3};
int unused_g(int x) { counter += x; return x * 3 + 1; }
const char *unused_s(void) { return "only a collected function names this"; }
__attribute__((noinline)) int used_f(int x) { return x + 2; }
int main(void) { printf("%d %s\n", used_f(40), "live"); return 0; }
"#,
}];

/// C++: the COMDAT function `big` in two objects (the second copy is
/// dropped with its debug information), a collected function in each, one
/// of them throwing (its `.gcc_except_table` and FDE are collected too),
/// exceptions caught in `main`.
const CXX_SOURCES: &[Source] = &[
    Source {
        name: "dg_a.cc",
        text: r#"#include <cstdio>
#include <stdexcept>
inline __attribute__((noinline)) int big(int x) { int s = 0; for (int i = 0; i < x; ++i) s += i * x; return s; }
int unused_a(int x) { if (x > 3) throw std::runtime_error("unused a"); return big(x) + 7; }
int helper(int);
int main(int argc, char **) {
  try { if (argc > 5) throw std::runtime_error("live"); } catch (const std::exception &e) { std::puts(e.what()); }
  std::printf("%d %d\n", big(argc + 3), helper(argc));
  return 0;
}
"#,
    },
    Source {
        name: "dg_b.cc",
        text: r"inline __attribute__((noinline)) int big(int x) { int s = 0; for (int i = 0; i < x; ++i) s += i * x; return s; }
int unused_b(int x) { return big(x) * 2; }
int helper(int x) { return big(x + 1); }
",
    },
];

/// One target: its drivers, GNU ld, and how its programs run.
struct Target {
    name: &'static str,
    gcc: &'static str,
    gxx: &'static str,
    /// Clang's `--target`, `None` on the host.
    clang_triple: Option<&'static str>,
    ld: &'static str,
    /// Its architecture: the emulator and gdb by the host
    /// (`mortar_testutil::target`).
    arch: target::Arch,
    /// ELFCLASS32: `--verify` is held to lld's count too (I-47).
    class32: bool,
}

const AARCH64: Target = Target {
    name: "aarch64",
    gcc: "aarch64-linux-gnu-gcc",
    gxx: "aarch64-linux-gnu-g++",
    clang_triple: Some("aarch64-linux-gnu"),
    ld: "aarch64-linux-gnu-ld.bfd",
    arch: target::Arch::Aarch64,
    class32: false,
};

const X86_64: Target = Target {
    name: "x86_64",
    gcc: "x86_64-linux-gnu-gcc",
    gxx: "x86_64-linux-gnu-g++",
    clang_triple: Some("x86_64-linux-gnu"),
    ld: "x86_64-linux-gnu-ld.bfd",
    arch: target::Arch::X86_64,
    class32: false,
};

const I386: Target = Target {
    name: "i386",
    gcc: "i686-linux-gnu-gcc",
    gxx: "i686-linux-gnu-g++",
    clang_triple: Some("i686-linux-gnu"),
    ld: "i686-linux-gnu-ld.bfd",
    arch: target::Arch::I386,
    class32: true,
};

impl Target {
    /// The emulator, `None` when the target is the host's.
    fn qemu(&self) -> Option<&'static str> {
        (!self.arch.is_host()).then(|| self.arch.qemu())
    }
}

/// One configuration of the matrix.
struct Config {
    clang: bool,
    cxx: bool,
    flags: &'static [&'static str],
}

/// C: every compiler, DWARF version and optimization level, and GCC's
/// `-gsplit-dwarf`; C++: each compiler at DWARF 4 and 5, one of them at
/// `-O0` and the other at `-O2` (a static C++ link is the slow one).
const CONFIGS: &[Config] = &[
    Config { clang: false, cxx: false, flags: &["-gdwarf-4", "-O0"] },
    Config { clang: false, cxx: false, flags: &["-gdwarf-4", "-O2"] },
    Config { clang: false, cxx: false, flags: &["-gdwarf-5", "-O0"] },
    Config { clang: false, cxx: false, flags: &["-gdwarf-5", "-O2"] },
    Config { clang: false, cxx: false, flags: &["-gdwarf-5", "-O0", "-gsplit-dwarf"] },
    Config { clang: true, cxx: false, flags: &["-gdwarf-4", "-O0"] },
    Config { clang: true, cxx: false, flags: &["-gdwarf-4", "-O2"] },
    Config { clang: true, cxx: false, flags: &["-gdwarf-5", "-O0"] },
    Config { clang: true, cxx: false, flags: &["-gdwarf-5", "-O2"] },
    Config { clang: false, cxx: true, flags: &["-gdwarf-4", "-O0"] },
    Config { clang: false, cxx: true, flags: &["-gdwarf-5", "-O2"] },
    Config { clang: true, cxx: true, flags: &["-gdwarf-4", "-O2"] },
    Config { clang: true, cxx: true, flags: &["-gdwarf-5", "-O0"] },
];

/// The tools every target needs, or `None` (a failure under
/// `MORTAR_REQUIRE_TOOLS`).
struct Tools {
    gdb: PathBuf,
    dwarfdump: PathBuf,
    lld: PathBuf,
}

fn tools(t: &Target) -> Option<Tools> {
    let gdb = t.arch.gdb();
    let need = [t.gcc, t.gxx, "clang", "clang++", t.ld, gdb, "llvm-dwarfdump", "ld.lld"];
    let missing: Vec<&str> = need.iter().copied().filter(|n| tool(n).is_none()).collect();
    let qemu = t.qemu();
    let qemu_missing = qemu.is_some_and(|q| tool(q).is_none());
    if !missing.is_empty() || qemu_missing {
        assert!(!tools_required(), "{}: {missing:?} or {qemu:?} is missing", t.name);
        eprintln!("note: {}: {missing:?} or {qemu:?} is missing; skipping", t.name);
        return None;
    }
    Some(Tools { gdb: tool(gdb)?, dwarfdump: tool("llvm-dwarfdump")?, lld: tool("ld.lld")? })
}

fn linker(exe: &Path, args: &[OsString], dir: &Path) -> Outcome {
    run_with(Command::new(exe).args(args).current_dir(dir), b"", Duration::from_mins(5))
}

/// Runs a tool in `dir` with its two streams in one, in order, and
/// debuginfod off.
fn tool_run(exe: &Path, args: &[&str], dir: &Path, limit: Duration) -> Outcome {
    let mut c = Command::new("sh");
    c.arg("-c").arg("exec \"$0\" \"$@\" 2>&1").arg(exe).args(args);
    c.current_dir(dir).env("DEBUGINFOD_URLS", "").env("TERM", "dumb");
    run_with(&mut c, b"", limit)
}

/// The program's standard output and exit status.
fn execute(t: &Target, exe: &Path) -> (String, Option<i32>) {
    let o = run_with(&mut t.arch.command(exe, None), b"", Duration::from_mins(2));
    (o.stdout_text(), o.status.and_then(|s| s.code()))
}

/// The number of errors `llvm-dwarfdump --verify` reports, `None` when it
/// does not finish in 20 s (it does not, on GCC 14's `-O2` C++ objects
/// here, whatever the linker: `tests/debuggers.rs` `verify_against`).
fn verify_errors(t: &Tools, prog: &Path, dir: &Path) -> Option<usize> {
    let p = prog.display().to_string();
    let o = tool_run(&t.dwarfdump, &["--verify", &p], dir, Duration::from_secs(20));
    if o.timed_out() {
        return None;
    }
    let text = format!("{}{}", o.stdout_text(), o.stderr_text());
    Some(text.lines().filter(|l| l.starts_with("error:")).count())
}

/// gdb's `info line` and `break` on `kept`, and `info line` on `dead`,
/// without running, addresses masked.
fn gdb(t: &Tools, prog: &Path, dir: &Path, kept: &str, dead: &str) -> String {
    let (il, br, dl) =
        (format!("info line {kept}"), format!("break {kept}"), format!("info line {dead}"));
    let p = prog.display().to_string();
    let args = ["-batch", "-nx", "-ex", &il, "-ex", &br, "-ex", &dl, &p];
    let o = tool_run(&t.gdb, &args, dir, Duration::from_mins(2));
    assert!(!o.timed_out(), "gdb on {p} did not finish");
    let text = format!("{}{}", o.stdout_text(), o.stderr_text());
    let mut out = String::new();
    for line in text.lines() {
        let mut masked = String::new();
        let mut rest = line;
        while let Some(i) = rest.find("0x") {
            masked.push_str(&rest[..i]);
            masked.push_str("ADDR");
            let tail = &rest[i + 2..];
            let n = tail.find(|c: char| !c.is_ascii_hexdigit()).unwrap_or(tail.len());
            rest = &tail[n..];
        }
        masked.push_str(rest);
        out.push_str(&masked);
        out.push('\n');
    }
    out
}

/// One configuration on one target: compile, link with mortar, GNU ld and
/// lld (all `--gc-sections`), and hold mortar's output to the bar.
#[allow(clippy::too_many_lines)] // the checks in the order of the doc comment
fn holds(t: &Target, tools: &Tools, c: &Config) {
    let lang = if c.cxx { "cxx" } else { "c" };
    let cc_name = if c.clang { "clang" } else { "gcc" };
    let label = format!("{}-{cc_name}-{lang}-{}", t.name, c.flags.join(""));
    let scratch = Scratch::new(&format!("debug-gc-{label}"));
    let driver = match (c.clang, c.cxx) {
        (false, false) => t.gcc,
        (false, true) => t.gxx,
        (true, false) => "clang",
        (true, true) => "clang++",
    };
    let target = t.clang_triple.map(|x| format!("--target={x}"));
    let mut flags: Vec<&str> = Vec::new();
    if c.clang
        && let Some(x) = &target
    {
        flags.push(x);
    }
    flags.extend_from_slice(&["-g", "-ffunction-sections", "-fdata-sections"]);
    flags.extend_from_slice(c.flags);
    let sources = if c.cxx { CXX_SOURCES } else { C_SOURCES };
    let built: Built =
        build_by(&scratch, driver, &flags, sources).unwrap_or_else(|| panic!("{label}: {driver}"));
    let dir = &built.dir;
    let (ours, theirs, lld_out) =
        (dir.join("out.mortar"), dir.join("out.bfd"), dir.join("out.lld"));
    let map = dir.join("out.mortar.map");

    // mortar: the link succeeds, collects code, and says nothing else.
    let mut args = with_output(&built.args, &ours);
    args.push(format!("--mortar-map={}", map.display()).into());
    args.extend(["--gc-sections".into(), "--print-gc-sections".into()]);
    let m = linker(Path::new(MORTAR), &args, dir);
    assert!(m.success(), "{label}: mortar: {}", m.stderr_text());
    let said = m.stderr_text();
    assert!(
        said.lines().all(|l| l.contains("removing unused section '")),
        "{label}: mortar warns: {said}"
    );
    let dead_fn = if c.cxx { "unused_a" } else { "unused_g" };
    assert!(
        said.lines().any(|l| l.contains(".text") && l.contains(dead_fn)),
        "{label}: {dead_fn} was not collected: {said}"
    );
    // The references.
    for (exe, out) in [(tool(t.ld).expect("checked"), &theirs), (tools.lld.clone(), &lld_out)] {
        let mut args = with_output(&built.args, out);
        args.push("--gc-sections".into());
        let o = linker(&exe, &args, dir);
        assert!(o.success(), "{label}: {}: {}", exe.display(), o.stderr_text());
    }

    // The verifier.
    support::plumb::readers(&ours);
    support::plumb::clean(&ours, &map, dir);

    // The program.
    let (run_m, run_b) = (execute(t, &ours), execute(t, &theirs));
    assert!(run_b.1 == Some(0), "{label}: GNU ld's program: {run_b:?}");
    assert_eq!(run_m, run_b, "{label}: mortar's program against GNU ld's");

    // llvm-dwarfdump --verify.
    match verify_errors(tools, &theirs, dir) {
        None => {
            eprintln!("note: {label}: llvm-dwarfdump --verify does not finish on GNU ld's output");
        }
        Some(bfd) => {
            let ours_n = verify_errors(tools, &ours, dir)
                .unwrap_or_else(|| panic!("{label}: --verify timed out on mortar's output only"));
            let bound = if t.class32 {
                bfd.max(verify_errors(tools, &lld_out, dir).unwrap_or(bfd))
            } else {
                bfd
            };
            assert!(
                ours_n <= bound,
                "{label}: llvm-dwarfdump --verify: {ours_n} errors on mortar's output, {bfd} on GNU ld's (bound {bound})"
            );
        }
    }

    // gdb.
    let kept = if c.cxx { "helper" } else { "used_f" };
    let g_m = gdb(tools, &ours, dir, kept, dead_fn);
    let g_b = gdb(tools, &theirs, dir, kept, dead_fn);
    assert!(g_m.contains("Breakpoint 1 at ADDR: file "), "{label}: gdb: {g_m}");
    assert!(g_m.contains(&format!("Function \"{dead_fn}\" not defined")), "{label}: gdb: {g_m}");
    assert_eq!(g_m, g_b, "{label}: gdb on mortar's output against GNU ld's");
}

fn matrix(t: &Target) {
    let Some(tools) = tools(t) else { return };
    for c in CONFIGS {
        holds(t, &tools, c);
    }
}

// spec: image.md I-45, I-47, I-112 (tombstones; the aranges length); plumb.md §5.8
#[test]
fn debug_gc_aarch64() {
    matrix(&AARCH64);
}

// spec: image.md I-45, I-47, I-112 (tombstones; the aranges length); plumb.md §5.8
#[test]
fn debug_gc_x86_64() {
    matrix(&X86_64);
}

// spec: image.md I-45, I-47 (the ELFCLASS32 column), I-112; plumb.md §5.8
#[test]
fn debug_gc_i386() {
    matrix(&I386);
}

// ---------------------------------------------------------------------
// The sibling audit's fields (WP-030d): every data width, a PC-relative
// field, a collected section and a collected merge piece, in every
// tombstone row of I-47, on a REL input (i386) and a RELA one (x86-64).

/// `debug_gc_fields.s`: in each debug section, sentinel words (`0xa5`)
/// around references to `.text.dead` (collected) and to `dead_s` (a
/// collected piece of a live `SHF_MERGE` section): `addr dead + 1`,
/// `.long dead`, `.word dead`, `.byte dead`, `.long dead - .`,
/// `addr dead_s` (not in `.debug_aranges`), and a live `addr live_s`.
const FIELDS_S: &str = include_str!("debug_gc_fields.s");

/// The sections of an ELF file: name and file bytes.
fn section_bytes(path: &Path) -> Vec<(String, Vec<u8>)> {
    let b = std::fs::read(path).expect("the output");
    let h = mortar_elf::FileHeader::parse(&b).expect("an ELF header");
    let t = mortar_elf::SectionTable::parse(&b, &h).expect("section headers");
    t.iter()
        .map(|s| {
            let sh = s.header();
            let name = String::from_utf8_lossy(s.name().unwrap_or(b"")).into_owned();
            let bytes = if sh.sh_type == 8 {
                Vec::new()
            } else {
                b[sh.sh_offset as usize..(sh.sh_offset + sh.sh_size) as usize].to_vec()
            };
            (name, bytes)
        })
        .collect()
}

/// The tombstone of I-47 for `section` in the class of `w`-byte
/// addresses, as `w` bytes.
fn tombstone(section: &str, w: usize) -> Vec<u8> {
    let v: u64 = match section {
        ".debug_ranges" | ".debug_loc" => 1,
        ".debug_rnglists" | ".debug_loclists" | ".debug_addr" => 0,
        ".debug_aranges" | ".debug_frame" => u64::MAX,
        _ if w == 8 => u64::MAX,
        _ => 0,
    };
    v.to_le_bytes()[..w].to_vec()
}

/// Links `debug_gc_fields.s` with mortar for one class and checks every
/// field: the verifier is clean, and in each debug section the first
/// reference (`addr dead + 1`, after one sentinel address) is I-47's
/// tombstone at the address width, the sentinel after it intact (or, in
/// `.debug_aranges`, the I-112 length 0); each narrower field holds the
/// tombstone truncated (I-48) and its sentinel is intact (the verifier
/// judges the live reference to `live_s`).
#[allow(clippy::many_single_char_names)] // offsets, widths and bytes of one layout
fn fields(asm: &str, emulation: &str, w64: bool) {
    let Some(asm_tool) = tool(asm) else {
        assert!(!tools_required(), "{asm} is required");
        return;
    };
    let label = format!("debug-gc-fields-{emulation}");
    let scratch = Scratch::new(&label);
    let src = scratch.write("fields.s", FIELDS_S);
    let dir = scratch.path();
    let mut args: Vec<OsString> = Vec::new();
    if w64 {
        args.extend(["--defsym".into(), "W64=1".into()]);
    }
    args.extend([src.into_os_string(), "-o".into(), dir.join("fields.o").into_os_string()]);
    let o = mortar_testutil::run(&asm_tool, &args);
    assert!(o.success(), "{label}: {asm}: {}", o.stderr_text());
    let (out, map) = (dir.join("out"), dir.join("out.map"));
    let args: Vec<OsString> = vec![
        "-m".into(),
        emulation.into(),
        "-static".into(),
        "-e".into(),
        "_start".into(),
        "--gc-sections".into(),
        "fields.o".into(),
        "-o".into(),
        out.clone().into_os_string(),
        format!("--mortar-map={}", map.display()).into(),
    ];
    let m = linker(Path::new(MORTAR), &args, dir);
    assert!(m.success(), "{label}: mortar: {}", m.stderr_text());
    assert_eq!(m.stderr_text(), "", "{label}: mortar warns");
    support::plumb::clean(&out, &map, dir);
    let w = if w64 { 8 } else { 4 };
    let secs = section_bytes(&out);
    let names = [
        ".debug_info",
        ".debug_line",
        ".debug_str_offsets",
        ".debug_ranges",
        ".debug_loc",
        ".debug_rnglists",
        ".debug_loclists",
        ".debug_addr",
        ".debug_frame",
        ".debug_aranges",
    ];
    let sentinel = |n: usize| {
        let mut v = vec![0xa5u8; n.min(4)];
        v.resize(n, 0);
        v
    };
    for name in names {
        let (_, b) = secs.iter().find(|s| s.0 == name).unwrap_or_else(|| panic!("{label}: {name}"));
        assert_eq!(b[..w], sentinel(w), "{label}: {name}: the leading sentinel");
        assert_eq!(
            b[w..2 * w],
            tombstone(name, w),
            "{label}: {name}: the tombstone of `addr dead + 1`"
        );
        let after = if name == ".debug_aranges" { vec![0u8; w] } else { sentinel(w) };
        assert_eq!(b[2 * w..3 * w], after, "{label}: {name}: the word after the tombstone");
        // `.long dead`: on i386 the address-sized absolute type (I-112
        // in `.debug_aranges`: the next 4 bytes are 0), on x86-64
        // `R_X86_64_32`.
        let at = 3 * w;
        let t4 = if name == ".debug_aranges" {
            vec![0xff; 4]
        } else {
            let mut t = tombstone(name, w);
            t.truncate(4);
            t
        };
        assert_eq!(b[at..at + 4], t4, "{label}: {name}: `.long dead`");
        let zeroed = name == ".debug_aranges" && !w64;
        let s4 = if zeroed { vec![0; 4] } else { sentinel(4) };
        assert_eq!(b[at + 4..at + 8], s4, "{label}: {name}: the word after `.long dead`");
        // `.word dead`, `.byte dead`, each followed by its sentinel.
        let t = tombstone(name, w);
        assert_eq!(b[at + 8..at + 10], t[..2], "{label}: {name}: `.word dead`");
        assert_eq!(b[at + 10..at + 12], [0xa5, 0xa5], "{label}: {name}: after `.word dead`");
        assert_eq!(b[at + 12], t[0], "{label}: {name}: `.byte dead`");
        assert_eq!(b[at + 13], 0xa5, "{label}: {name}: after `.byte dead`");
        // `.long dead - .`: PC-relative, the tombstone at 4 bytes too.
        assert_eq!(b[at + 14..at + 18], t[..4], "{label}: {name}: `.long dead - .`");
        assert_eq!(b[at + 18..at + 22], [0xa5; 4], "{label}: {name}: after `.long dead - .`");
        if name != ".debug_aranges" {
            let p = at + 22;
            assert_eq!(b[p..p + w], t[..], "{label}: {name}: `addr dead_s` (a collected piece)");
            assert_eq!(b[p + w..p + 2 * w], sentinel(w), "{label}: {name}: after `addr dead_s`");
        }
    }
}

// spec: image.md I-45, I-47 (ELFCLASS32), I-48, I-112; x86.md X86-59
#[test]
fn tombstones_are_as_wide_as_their_rel_fields_on_i386() {
    fields("i686-linux-gnu-as", "elf_i386", false);
}

// spec: image.md I-45, I-47, I-48, I-112
#[test]
fn tombstones_are_as_wide_as_their_fields_on_x86_64() {
    fields("x86_64-linux-gnu-as", "elf_x86_64", true);
}
