//! The `mortar` executable as a build sees it (`spec/diag.md` §7): what a
//! failed link prints, where, and what it leaves behind.

use std::process::Command;
use std::time::Duration;

use mortar_testutil::{DEFAULT_TIMEOUT, Scratch, run, run_with, tool};

const MORTAR: &str = env!("CARGO_BIN_EXE_mortar");

// spec: diag.md §7 (a failed link: the report on standard error, nothing
// on standard output, status 1, no output file), §4 (refused by name)
#[test]
fn a_failed_link_reports_on_stderr_and_leaves_no_output() {
    let scratch = Scratch::new("cli-fail");
    let output = scratch.join("a.out");
    let outcome = run(MORTAR, [scratch.join("a.o").as_os_str(), "-o".as_ref(), output.as_os_str()]);
    assert_eq!(outcome.status.and_then(|s| s.code()), Some(1));
    assert_eq!(outcome.stdout_text(), "");
    assert_eq!(
        outcome.stderr_text(),
        format!(
            "mortar: error: unsupported option: -o\n\
             mortar: error: {a}: cannot open {a}: No such file or directory\n\
             mortar: error: target emulation unknown: -m or at least one .o file required\n",
            a = scratch.join("a.o").display()
        )
    );
    assert!(!output.exists(), "a failed link leaves no output file");
}

// spec: diag.md §5.1 (diagnostics start with the name mortar was invoked
// as)
#[cfg(unix)]
#[test]
fn diagnostics_start_with_the_invoked_name() {
    let scratch = Scratch::new("cli-name");
    for name in ["ld", "ld.mortar"] {
        let link = scratch.join(name);
        std::os::unix::fs::symlink(MORTAR, &link).expect("symlink in the scratch directory");
        let outcome = run_with(Command::new(&link).arg("a.o"), &[], Duration::from_mins(1));
        assert_eq!(outcome.status.and_then(|s| s.code()), Some(1));
        assert_eq!(outcome.stdout_text(), "");
        let stderr = outcome.stderr_text();
        assert!(
            stderr.starts_with(&format!("{name}: error: a.o: cannot open a.o")),
            "{stderr:?}"
        );
    }
}

// `--version` is answered on standard output with status 0 (WP-001).
#[test]
fn version_goes_to_stdout() {
    let outcome = run(MORTAR, ["--version"]);
    assert!(outcome.success());
    assert_eq!(
        outcome.stdout_text(),
        format!("mortar {} (compatible with GNU ld)\n", env!("CARGO_PKG_VERSION"))
    );
    assert_eq!(outcome.stderr_text(), "");
}

// spec: cli.md §2.1 (arguments are bytes: one that is not UTF-8 is read,
// and shown lossily)
#[cfg(unix)]
#[test]
fn a_non_utf8_argument_is_read() {
    use std::os::unix::ffi::OsStrExt;
    let name = std::ffi::OsStr::from_bytes(b"caf\xe9.o");
    let option = std::ffi::OsStr::from_bytes(b"--frob\xff");
    let outcome = run(MORTAR, [name, option]);
    assert_eq!(outcome.status.and_then(|s| s.code()), Some(1));
    assert_eq!(outcome.stdout_text(), "");
    assert!(
        outcome.stderr_text().starts_with("mortar: error: unknown argument '--frob\u{fffd}'\n"),
        "{:?}",
        outcome.stderr_text()
    );
}

// spec: cli.md §5.1 (`ld.mortar --version`, as compiler drivers probe it
// through `-fuse-ld=mortar`)
#[cfg(unix)]
#[test]
fn version_through_a_symlink() {
    let scratch = Scratch::new("cli-version-link");
    let link = scratch.join("ld.mortar");
    std::os::unix::fs::symlink(MORTAR, &link).expect("symlink in the scratch directory");
    let outcome = run_with(Command::new(&link).arg("--version"), &[], Duration::from_mins(1));
    assert!(outcome.success());
    assert_eq!(
        outcome.stdout_text(),
        format!("mortar {} (compatible with GNU ld)\n", env!("CARGO_PKG_VERSION"))
    );
    assert_eq!(outcome.stderr_text(), "");
}

// spec: cli.md §6, diag.md §7 (gcc runs mortar as its `ld`: the front
// lays the link out, then the output file is refused — WP-027b writes it —
// so the link fails with status 1 and leaves no output)
#[cfg(unix)]
#[test]
fn a_gcc_link_through_mortar_fails_cleanly() {
    let Some(gcc) = tool("gcc") else { return };
    let scratch = Scratch::new("cli-gcc");
    let bin = scratch.join("bin");
    std::fs::create_dir_all(&bin).expect("mkdir bin");
    std::os::unix::fs::symlink(MORTAR, bin.join("ld")).expect("symlink in the scratch directory");
    let src = scratch.write("h.c", "int main(void) { return 0; }\n");
    let output = scratch.join("h");
    let outcome = run_with(
        Command::new(gcc).arg(format!("-B{}/", bin.display())).arg("-static").arg(&src).arg("-o").arg(&output),
        &[],
        DEFAULT_TIMEOUT,
    );
    assert_eq!(outcome.status.and_then(|s| s.code()), Some(1), "{}", outcome.stderr_text());
    let stderr = outcome.stderr_text();
    assert!(
        stderr.contains(
            "ld: error: unsupported option: -o\n\
             ld: error: unsupported feature: writing the output file\n\
             >>> the link was laid out; the image and the output file are WP-027b\n"
        ),
        "{stderr}"
    );
    assert!(!output.exists(), "a failed link leaves no output file");
}
