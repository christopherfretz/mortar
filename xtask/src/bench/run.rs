//! Running the outputs (`spec/harness.md` "bench", "Running the
//! outputs", WP-036): a workload whose manifest section has `run` has its
//! program run once from mortar's verified output and once from GNU ld's
//! output of the same response file, untimed; a different exit status or
//! standard output fails mortar's rows.

use std::path::{Path, PathBuf};
use std::time::Duration;

use super::measure::LINK_LIMITS;
use super::prepare::{GNU_LD, Prepared};
use crate::process::{self, End, Limits, Output};

/// The limits of one run of a linked program.
pub const RUN_LIMITS: Limits = Limits { time: Duration::from_mins(1), memory_kib: 4 << 20 };

/// What running mortar's output against GNU ld's said.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Ran {
    /// The same exit status and standard output: the status, the bytes.
    Same { end: End, stdout: usize },
    /// They differ: how.
    Differs(String),
    /// Not run, for the reason given.
    NotRun(String),
}

impl Ran {
    /// The value of the results header's `run.<workload>`.
    pub fn text(&self) -> String {
        match self {
            Ran::Same { end, stdout } => format!("same: {end}, {stdout} bytes of standard output"),
            Ran::Differs(how) => format!("differs: {how}"),
            Ran::NotRun(why) => format!("not run: {why}"),
        }
    }

    /// The detail of mortar's rows when its program runs differently.
    pub fn row_detail(&self) -> Option<String> {
        let Ran::Differs(how) = self else { return None };
        Some(format!("mortar's output runs differently from GNU ld's: {how}"))
    }
}

/// Compares two runs of the program: mortar's and GNU ld's.
pub fn compare(mortar: &Output, gnu: &Output) -> Ran {
    if let End::Killed(_) = gnu.end {
        return Ran::NotRun(format!("GNU ld's output was {}", gnu.end));
    }
    if mortar.end != gnu.end {
        let said = mortar.first_error_line();
        let said = if said.is_empty() { String::new() } else { format!(" ({said})") };
        return Ran::Differs(format!("{}{said} where GNU ld's ended with {}", mortar.end, gnu.end));
    }
    if mortar.stdout != gnu.stdout || mortar.truncated != gnu.truncated {
        let at = mortar.stdout.iter().zip(&gnu.stdout).take_while(|(a, b)| a == b).count();
        return Ran::Differs(format!(
            "standard output differs from byte {at} ({} bytes, GNU ld's {})",
            mortar.stdout.len(),
            gnu.stdout.len()
        ));
    }
    Ran::Same { end: mortar.end, stdout: mortar.stdout.len() }
}

/// The directory `$S` of a `run` line stands for: the unpacked archive's
/// root (`cmake`), the bench's sources (`sources`), else the workload's
/// own directory.
pub fn source_dir(prepared: &Prepared, work: &Path, workspace: &Path) -> PathBuf {
    use super::manifest::Kind;
    match &prepared.workload.kind {
        Kind::Cmake(c) => work.join("src").join(&c.sha256[..16]).join(&c.root),
        Kind::Sources { .. } => workspace.join(super::manifest::SOURCES),
        _ => prepared.dir.clone(),
    }
}

/// Runs mortar's output `out` of `prepared` and GNU ld's (linked here,
/// untimed, to `<scratch>/<workload>-bfd-run.out`) with the workload's
/// `run` arguments, each in an empty directory of its own under
/// `scratch`, without input, and compares them.
///
/// # Errors
/// A child cannot be started, or GNU ld does not link the workload.
pub fn run(prepared: &Prepared, out: &Path, sources: &Path, scratch: &Path) -> Result<Ran, String> {
    let name = &prepared.workload.name;
    let Some(args) = prepared.workload.run_args(&sources.to_string_lossy()) else {
        return Ok(Ran::NotRun("no `run` in the manifest".to_owned()));
    };
    let gnu = scratch.join(format!("{name}-bfd-run.out"));
    let _ = std::fs::remove_file(&gnu);
    let linked = process::run(
        Path::new(GNU_LD),
        |c| {
            c.arg(format!("@{}", prepared.response.display())).arg("-o").arg(&gnu);
            c.current_dir(&prepared.cwd).env("LC_ALL", "C");
        },
        b"",
        LINK_LIMITS,
    )?;
    if !linked.end.success() {
        let _ = std::fs::remove_file(&gnu);
        return Err(format!(
            "{name}: GNU ld does not link the workload for its run: {}: {}",
            linked.end,
            linked.first_error_line()
        ));
    }
    let once = |program: &Path, who: &str| -> Result<Output, String> {
        let dir = scratch.join(format!("{name}-{who}-run"));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir)
            .map_err(|e| format!("cannot create {}: {e}", dir.display()))?;
        let output = process::run(
            program,
            |c| {
                c.args(&args).current_dir(&dir).env("LC_ALL", "C");
            },
            b"",
            RUN_LIMITS,
        );
        let _ = std::fs::remove_dir_all(&dir);
        output
    };
    let result = once(out, "mortar").and_then(|m| Ok((m, once(&gnu, "bfd")?)));
    let _ = std::fs::remove_file(&gnu);
    let (m, g) = result?;
    Ok(compare(&m, &g))
}

// spec/harness.md "bench", "Running the outputs".
#[cfg(test)]
mod tests {
    use super::*;

    fn output(end: End, stdout: &str, stderr: &str) -> Output {
        Output {
            end,
            stdout: stdout.as_bytes().to_vec(),
            stderr: stderr.as_bytes().to_vec(),
            truncated: false,
            wall: Duration::ZERO,
            usage: None,
        }
    }

    // "Running the outputs": the same status and standard output is the
    // same run; anything else differs, and says how.
    #[test]
    fn runs_are_compared() {
        let gnu = output(End::Exit(0), "hello\n", "");
        let same = compare(&output(End::Exit(0), "hello\n", "noise"), &gnu);
        assert_eq!(same, Ran::Same { end: End::Exit(0), stdout: 6 });
        assert_eq!(same.text(), "same: exit status 0, 6 bytes of standard output");
        assert_eq!(same.row_detail(), None);
        let status = compare(&output(End::Signal(11), "", "boom\n"), &gnu);
        assert_eq!(
            status,
            Ran::Differs("signal 11 (boom) where GNU ld's ended with exit status 0".to_owned())
        );
        let text = compare(&output(End::Exit(0), "help\n", ""), &gnu);
        assert_eq!(
            text,
            Ran::Differs("standard output differs from byte 3 (5 bytes, GNU ld's 6)".to_owned())
        );
        assert_eq!(
            text.row_detail().unwrap(),
            "mortar's output runs differently from GNU ld's: standard output differs from byte 3 \
             (5 bytes, GNU ld's 6)"
        );
        // A reference program killed by a limit says nothing about mortar's.
        let killed = output(End::Killed(process::Limit::Time), "", "");
        assert!(matches!(compare(&gnu, &killed), Ran::NotRun(_)));
    }
}
