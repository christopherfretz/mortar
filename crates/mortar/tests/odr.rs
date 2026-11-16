//! The one-definition-rule switches and the foreign checks end to end
//! (`spec/odr.md` §8.1, O-40 to O-44, T-O-41; WP-072): the modes, the
//! byte identity of `--odr=off`, `--odr-stats`, and the report's
//! independence of the thread count. The link tests of `tests/link/odr/`
//! check the catalogue's cases with compilers; these use assembly, so that
//! each copy is exactly what the test says.

use std::ffi::OsString;
use std::path::PathBuf;
use std::process::Command;
use std::time::Duration;

use mortar_testutil::{Outcome, Scratch, run_with, tool, tools_required};

const MORTAR: &str = env!("CARGO_BIN_EXE_mortar");

/// `_start`; `v`, data of 16 bytes in COMDAT group `v`; `f`, a function in
/// COMDAT group `f`.
const A: &str = "\t.text\n\t.globl _start\n\t.type _start, %function\n_start:\n\
    \tmov x0, #0\n\tmov x8, #93\n\tsvc #0\n\
    \t.section .data.v,\"awG\",%progbits,v,comdat\n\t.weak v\n\t.type v, %object\n\
    \t.size v, 16\nv:\t.quad 1\n\t.quad 2\n\
    \t.section .text.f,\"axG\",%progbits,f,comdat\n\t.weak f\n\t.type f, %function\n\
    f:\tret\n\t.size f, 4\n\
    \t.section .note.GNU-stack,\"\",%progbits\n";

/// `v` again, of 8 bytes (W1 against `a.o`).
const B: &str = "\t.section .data.v,\"awG\",%progbits,v,comdat\n\t.weak v\n\t.type v, %object\n\
    \t.size v, 8\nv:\t.quad 1\n\t.section .note.GNU-stack,\"\",%progbits\n";

/// `f` as data in its own COMDAT group `f` (W2 against `a.o`).
const C: &str = "\t.section .data.f,\"awG\",%progbits,f,comdat\n\t.weak f\n\t.type f, %object\n\
    \t.size f, 8\nf:\t.quad 0\n\t.section .note.GNU-stack,\"\",%progbits\n";

/// `g`, a strong definition outside any group (W3 with `e.o`).
const D: &str = "\t.text\n\t.globl g\n\t.type g, %function\ng:\tret\n\t.size g, 4\n\
    \t.section .note.GNU-stack,\"\",%progbits\n";

/// `g` in COMDAT group `g`.
const E: &str = "\t.section .text.g,\"axG\",%progbits,g,comdat\n\t.weak g\n\t.type g, %function\n\
    g:\tret\n\t.size g, 4\n\t.section .note.GNU-stack,\"\",%progbits\n";

/// `v` again, 16 bytes like `a.o`'s with other contents (W4 under strict).
const F: &str = "\t.section .data.v,\"awG\",%progbits,v,comdat\n\t.weak v\n\t.type v, %object\n\
    \t.size v, 16\nv:\t.quad 1\n\t.quad 3\n\t.section .note.GNU-stack,\"\",%progbits\n";

const W1: &str = "warning: v has different sizes in different files";
const W2: &str = "warning: f is a function in one file and data in another";
const W3: &str = "warning: g has a non-inline definition and an inline copy";
const W4: &str = "warning: v has different contents in different files";

struct Cx {
    scratch: Scratch,
}

impl Cx {
    fn new() -> Option<Self> {
        if std::env::consts::ARCH != "aarch64" {
            return None;
        }
        let Some(gcc) = tool("gcc") else {
            assert!(!tools_required(), "gcc is required");
            return None;
        };
        let cx = Self { scratch: Scratch::new("odr") };
        for (name, text) in [("a", A), ("b", B), ("c", C), ("d", D), ("e", E), ("f", F)] {
            let src = cx.scratch.write(&format!("{name}.s"), text);
            let args: Vec<OsString> =
                vec!["-c".into(), src.into(), "-o".into(), format!("{name}.o").into()];
            let o = cx.run(&gcc, &args);
            assert!(o.success(), "gcc {name}: {}", o.stderr_text());
        }
        Some(cx)
    }

