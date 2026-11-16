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
        // Where the test binaries record their named skips.
        cargo.env(mortar_testutil::NAMED_SKIPS_ENV, named_skips_dir());
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

/// The census step, after the link tests: `cargo xtask census --quick`
/// (`--quick`) or `cargo xtask census` (full) in-process
/// (`spec/harness.md` "ci", "census").
pub const CENSUS: &str = "census";

/// The Mason step, after the census: libc++'s `ci` sample through the
/// Mason lane, static and dynamic (`crate::mason::ci`; the full gate adds
/// the programs table), skipped by name without kiln and hod builds
/// (`spec/harness.md` "ci").
pub const MASON: &str = "mason";

/// The sweep step, after the Mason step: `cargo xtask sweep --quick`
/// (`--quick`) or `cargo xtask sweep` (full) in-process
/// (`spec/harness.md` "ci").
pub const SWEEP: &str = "sweep";

/// Every step's name, in the order they run.
pub const STEP_NAMES: [&str; 8] =
    [STEPS[0].0, STEPS[1].0, STEPS[2].0, LINKTEST, CENSUS, MASON, SWEEP, STATIC_IDENTICAL];

/// The arguments of the census step.
pub fn census_args(quick: bool) -> Vec<String> {
    if quick { vec!["--quick".to_owned()] } else { Vec::new() }
}

/// The arguments of the sweep step.
pub fn sweep_args(quick: bool) -> Vec<String> {
    census_args(quick)
}

/// The arguments of the link-test step: the host's architecture only for
/// `--quick` (when it is one of the three), every architecture otherwise.
pub fn linktest_args(quick: bool) -> Vec<String> {
    match crate::linktest::target::Arch::host() {
        Some(host) if quick => vec!["--arch".to_owned(), host.name().to_owned()],
        _ => Vec::new(),
    }
}

/// What `cargo xtask ci [--quick] [--no-fail-fast] [--step <name>]...`
/// asks for.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Args {
    pub quick: bool,
    /// Every selected step runs whatever an earlier one did, and `cargo
    /// test` runs every test binary (`--no-fail-fast`): a diagnosis run
    /// that shows every failure at once (`spec/harness.md` "ci").
    pub no_fail_fast: bool,
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
        let mut no_fail_fast = false;
        let mut named: Vec<String> = Vec::new();
        let mut rest = args.iter();
        while let Some(arg) = rest.next() {
            match arg.as_str() {
                "--quick" => quick = true,
                "--no-fail-fast" => no_fail_fast = true,
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
        Ok(Self { quick, no_fail_fast, steps })
    }

    /// Whether `step` runs.
    pub fn runs(&self, step: &str) -> bool {
        self.steps.contains(&step)
    }
}

/// The arguments of cargo step `name`: [`STEPS`]'s, and `--no-fail-fast`
/// for `test` when asked for.
pub fn cargo_args(name: &str, no_fail_fast: bool) -> Option<Vec<&'static str>> {
    let (_, args) = STEPS.iter().find(|(step, _)| *step == name)?;
    let mut args = args.to_vec();
    if no_fail_fast && name == "test" {
        args.push("--no-fail-fast");
    }
    Some(args)
}

/// Where the test binaries record their named skips
/// ([`mortar_testutil::named_skip`]): `MORTAR_NAMED_SKIPS` when set, else
/// `<build directory>/tmp/named-skips`. A gate run by a test (xtask's own
/// tests) is given a directory of its own: the build directory's is the
/// outer gate's, which it empties before its test step and counts after.
pub fn named_skips_dir() -> std::path::PathBuf {
    std::env::var_os(mortar_testutil::NAMED_SKIPS_ENV).map_or_else(
        || crate::linktest::build_dir().join("tmp").join(mortar_testutil::NAMED_SKIPS),
        std::path::PathBuf::from,
    )
}

/// The named skips recorded in `dir`, one `<name>\t<reason>` line each,
/// sorted.
pub fn named_skips(dir: &std::path::Path) -> Vec<String> {
    let Ok(entries) = std::fs::read_dir(dir) else { return Vec::new() };
    let mut lines: Vec<String> = entries
        .flatten()
        .filter_map(|entry| std::fs::read_to_string(entry.path()).ok())
        .flat_map(|text| text.lines().map(str::to_owned).collect::<Vec<_>>())
        .collect();
    lines.sort();
    lines
}

