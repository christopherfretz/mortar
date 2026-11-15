//! `cargo xtask linktest` end to end, with fake linkers
//! (`spec/harness.md` "Link tests": "Outcomes", "The self-check", "Exit
//! status").
//!
//! The reference linkers and mortar are scripts that run the host's GNU ld,
//! unless the directory they link in (a copy of the test's inputs) has a
//! file `<name>.fails` (they fail with a diagnostic) or `<name>.sleeps`
//! (they sleep past the test's time limit). The fakes of the references
//! come first on `PATH`, under the names the harness looks for; mortar's
//! is named with `--mortar`. Each outcome is then reachable from a test
//! file, and the self-check can be watched.
#![cfg(target_os = "linux")]

use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::Duration;

use mortar_testutil::{Scratch, run_with};
use xtask::linktest::target::Arch;

/// A fake linker named `name`, which runs `bfd` unless told otherwise.
fn fake(name: &str, bfd: &Path) -> String {
    format!(
        "if [ -e {name}.fails ]; then echo '{name}: error: told to fail' >&2; exit 1; fi\n\
         if [ -e {name}.sleeps ]; then sleep 60; fi\n\
         exec '{}' \"$@\"",
        bfd.display()
    )
}

/// A test of a static C program that prints `hello`, with `directives`
/// and the embedded `marks`.
fn program(arch: Arch, directives: &str, marks: &[&str]) -> String {
    let mut text = format!(
        "#@ spec: harness.md \"Link tests\"\n#@ arch: {}\n{directives}\
         cc main.c -o main.o\n\
         link gcc -static main.o -o main\n\
         run main\n  stdout: hello\n\
         check nm main\n  has: T main$\n\
         #--- main.c\n\
         #include <stdio.h>\nint main(void) {{ puts(\"hello\"); return 0; }}\n",
        arch.name()
    );
    for mark in marks {
        text.push_str("#--- ");
        text.push_str(mark);
        text.push('\n');
    }
    text
}

/// The records of a results file: outcome, test file name, architecture,
/// linker.
fn records(results: &Path) -> Vec<[String; 4]> {
    let text = std::fs::read_to_string(results).unwrap_or_default();
    text.lines()
        .map(|line| {
            let fields: Vec<&str> = line.split('\t').collect();
            assert_eq!(fields.len(), 6, "{line}");
            let file = Path::new(fields[1]).file_name().unwrap().to_string_lossy().into_owned();
            [fields[0].to_owned(), file, fields[2].to_owned(), fields[3].to_owned()]
        })
        .collect()
}

fn row(outcome: &str, test: &str, arch: &str, linker: &str) -> [String; 4] {
    [outcome.to_owned(), test.to_owned(), arch.to_owned(), linker.to_owned()]
}

struct Setup {
    scratch: Scratch,
    arch: Arch,
    path: String,
    mortar: PathBuf,
}

impl Setup {
    fn new() -> Option<Self> {
        let arch = Arch::host()?;
        let bfd = mortar_testutil::tool("ld.bfd")?;
        mortar_testutil::tool("gcc")?;
        let scratch = Scratch::new("linktest-e2e");
        std::fs::create_dir_all(scratch.join("bin")).unwrap();
        // The names `CommonTools` and `ArchTools` find first.
        for (file, name) in [("ld.bfd", "bfd"), ("ld.lld-22", "lld"), ("mold", "mold")] {
            scratch.write_script(&format!("bin/{file}"), &fake(name, &bfd));
        }
        let mortar = scratch.write_script("bin/mortar", &fake("mortar", &bfd));
        let path = format!(
            "{}:{}",
            scratch.join("bin").display(),
            std::env::var("PATH").unwrap_or_default()
        );
        std::fs::create_dir_all(scratch.join("tests")).unwrap();
        Some(Self { scratch, arch, path, mortar })
    }

    fn write(&self, name: &str, text: &str) -> PathBuf {
        self.scratch.write(&format!("tests/{name}"), text)
    }

    /// Runs `xtask linktest <args> <paths>`: the exit status, the records,
    /// the summary.
    fn linktest(
        &self,
        label: &str,
        args: &[&str],
        paths: &[PathBuf],
    ) -> (i32, Vec<[String; 4]>, String) {
        let results = self.scratch.join(format!("{label}.tsv"));
        let mut command = Command::new(env!("CARGO_BIN_EXE_xtask"));
        command.args(["linktest", "--arch", self.arch.name(), "-j", "4", "--mortar"]);
        command.arg(&self.mortar).arg("--results").arg(&results).args(args).args(paths);
        command.env("PATH", &self.path);
        let outcome = run_with(&mut command, b"", Duration::from_mins(10));
        let status = outcome.status.and_then(|status| status.code()).unwrap_or(-1);
        let summary = format!("{}{}", outcome.stdout_text(), outcome.stderr_text());
        (status, records(&results), summary)
    }
}

