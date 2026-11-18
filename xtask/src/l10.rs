//! `cargo xtask l10`: L10, the random edit-sequence lane of incremental
//! linking (`spec/incr.md` IN-63, `spec/harness.md` "L10", WP-067). The
//! lane itself is `tests/l10/l10.py` (it judges each step with
//! `tests/l10/judge.py`, which calls the IN-63a oracle of
//! `tests/incr-oracle/oracle.py` as a library and this xtask's `plumb`
//! and `plumb --equiv`); this command builds mortar (and the floor's
//! example when a run needs it), holds `-j` jobserver tokens for the
//! lane's workers and runs it.
//!
//!   cargo xtask l10 [--quick] [--seed N]... [--steps N] [--hops 2|3] [--project P,...]
//!                   [--compiler gcc|clang|kiln,...] [--target a64|x64|i386,...]
//!                   [--config NAME,...] [--edits ROW,...] [--subject mortar|floor|PATH]
//!                   [-j N] [--keep] [--replay FILE]... [--regressions]
//!                   [--minimize FILE] [--self-test] [--mortar PATH]
//!
//! `--quick` is the pre-merge smoke (the gate runs it after the link
//! tests); without it, the nightly matrix with the regressions and the
//! lane's own test.

use std::path::PathBuf;
use std::process::{Command, ExitCode};

use crate::process;

pub const USAGE: &str = "usage: cargo xtask l10 [--quick] [--seed <n>]... [--steps <n>] [--hops 2|3] \
                         [--project <p>,...] \
                         [--compiler gcc|clang|kiln,...] [--target a64|x64|i386,...] [--config <name>,...] \
                         [--edits <row>,...] [--subject mortar|floor|<path>] [-j <n>] [--keep] \
                         [--replay <file>]... [--regressions] [--minimize <file>] [--self-test] [--mortar <path>]\n";

/// The arguments that take a value, passed to the lane as they are.
const VALUED: [&str; 13] = [
    "--seed",
    "--steps",
    "--hops",
    "--project",
    "--compiler",
    "--target",
    "--config",
    "--edits",
    "--subject",
    "-j",
    "--replay",
    "--minimize",
    "--floor",
];

/// The arguments that take none.
const FLAGS: [&str; 4] = ["--quick", "--keep", "--self-test", "--regressions"];

/// What the command line asks for, split into the lane's words and ours.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct Args {
    pub pass: Vec<String>,
    pub mortar: Option<PathBuf>,
    pub jobs: usize,
    pub quick: bool,
    /// The run needs the floor's example (`--subject floor`, the self test,
    /// the nightly run).
    pub floor: bool,
    pub nightly: bool,
}

impl Args {
    /// # Errors
    /// An unknown argument, or one without its value.
    pub fn parse(args: &[String]) -> Result<Args, String> {
        let mut a = Args { jobs: 4, ..Args::default() };
        let mut i = 0;
        while i < args.len() {
            let w = args[i].as_str();
            if FLAGS.contains(&w) {
                a.quick |= w == "--quick";
                a.floor |= w == "--self-test";
                a.pass.push(w.to_owned());
            } else if VALUED.contains(&w) || w == "--mortar" {
                let v = args.get(i + 1).ok_or_else(|| format!("{w} needs a value"))?;
                i += 1;
                match w {
                    "--mortar" => a.mortar = Some(PathBuf::from(v)),
                    "-j" => {
                        a.jobs = v.parse().map_err(|_| format!("-j {v}: not a number"))?;
                    }
                    _ => {
                        a.floor |= w == "--subject" && v == "floor";
                        a.pass.push(w.to_owned());
                        a.pass.push(v.clone());
                    }
                }
            } else {
                return Err(format!("unknown argument `{w}`"));
            }
            i += 1;
        }
        // `cargo xtask l10` with nothing but -j, --mortar or --keep is the
        // nightly run.
        a.nightly = a.pass.iter().all(|w| w == "--keep");
        a.floor |= a.nightly;
        a.jobs = a.jobs.max(1);
        Ok(a)
    }
}

/// The gate's step (`cargo xtask ci`, after the link tests): the smoke.
pub fn ci(_quick: bool) -> bool {
    main(&["--quick".to_owned()]) == ExitCode::SUCCESS
}

fn floor_binary() -> Result<PathBuf, String> {
    let cargo = std::env::var_os("CARGO").unwrap_or_else(|| "cargo".into());
    let mut command = Command::new(cargo);
    command.args(["build", "--quiet", "--package", "mortar", "--example", "incr_floor"]);
    match process::run_step(&mut command, true) {
        Ok((status, _)) if status.success() => {}
        Ok((status, _)) => return Err(format!("cannot build incr_floor: cargo build: {status}")),
        Err(e) => return Err(format!("cannot build incr_floor: {e}")),
    }
    let p = crate::linktest::build_dir().join("debug").join("examples").join("incr_floor");
    if p.is_file() { Ok(p) } else { Err(format!("no incr_floor at '{}'", p.display())) }
}