/// Runs each selected step in order and stops at the first failure, or
/// with `--no-fail-fast` runs them all (`spec/harness.md` "ci").
/// `--quick` is what an implementer runs before handing back: the link
/// tests on the host's architecture only; the full gate runs them on
/// every architecture, and adds the differential lanes as they land
/// (`docs/VALIDATION.md`). After the test step it lists and counts the
/// named skips the tests recorded ("Named skips").
pub fn run(args: &Args) -> bool {
    let mut failed: Vec<&str> = Vec::new();
    for name in STEP_NAMES {
        if !args.runs(name) {
            continue;
        }
        eprintln!("[ci] {name}");
        let skips = named_skips_dir();
        if name == "test" {
            let _ = std::fs::remove_dir_all(&skips);
        }
        let passed = match cargo_args(name, args.no_fail_fast) {
            // A cargo step is a jobserver client of its own
            // (spec/harness.md "Jobserver"), and is run once more if an
            // artifact vanished under it ("ci").
            Some(cargo_args) => cargo_step(&cargo_args),
            // Any exit status but 0 fails the gate ("Link tests", "Exit
            // status").
            None if name == LINKTEST => crate::linktest::main(&linktest_args(args.quick)) == 0,
            #[cfg(unix)]
            None if name == CENSUS => crate::census::main(&census_args(args.quick)) == 0,
            None if name == MASON => crate::mason::ci(args.quick),
            #[cfg(unix)]
            None if name == SWEEP => crate::sweep::main(&sweep_args(args.quick)) == 0,
            None => crate::identical::run(&crate::identical::Args::default()),
        };
        if name == "test" {
            let lines = named_skips(&skips);
            eprintln!("[ci] named skips: {}", lines.len());
            for line in &lines {
                eprintln!("[ci]   {}", line.replace('\t', ": "));
            }
        }
        if !passed {
            eprintln!("[ci] {name}: FAILED");
            if !args.no_fail_fast {
                return false;
            }
            failed.push(name);
        }
    }
    if !failed.is_empty() {
        eprintln!("[ci] FAILED: {}", failed.join(", "));
        return false;
    }
    eprintln!("[ci] ok");
    true
}

#[cfg(test)]
mod tests {
    use super::{
        Args, STEP_NAMES, STEPS, cargo_args, linktest_args, named_skips, should_retry, sweep_args,
    };

    // spec/harness.md "ci": `--step` selects steps, which run in the
    // gate's order whatever the order they are named in.
    #[test]
    fn steps_are_selected_by_name() {
        let parse =
            |args: &[&str]| Args::parse(&args.iter().map(|a| (*a).to_owned()).collect::<Vec<_>>());
        let all = parse(&["--quick"]).unwrap();
        assert!(all.quick);
        assert_eq!(all.steps, STEP_NAMES);
        assert_eq!(
            all.steps,
            ["fmt", "clippy", "test", "linktest", "census", "mason", "sweep", "static-identical"]
        );
        assert_eq!(parse(&[]).unwrap().steps, all.steps);
        let two = parse(&["--step", "test", "--step=fmt"]).unwrap();
        assert!(!two.quick);
        assert_eq!(two.steps, ["fmt", "test"]);
        assert!(two.runs("fmt") && !two.runs("clippy") && !two.runs("linktest"));
        assert_eq!(parse(&["--step", "linktest"]).unwrap().steps, ["linktest"]);
        assert!(parse(&["--step", "lint"]).unwrap_err().contains("unknown step `lint`"));
        assert!(parse(&["--step"]).is_err());
        assert!(parse(&["--fast"]).unwrap_err().contains("unknown argument `--fast`"));
        assert!(!all.no_fail_fast);
        let every = parse(&["--no-fail-fast", "--step", "test"]).unwrap();
        assert!(every.no_fail_fast && every.steps == ["test"]);
    }

    // spec/harness.md "ci": `--no-fail-fast` reaches `cargo test` only.
    #[test]
    fn no_fail_fast_reaches_cargo_test() {
        assert_eq!(cargo_args("test", true).unwrap(), ["test", "--workspace", "--no-fail-fast"]);
        assert_eq!(cargo_args("test", false).unwrap(), ["test", "--workspace"]);
        assert_eq!(cargo_args("fmt", true).unwrap(), ["fmt", "--all", "--check"]);
        assert!(cargo_args("linktest", true).is_none());
    }

    // spec/harness.md "Named skips": the records of a test run, one line
    // each, sorted; none when the directory is not there.
    #[test]
    fn named_skips_are_listed_sorted() {
        let scratch = mortar_testutil::Scratch::new("named-skips");
        std::fs::write(scratch.join("b"), "b::x\tnot here\n").unwrap();
        std::fs::write(scratch.join("a"), "a::y\toff its host\n").unwrap();
        assert_eq!(named_skips(scratch.path()), ["a::y\toff its host", "b::x\tnot here"]);
        assert!(named_skips(&scratch.join("none")).is_empty());
    }

    // spec/harness.md "ci": the steps are cargo's own checks of the whole
    // workspace, with warnings denied, then the link tests: on the host's
    // architecture for `--quick`, on every one for the full gate.
    #[test]
    fn the_steps_are_fmt_clippy_test_and_linktest() {
        assert!(linktest_args(false).is_empty());
        assert_eq!(sweep_args(true), ["--quick"]);
        assert!(sweep_args(false).is_empty());
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