    fn run(&self, exe: &std::path::Path, args: &[OsString]) -> Outcome {
        let mut c = Command::new(exe);
        c.args(args).current_dir(self.scratch.path()).env_remove("MORTAR_SCHED_SEED");
        run_with(&mut c, b"", Duration::from_mins(2))
    }

    /// mortar on `args`, which must succeed.
    fn mortar(&self, args: &[&str]) -> Outcome {
        let words: Vec<OsString> = args.iter().map(OsString::from).collect();
        let o = self.run(std::path::Path::new(MORTAR), &words);
        assert!(o.success(), "mortar {args:?}: {}", o.stderr_text());
        o
    }

    /// mortar's standard error for `inputs` with `extra`, to a fresh
    /// output `out`.
    fn stderr(&self, inputs: &[&str], extra: &[&str], out: &str) -> String {
        let mut args = inputs.to_vec();
        args.extend_from_slice(extra);
        args.extend(["-o", out]);
        self.mortar(&args).stderr_text()
    }

    fn output(&self, out: &str) -> Vec<u8> {
        std::fs::read(self.scratch.join(out)).unwrap()
    }

    fn path(&self, out: &str) -> PathBuf {
        self.scratch.join(out)
    }
}

// spec: odr.md O-40, O-41, §8.1 (W1 and W2 on by default, warnings in
// every mode; W3 and W4 only under strict; nothing under off)
#[test]
fn modes() {
    let Some(cx) = Cx::new() else { return };
    let all = ["a.o", "b.o", "c.o", "d.o", "e.o"];
    for (mode, out) in [("warn", "w.out"), ("error", "e.out")] {
        let e = cx.stderr(&all, &[&format!("--odr={mode}")], out);
        assert!(e.contains(W1) && e.contains(W2), "{mode}: {e}");
        assert!(!e.contains(W3) && !e.contains("error:"), "{mode}: {e}");
    }
    let e = cx.stderr(&all, &[], "d.out");
    assert!(e.contains(W1) && e.contains(W2) && !e.contains(W3), "{e}");
    assert!(e.contains(">>> defined in a.o:(v)\n>>> defined differently in b.o:(v)\n"), "{e}");
    assert!(e.contains(">>> defined in a.o:(f)\n>>> defined differently in c.o:(f)\n"), "{e}");
    let e = cx.stderr(&all, &["--odr=strict"], "s.out");
    assert!(e.contains(W1) && e.contains(W2) && e.contains(W3), "{e}");
    assert!(e.contains(">>> defined in d.o:(g)\n>>> defined differently in e.o:(g)\n"), "{e}");
    assert!(!e.contains("error:"), "{e}");
    let e = cx.stderr(&["a.o", "f.o"], &["--odr=strict"], "s4.out");
    assert!(e.contains(W4) && !e.contains(W1), "{e}");
    assert!(e.contains("differs from the first in its bytes"), "{e}");
    assert_eq!(cx.stderr(&["a.o", "f.o"], &[], "w4.out"), "");
    for off in [&["--odr=off"][..], &["--no-odr-check"], &["--odr=strict", "--no-odr-check"]] {
        assert_eq!(cx.stderr(&all, off, "o.out"), "", "{off:?}");
    }
    // The last of `--odr=` and `--no-odr-check` wins.
    let e = cx.stderr(&all, &["--no-odr-check", "--odr=warn"], "l.out");
    assert!(e.contains(W1), "{e}");
    // `--fatal-warnings` acts on them as on every warning (O-40).
    let mut args = all.to_vec();
    args.extend(["--fatal-warnings", "-o", "fw.out"]);
    let words: Vec<OsString> = args.iter().map(OsString::from).collect();
    let o = cx.run(std::path::Path::new(MORTAR), &words);
    assert!(!o.success() && o.stderr_text().contains("error: v has different sizes"));
    assert!(!cx.path("fw.out").exists());
}

