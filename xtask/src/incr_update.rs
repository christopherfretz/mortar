//! `cargo xtask incr-update`: the named update scenarios, each hop judged
//! IN-79's four ways (`spec/harness.md` "incr-update", `spec/incr.md`
//! IN-75, IN-75a, IN-78, IN-79; H-1, WP-223). The lane is
//! `tests/incr-update/lane.py`; it judges each hop with
//! `tests/l10/judge.py`'s one `judge_step`. This command builds mortar, the
//! same crate with `--features testhooks` into its own target directory
//! (the subject: `MORTAR_INCR_DIRTY` gives an update's dirty ranges), holds
//! `-j` jobserver tokens for the lane's workers and runs it.
//!
//!   cargo xtask incr-update [--quick] [--only <scenario>,...] [--target a64|x64|i386,...]
//!                           [--compiler gcc|kiln,...] [--subject mortar|<name>|<path>]
//!                           [-j N] [--keep] [--self-test] [--mortar PATH]
//!
//! The work is under `<target>/incr-update`; a run's directory is removed
//! when it passes unless `--keep`.

use std::path::PathBuf;
use std::process::{Command, ExitCode};

use crate::process;

pub const USAGE: &str = "usage: cargo xtask incr-update [--quick] [--only <scenario>,...] [--target a64|x64|i386,...] \
                         [--compiler gcc|kiln,...] [--subject mortar|<name>|<path>] [-j <n>] [--keep] [--self-test] \
                         [--mortar <path>]\n";

/// The arguments that take a value, passed to the lane as they are.
const VALUED: [&str; 4] = ["--only", "--target", "--compiler", "--subject"];

/// The arguments that take none.
const FLAGS: [&str; 3] = ["--quick", "--keep", "--self-test"];

/// What the command line asks for, split into the lane's words and ours.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct Args {
    pub pass: Vec<String>,
    pub mortar: Option<PathBuf>,
    pub jobs: usize,
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
                a.pass.push(w.to_owned());
            } else if VALUED.contains(&w) || w == "--mortar" || w == "-j" {
                let v = args.get(i + 1).ok_or_else(|| format!("{w} needs a value"))?;
                i += 1;
                match w {
                    "--mortar" => a.mortar = Some(PathBuf::from(v)),
                    "-j" => a.jobs = v.parse().map_err(|_| format!("-j {v}: not a number"))?,
                    _ => {
                        a.pass.push(w.to_owned());
                        a.pass.push(v.clone());
                    }
                }
            } else {
                return Err(format!("unknown argument `{w}`"));
            }
            i += 1;
        }
        a.jobs = a.jobs.max(1);
        Ok(a)
    }
}

/// The mortar the subject runs: the `mortar` crate built with
/// `--features testhooks` (the feature is on `mortar`, not `mortar_cli`),
/// in its own target directory so the default build is untouched.
fn hooked_binary() -> Result<PathBuf, String> {
    let dir = crate::linktest::build_dir().join("testhooks");
    let cargo = std::env::var_os("CARGO").unwrap_or_else(|| "cargo".into());
    let mut command = Command::new(cargo);
    command
        .args(["build", "--quiet", "--package", "mortar", "--features", "testhooks"])
        .env("CARGO_TARGET_DIR", &dir)
        // no debug info: the binary is a test subject, never debugged here
        .env("CARGO_PROFILE_DEV_DEBUG", "0");
    match process::run_step(&mut command, true) {
        Ok((status, _)) if status.success() => {}
        Ok((status, _)) => {
            return Err(format!("cannot build mortar --features testhooks: cargo build: {status}"));
        }
        Err(e) => return Err(format!("cannot build mortar --features testhooks: {e}")),
    }
    let p = dir.join("debug").join("mortar");
    if p.is_file() { Ok(p) } else { Err(format!("no testhooks mortar at '{}'", p.display())) }
}

fn lane(a: &Args, extra: &[String]) -> Result<bool, String> {
    let mortar = crate::mason::mortar_binary(a.mortar.as_deref())?;
    let hooked = hooked_binary()?;
    let python = mortar_testutil::find_tool("python3").ok_or("python3 not found")?;
    let xtask = std::env::current_exe().map_err(|e| format!("the xtask binary: {e}"))?;
    let mut cmd = Command::new(python);
    cmd.arg(crate::linktest::workspace_root().join("tests/incr-update/lane.py"))
        .arg("--mortar")
        .arg(&mortar)
        .arg("--hooked")
        .arg(&hooked)
        .arg("--xtask")
        .arg(&xtask)
        .arg("--work")
        .arg(crate::linktest::build_dir().join("incr-update"))
        .arg("-j")
        .arg(a.jobs.to_string())
        .args(&a.pass)
        .args(extra)
        .env("PYTHONDONTWRITEBYTECODE", "1");
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

/// The gate's step (`cargo xtask ci`, after L10's smoke in `linktest`):
/// the quick subset, quick and full gate alike (spec/harness.md
/// "incr-update": the full matrix is not a gate step yet).
pub fn ci(_quick: bool) -> bool {
    main(&["--quick".to_owned()]) == ExitCode::SUCCESS
}

pub fn main(args: &[String]) -> ExitCode {
    let a = match Args::parse(args) {
        Ok(a) => a,
        Err(e) => {
            eprint!("xtask incr-update: {e}\n{USAGE}");
            return ExitCode::from(2);
        }
    };
    let mut extra: Vec<String> = Vec::new();
    if std::env::var_os("MORTAR_REQUIRE_TOOLS").is_some_and(|v| v == "1") {
        extra.push("--require-tools".to_owned());
    }
    match lane(&a, &extra) {
        Ok(true) => ExitCode::SUCCESS,
        Ok(false) => ExitCode::FAILURE,
        Err(e) => {
            eprintln!("xtask incr-update: {e}");
            ExitCode::FAILURE
        }
    }
}

#[cfg(test)]
mod tests {
    use super::Args;

    fn s(w: &[&str]) -> Vec<String> {
        w.iter().map(|x| (*x).to_owned()).collect()
    }

    // spec/harness.md "incr-update": the lane's words pass through; -j and
    // --mortar are ours.
    #[test]
    fn arguments() {
        let a = Args::parse(&s(&["--quick", "--only", "s1_section_permute", "-j", "2", "--keep"]))
            .unwrap();
        assert_eq!(a.jobs, 2);
        assert_eq!(a.pass, s(&["--quick", "--only", "s1_section_permute", "--keep"]));
        assert_eq!(Args::parse(&[]).unwrap().jobs, 4);
        let m = Args::parse(&s(&["--mortar", "/x/mortar", "--self-test"])).unwrap();
        assert_eq!(m.mortar.as_deref(), Some(std::path::Path::new("/x/mortar")));
        assert_eq!(m.pass, s(&["--self-test"]));
        assert!(Args::parse(&s(&["--target"])).is_err());
        assert!(Args::parse(&s(&["--bogus"])).is_err());
        assert!(Args::parse(&s(&["-j", "x"])).is_err());
    }
}
