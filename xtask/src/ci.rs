//! The steps of `cargo xtask ci` (`spec/harness.md` "ci"): three cargo
//! steps, ported from hod's `xtask ci`, which met the same build directory,
//! then the link tests and the static-identical check, run in-process.

use std::process::Command;

use crate::jobs::children::run_step;

/// Should a cargo step that failed with `stderr` be run again
/// (`spec/harness.md` "ci")? Once, and only for the two errors rustc gives
/// when a dependency's artifact is not in the build directory although
/// cargo had just written it, which the workspace's file system does to a
/// build under load (its directory listings lag behind fresh files):
///
/// - ``error[E0463]: can't find crate for `X` ``: neither the rlib nor the
///   rmeta of a dependency found by `-L dependency=` was listed;
/// - ``error: crate `X` required to be available in rlib format, but was not
///   found in this form``: its rmeta was listed (written earlier, at the end
///   of analysis) but its rlib (renamed into place at the end of codegen)
///   not yet, and a binary is being linked.
///
/// A rerun finds the artifacts, or rebuilds them. Every other failure is
/// real.
pub fn should_retry(stderr: &str, retried: bool) -> bool {
    !retried && stderr.lines().any(|line| vanished_artifact(line).is_some())
}

/// The rustc error that `line` is, if it is one of those [`should_retry`]
/// retries: its short name for the note.
fn vanished_artifact(line: &str) -> Option<&'static str> {
    let line = line.trim();
    if line.starts_with("error[E0463]") {
        return Some("error[E0463] (can't find crate)");
    }
    let rlib = line.strip_prefix("error: crate `")?.strip_suffix(
        "` required to be available in rlib format, but was not found in this form",
    )?;
    (!rlib.is_empty() && !rlib.contains(char::is_whitespace))
        .then_some("crate required in rlib format")
}

/// The cargo that runs the steps: the one that runs xtask (`CARGO`, which
/// cargo sets for what it runs), else `cargo` on `PATH`.
fn cargo() -> std::ffi::OsString {
    std::env::var_os("CARGO").unwrap_or_else(|| "cargo".into())
}

/// Runs `cargo <args>`, echoing its standard error while reading it, and
/// runs it once more when it failed as [`should_retry`] describes. Returns
/// whether the step passed.
///
/// The step is a jobserver client of its own and is stopped with `xtask`
/// ([`run_step`], `spec/harness.md` "Jobserver").
pub fn cargo_step(args: &[&str]) -> bool {
    let shown = args.join(" ");
    let mut retried = false;
    loop {
        let mut cargo = Command::new(cargo());
        let (status, stderr) = match run_step(cargo.args(args), true) {
            Ok(result) => result,
            Err(e) => {
                eprintln!("[ci] cannot run cargo {shown}: {e}");
                return false;
            }
        };
        if status.success() {
            return true;
        }
        if !should_retry(&stderr, retried) {
            eprintln!("[ci] cargo {shown}: {status}");
            return false;
        }
        let why = stderr.lines().find_map(vanished_artifact).unwrap_or("a missing artifact");
        eprintln!(
            "[ci] [retry] cargo {shown} failed with {why}: an artifact was not visible to rustc \
             mid-build; running it once more"
        );
        retried = true;
    }
}

/// The steps of `cargo xtask ci`, in the order they run, each with its
/// `cargo` arguments (`spec/harness.md` "ci").
pub const STEPS: [(&str, &[&str]); 3] = [
    ("fmt", &["fmt", "--all", "--check"]),
    ("clippy", &["clippy", "--workspace", "--all-targets", "--", "-D", "warnings"]),
    ("test", &["test", "--workspace"]),
];

/// The link-test step, which runs after the cargo steps: `cargo xtask
/// linktest` in-process (`spec/harness.md` "ci", "Link tests").
pub const LINKTEST: &str = "linktest";

/// The static-identical step, last: `cargo xtask static-identical`
/// in-process, on every architecture whatever `--quick` says
/// (`spec/harness.md` "static-identical").
pub const STATIC_IDENTICAL: &str = "static-identical";

/// Every step's name, in the order they run.
pub const STEP_NAMES: [&str; 5] = [STEPS[0].0, STEPS[1].0, STEPS[2].0, LINKTEST, STATIC_IDENTICAL];

/// The arguments of the link-test step: the host's architecture only for
/// `--quick` (when it is one of the three), every architecture otherwise.
pub fn linktest_args(quick: bool) -> Vec<String> {
    match crate::linktest::target::Arch::host() {
        Some(host) if quick => vec!["--arch".to_owned(), host.name().to_owned()],
        _ => Vec::new(),
    }
}

/// What `cargo xtask ci [--quick] [--step <name>]...` asks for.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Args {
    pub quick: bool,
    /// The steps to run, in [`STEP_NAMES`] order; every step when none is
    /// named.
    pub steps: Vec<&'static str>,
}

impl Args {
    /// Parses the arguments after `ci`.
    ///
    /// # Errors
    /// An unknown argument or step, or `--step` without a name.
    pub fn parse(args: &[String]) -> Result<Self, String> {
        let mut quick = false;
        let mut named: Vec<String> = Vec::new();
        let mut rest = args.iter();
        while let Some(arg) = rest.next() {
            match arg.as_str() {
                "--quick" => quick = true,
                "--step" => named.push(rest.next().ok_or("--step needs a step name")?.clone()),
                other => match other.strip_prefix("--step=") {
                    Some(step) => named.push(step.to_owned()),
                    None => return Err(format!("unknown argument `{other}`")),
                },
            }
        }
        let known = STEP_NAMES;
        if let Some(unknown) = named.iter().find(|step| !known.contains(&step.as_str())) {
            return Err(format!("unknown step `{unknown}` (steps: {})", known.join(", ")));
        }
        let steps = known
            .into_iter()
            .filter(|step| named.is_empty() || named.iter().any(|n| n == step))
            .collect();
        Ok(Self { quick, steps })
    }