// spec: odr.md O-41, O-42, T-O-41 (`--odr=off` reads nothing more and the
// output is the default link's, byte for byte, whatever the findings)
#[test]
fn off_reads_nothing_and_changes_no_byte() {
    let Some(cx) = Cx::new() else { return };
    let all = ["a.o", "b.o", "c.o", "d.o", "e.o"];
    cx.stderr(&all, &[], "warn.out");
    cx.stderr(&all, &["--odr=off"], "off.out");
    cx.stderr(&all, &["--no-odr-check"], "no.out");
    cx.stderr(&all, &["--odr=strict"], "strict.out");
    let warn = cx.output("warn.out");
    assert_eq!(warn, cx.output("off.out"));
    assert_eq!(warn, cx.output("no.out"));
    assert_eq!(warn, cx.output("strict.out"));
    let status = Command::new(cx.path("warn.out")).status().unwrap();
    assert_eq!(status.code(), Some(0));
}

// spec: odr.md O-44 (one line to standard error, after the diagnostics)
#[test]
fn stats_line() {
    let Some(cx) = Cx::new() else { return };
    let e = cx.stderr(&["a.o", "b.o", "c.o"], &["--odr-stats"], "st.out");
    let last = e.lines().last().unwrap_or_default();
    assert!(last.starts_with("mortar: odr-stats: mode warn; units 0, rows 0 (kind 1 0,"), "{e}");
    // O-42: the default reads no container.
    assert!(last.contains(", container bytes read 0, record findings 0;"), "{e}");
    assert!(last.contains("findings W1 1, W2 1, W3 0, W4 0;"), "{e}");
    assert_eq!(e.matches("odr-stats").count(), 1, "{e}");
    let e = cx.stderr(&["a.o", "b.o"], &["--odr=off", "--odr-stats"], "st0.out");
    assert!(e.starts_with("mortar: odr-stats: mode off;") && e.contains("W1 0,"), "{e}");
    assert!(e.contains(", container bytes read 0,"), "{e}");
    assert_eq!(cx.stderr(&["a.o", "b.o"], &["--odr=off"], "st1.out"), "");
}

// spec: odr.md O-41 (`--detect-odr-violations` is a no-op), cli.md §4.2
#[test]
fn detect_odr_violations_is_a_noop() {
    let Some(cx) = Cx::new() else { return };
    let all = ["a.o", "b.o", "c.o"];
    let plain = cx.stderr(&all, &[], "p.out");
    let with = cx.stderr(&all, &["--detect-odr-violations"], "dv.out");
    assert_eq!(plain, with);
    assert_eq!(cx.output("p.out"), cx.output("dv.out"));
    // It does not lower an error to a warning (review M11).
    let mut args = all.to_vec();
    args.extend(["--fatal-warnings", "--detect-odr-violations", "-o", "dvf.out"]);
    let words: Vec<OsString> = args.iter().map(OsString::from).collect();
    assert!(!cx.run(std::path::Path::new(MORTAR), &words).success());
}

// spec: model.md §8, diag.md §6.6 (the report does not depend on the
// thread count)
#[test]
fn threads_do_not_change_the_report() {
    let Some(cx) = Cx::new() else { return };
    let all = ["a.o", "b.o", "c.o", "d.o", "e.o", "f.o"];
    let one = cx.stderr(&all, &["--odr=strict", "--threads=1"], "t1.out");
    for threads in ["--threads=2", "--threads=4", "--threads=7"] {
        assert_eq!(one, cx.stderr(&all, &["--odr=strict", threads], "tn.out"), "{threads}");
    }
}