// spec/harness.md "Link tests", "Outcomes" and "The self-check": each
// outcome comes from what the linkers did; a reference that fails makes
// the test ORACLE and mortar is not judged; an expected failure that
// passes is XPASS and fails the run; a limit is KILLED whatever the
// marker.
#[test]
fn every_outcome_is_reachable_and_the_references_go_first() {
    let Some(setup) = Setup::new() else { return };
    let arch = setup.arch;
    let xfail = "#@ xfail(mortar): not yet\n";
    setup.write("xfail.test", &program(arch, xfail, &["mortar.fails"]));
    setup.write("xpass.test", &program(arch, xfail, &[]));
    setup.write("oracle.test", &program(arch, xfail, &["lld.fails", "mortar.fails"]));
    setup.write(
        "killed.test",
        &program(arch, &format!("{xfail}#@ timeout: 5\n"), &["mortar.sleeps"]),
    );
    setup.write("fail.test", &program(arch, "", &["mortar.fails"]));
    setup.write("skip.test", &program(arch, "#@ skip(mold): not this one\n", &[]));
    setup.write(
        "refused.test",
        &format!(
            "#@ spec: harness.md \"Link tests\"\n#@ arch: {}\n\
             cc main.c -o main.o\n\
             link gcc -static main.o -o main\n  exit: fail\n  diag: undefined reference to `foo'\n\
             #--- main.c\nint foo(void);\nint main(void) {{ return foo(); }}\n",
            arch.name()
        ),
    );
    setup.write(
        "reference.test",
        &format!(
            "#@ spec: harness.md \"Link tests\"\n#@ arch: {}\n\
             cc main.c -o main.o\nlink gcc -static main.o -o main\nrun main\n  output: reference\n\
             #--- main.c\n#include <stdio.h>\nint main(void) {{ puts(\"same\"); return 0; }}\n",
            arch.name()
        ),
    );
    setup.write("broken.test", "link ld a.o\n");
    let (status, records, summary) = setup.linktest("all", &[], &[setup.scratch.join("tests")]);
    let a = arch.name();
    let mut want = Vec::new();
    want.push(row("ERROR", "broken.test", "-", "-"));
    for linker in ["bfd", "lld", "mold"] {
        want.push(row("PASS", "fail.test", a, linker));
    }
    want.push(row("FAIL", "fail.test", a, "mortar"));
    for linker in ["bfd", "lld", "mold"] {
        want.push(row("PASS", "killed.test", a, linker));
    }
    want.push(row("KILLED", "killed.test", a, "mortar"));
    want.push(row("PASS", "oracle.test", a, "bfd"));
    want.push(row("FAIL", "oracle.test", a, "lld"));
    want.push(row("PASS", "oracle.test", a, "mold"));
    want.push(row("ORACLE", "oracle.test", a, "-"));
    for linker in ["bfd", "lld", "mold", "mortar"] {
        want.push(row("PASS", "reference.test", a, linker));
    }
    for linker in ["bfd", "lld", "mold", "mortar"] {
        want.push(row("PASS", "refused.test", a, linker));
    }
    want.push(row("PASS", "skip.test", a, "bfd"));
    want.push(row("PASS", "skip.test", a, "lld"));
    want.push(row("SKIP", "skip.test", a, "mold"));
    want.push(row("PASS", "skip.test", a, "mortar"));
    for linker in ["bfd", "lld", "mold"] {
        want.push(row("PASS", "xfail.test", a, linker));
    }
    want.push(row("XFAIL", "xfail.test", a, "mortar"));
    for linker in ["bfd", "lld", "mold"] {
        want.push(row("PASS", "xpass.test", a, linker));
    }
    want.push(row("XPASS", "xpass.test", a, "mortar"));
    assert_eq!(records, want, "{summary}");
    assert_eq!(status, 1, "{summary}");
    assert!(summary.contains("KILLED  ") && summary.contains("killed: time"), "{summary}");
    assert!(summary.contains("passes: remove its xfail (not yet)"), "{summary}");
    assert!(summary.contains("lld: error: told to fail"), "{summary}");
    assert!(summary.contains("no 'spec' directive"), "{summary}");

    // A wrong test alone: exit status 3, and the failing reference's
    // record is that wrong test's.
    let oracle = setup.scratch.join("tests/oracle.test");
    let (status, records, summary) = setup.linktest("oracle", &[], std::slice::from_ref(&oracle));
    assert_eq!(status, 3, "{summary}");
    assert!(records.iter().all(|r| r[3] != "mortar"), "mortar is not judged: {records:?}");

    // Everything as the test says: exit status 0.
    let good = setup.scratch.join("tests/xfail.test");
    let (status, _, summary) = setup.linktest("good", &[], std::slice::from_ref(&good));
    assert_eq!(status, 0, "{summary}");
    assert!(summary.contains("1 tests, 4 results: 3 PASS, 0 FAIL, 1 XFAIL"), "{summary}");

    // Without a reference in the run nothing is proven, and the run says
    // so; mortar is judged by the test's own expectations.
    let (status, records, summary) =
        setup.linktest("alone", &["--linker", "mortar"], &[good, oracle]);
    assert_eq!(status, 0, "{summary}");
    assert_eq!(
        records,
        [row("XFAIL", "oracle.test", a, "mortar"), row("XFAIL", "xfail.test", a, "mortar")]
    );
    assert!(summary.contains("note: no reference linker in the run"), "{summary}");

    // Usage errors.
    let (status, _, summary) = setup.linktest("usage", &["--linker", "gold"], &[]);
    assert_eq!(status, 2, "{summary}");
    assert!(summary.contains("usage: cargo xtask linktest"), "{summary}");
    let (status, _, summary) = setup.linktest("missing", &[], &[setup.scratch.join("nothing")]);
    assert_eq!(status, 2, "{summary}");
}