    /// Whether `step` runs.
    pub fn runs(&self, step: &str) -> bool {
        self.steps.contains(&step)
    }
}

/// Runs each selected step in order and stops at the first failure
/// (`spec/harness.md` "ci"). `--quick` is what an implementer runs before
/// handing back: the link tests on the host's architecture only; the full
/// gate runs them on every architecture, and adds the differential lanes
/// as they land (`docs/VALIDATION.md`).
pub fn run(args: &Args) -> bool {
    for name in STEP_NAMES {
        if !args.runs(name) {
            continue;
        }
        eprintln!("[ci] {name}");
        let passed = match STEPS.iter().find(|(step, _)| *step == name) {
            // A cargo step is a jobserver client of its own
            // (spec/harness.md "Jobserver"), and is run once more if an
            // artifact vanished under it ("ci").
            Some((_, cargo_args)) => cargo_step(cargo_args),
            // Any exit status but 0 fails the gate ("Link tests", "Exit
            // status").
            None if name == LINKTEST => crate::linktest::main(&linktest_args(args.quick)) == 0,
            None => crate::identical::run(&crate::identical::Args::default()),
        };
        if !passed {
            eprintln!("[ci] {name}: FAILED");
            return false;
        }
    }
    eprintln!("[ci] ok");
    true
}

#[cfg(test)]
mod tests {
    use super::{Args, STEP_NAMES, STEPS, linktest_args, should_retry};

    // spec/harness.md "ci": `--step` selects steps, which run in the
    // gate's order whatever the order they are named in.
    #[test]
    fn steps_are_selected_by_name() {
        let parse =
            |args: &[&str]| Args::parse(&args.iter().map(|a| (*a).to_owned()).collect::<Vec<_>>());
        let all = parse(&["--quick"]).unwrap();
        assert!(all.quick);
        assert_eq!(all.steps, STEP_NAMES);
        assert_eq!(all.steps, ["fmt", "clippy", "test", "linktest", "static-identical"]);
        assert_eq!(parse(&[]).unwrap().steps, all.steps);
        let two = parse(&["--step", "test", "--step=fmt"]).unwrap();
        assert!(!two.quick);
        assert_eq!(two.steps, ["fmt", "test"]);
        assert!(two.runs("fmt") && !two.runs("clippy") && !two.runs("linktest"));
        assert_eq!(parse(&["--step", "linktest"]).unwrap().steps, ["linktest"]);
        assert!(parse(&["--step", "lint"]).unwrap_err().contains("unknown step `lint`"));
        assert!(parse(&["--step"]).is_err());
        assert!(parse(&["--fast"]).unwrap_err().contains("unknown argument `--fast`"));
    }

    // spec/harness.md "ci": the steps are cargo's own checks of the whole
    // workspace, with warnings denied, then the link tests: on the host's
    // architecture for `--quick`, on every one for the full gate.
    #[test]
    fn the_steps_are_fmt_clippy_test_and_linktest() {
        assert!(linktest_args(false).is_empty());
        if let Some(host) = crate::linktest::target::Arch::host() {
            assert_eq!(linktest_args(true), ["--arch", host.name()]);
        }
        let commands: Vec<String> = STEPS.iter().map(|(_, args)| args.join(" ")).collect();
        assert_eq!(
            commands,
            [
                "fmt --all --check",
                "clippy --workspace --all-targets -- -D warnings",
                "test --workspace",
            ]
        );
    }

    // spec/harness.md "ci": one rerun, only for a missing crate, and only
    // when rustc said so at the start of a line.
    #[test]
    fn retries_only_missing_crates_once() {
        let missing = "   Compiling mortar_elf v0.0.0\n\
                       error[E0463]: can't find crate for `mortar_elf`\n \
                       --> crates/mortar/tests/common/mod.rs:7:5\n";
        assert!(should_retry(missing, false));
        assert!(!should_retry(missing, true), "a second failure is real");
        assert!(should_retry("warning: x\n  error[E0463]: can't find crate for `std`\n", false));
        assert!(!should_retry("error[E0308]: mismatched types\n", false));
        assert!(!should_retry("test result: FAILED. 1 passed; 1 failed\n", false));
        assert!(!should_retry("note: the test checks that error[E0463] is retried\n", false));
        assert!(!should_retry("", false));
    }

    // spec/harness.md "ci": the rlib form of the same cause, exactly as
    // rustc prints it, is retried once; its relatives are not.
    #[test]
    fn retries_a_crate_seen_only_as_rmeta_once() {
        let rlib = "   Compiling mortar v0.0.0\n\
                    error: crate `mortar_elf` required to be available in rlib format, but was \
                    not found in this form\n\n\
                    error: could not compile `mortar` (test \"cli\") due to 1 previous error\n";
        assert!(should_retry(rlib, false));
        assert!(!should_retry(rlib, true), "a second failure is real");
        let unit = "error: crate `mortar_elf` required to be available in dylib format, but was \
                    not found in this form\n";
        assert!(!should_retry(unit, false), "mortar links no dylib: that is a real error");
        assert!(!should_retry(
            "error: crate `` required to be available in rlib format, but was \
                               not found in this form\n",
            false
        ));
        assert!(!should_retry(
            "note: error: crate `a` required to be available in rlib format, \
                               but was not found in this form\n",
            false
        ));
        assert!(!should_retry("error: crate `a` required to be available in rlib format\n", false));
        assert!(!should_retry("error: linking with `cc` failed: exit status: 1\n", false));
    }
}
