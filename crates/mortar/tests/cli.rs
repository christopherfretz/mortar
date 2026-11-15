//! The `mortar` executable as a build sees it (`spec/diag.md` §7): what a
//! failed link prints, where, and what it leaves behind.

use std::process::Command;
use std::time::Duration;

use mortar_testutil::{Scratch, run, run_with};

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
        "mortar: error: unsupported feature: linking\n\
         >>> no link is implemented yet (docs/ROADMAP.md, milestone M1)\n"
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
            stderr.starts_with(&format!("{name}: error: unsupported feature: linking\n")),
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
