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

use mortar_testutil::{Outcome, Scratch, run_with, tool, tools_required};
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
    /// The emulator, `None` when the target is the host's.
    qemu: Option<&'static str>,
    /// ELFCLASS32: `--verify` is held to lld's count too (I-47).
    class32: bool,
}

const AARCH64: Target = Target {
    name: "aarch64",
    gcc: "aarch64-linux-gnu-gcc",
    gxx: "aarch64-linux-gnu-g++",
    clang_triple: Some("aarch64-linux-gnu"),
    ld: "aarch64-linux-gnu-ld.bfd",
    qemu: None,
    class32: false,
};

const X86_64: Target = Target {
    name: "x86_64",
    gcc: "x86_64-linux-gnu-gcc",
    gxx: "x86_64-linux-gnu-g++",
    clang_triple: Some("x86_64-linux-gnu"),
    ld: "x86_64-linux-gnu-ld.bfd",
    qemu: Some("qemu-x86_64"),
    class32: false,
};

const I386: Target = Target {
    name: "i386",
    gcc: "i686-linux-gnu-gcc",
    gxx: "i686-linux-gnu-g++",
    clang_triple: Some("i686-linux-gnu"),
    ld: "i686-linux-gnu-ld.bfd",
    qemu: Some("qemu-i386"),
    class32: true,
};

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
    let need = [t.gcc, t.gxx, "clang", "clang++", t.ld, "gdb", "llvm-dwarfdump", "ld.lld"];
    let missing: Vec<&str> = need.iter().copied().filter(|n| tool(n).is_none()).collect();
    let qemu_missing = t.qemu.is_some_and(|q| tool(q).is_none());
    if !missing.is_empty() || qemu_missing {
        assert!(!tools_required(), "{}: {missing:?} or {:?} is missing", t.name, t.qemu);
        eprintln!("note: {}: {missing:?} or {:?} is missing; skipping", t.name, t.qemu);
        return None;
    }
    Some(Tools { gdb: tool("gdb")?, dwarfdump: tool("llvm-dwarfdump")?, lld: tool("ld.lld")? })
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
    let mut c = match t.qemu {
        Some(q) => {
            let mut c = Command::new(q);
            c.arg(exe);
            c
        }
        None => Command::new(exe),
    };
    let o = run_with(&mut c, b"", Duration::from_mins(2));
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