/// [`mortar_testutil::LLVM_MAJORS`], newest first, space-separated.
fn llvm_majors() -> String {
    mortar_testutil::LLVM_MAJORS.map(|m| m.to_string()).join(" ")
}

fn lane(a: &Args, extra: &[String]) -> Result<bool, String> {
    let mortar = crate::mason::mortar_binary(a.mortar.as_deref())?;
    let python = mortar_testutil::find_tool("python3").ok_or("python3 not found")?;
    let xtask = std::env::current_exe().map_err(|e| format!("the xtask binary: {e}"))?;
    let mut cmd = Command::new(python);
    cmd.arg(crate::linktest::workspace_root().join("tests/l10/l10.py"))
        .arg("--mortar")
        .arg(&mortar)
        .arg("--xtask")
        .arg(&xtask)
        .arg("--work")
        .arg(crate::linktest::build_dir().join("l10"))
        .arg("-j")
        .arg(a.jobs.to_string())
        .args(&a.pass)
        .args(extra)
        .env("PYTHONDONTWRITEBYTECODE", "1")
        // The LLVM majors the lane's clang is found by, as find_tool finds
        // the link tests' and the census's (tests/l10/build.py `which`).
        .env("MORTAR_LLVM_MAJORS", llvm_majors());
    if a.floor && !a.pass.iter().any(|w| w == "--floor") {
        cmd.arg("--floor").arg(floor_binary()?);
    }
    // The lane's workers each hold a jobserver token for the run
    // (spec/harness.md "Jobserver"); the first is this process's own.
    let client = crate::jobs::global();
    let tokens: Vec<_> = (0..a.jobs).map(|_| client.acquire()).collect();
    let r = process::run_step(&mut cmd, false);
    drop(tokens);
    match r {
        Ok((status, _)) => Ok(status.success()),
        Err(e) => Err(format!("cannot run python3: {e}")),
    }
}

pub fn main(args: &[String]) -> ExitCode {
    let a = match Args::parse(args) {
        Ok(a) => a,
        Err(e) => {
            eprint!("xtask l10: {e}\n{USAGE}");
            return ExitCode::from(2);
        }
    };
    let required = std::env::var_os("MORTAR_REQUIRE_TOOLS").is_some_and(|v| v == "1");
    let mut extra: Vec<String> = Vec::new();
    if required {
        extra.push("--require-tools".to_owned());
    }
    let mut ok = match lane(&a, &extra) {
        Ok(ok) => ok,
        Err(e) => {
            eprintln!("xtask l10: {e}");
            return ExitCode::FAILURE;
        }
    };
    // The nightly run: the regressions and the lane's own test after the matrix.
    if a.nightly {
        for more in ["--regressions", "--self-test"] {
            let b = Args {
                pass: vec![more.to_owned()],
                mortar: a.mortar.clone(),
                jobs: a.jobs,
                floor: true,
                ..Args::default()
            };
            match lane(&b, &extra) {
                Ok(o) => ok &= o,
                Err(e) => {
                    eprintln!("xtask l10: {e}");
                    ok = false;
                }
            }
        }
    }
    if ok { ExitCode::SUCCESS } else { ExitCode::FAILURE }
}

#[cfg(test)]
mod tests {
    use super::Args;

    fn s(w: &[&str]) -> Vec<String> {
        w.iter().map(|x| (*x).to_owned()).collect()
    }

    // spec/harness.md "L10": the lane's words pass through; -j and --mortar
    // are ours; a selective run is not the nightly one.
    #[test]
    fn arguments() {
        let a = Args::parse(&s(&["--quick", "--seed", "3", "-j", "2"])).unwrap();
        assert!(a.quick && !a.nightly && !a.floor);
        assert_eq!(a.jobs, 2);
        assert_eq!(a.pass, s(&["--quick", "--seed", "3"]));
        let n = Args::parse(&[]).unwrap();
        assert!(n.nightly && n.floor && n.jobs == 4);
        let f = Args::parse(&s(&["--subject", "floor", "--seed", "1"])).unwrap();
        assert!(f.floor && !f.nightly);
        assert!(!Args::parse(&s(&["--replay", "x.json"])).unwrap().nightly);
        assert!(Args::parse(&s(&["--self-test"])).unwrap().floor);
        assert!(Args::parse(&s(&["--seed"])).is_err());
        assert!(Args::parse(&s(&["--bogus"])).is_err());
        assert!(Args::parse(&s(&["-j", "x"])).is_err());
    }
}
