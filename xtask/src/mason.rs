//! `cargo xtask mason`: the Mason lane (WP-033) — objects made by kiln (the
//! front end) and hod (the code generator), linked by mortar and run.
//! Contract: `tests/mason/README.md`; record: `spec/landed/mason/WP-033.md`.
//!
//!   cargo xtask mason [programs] [options]   hand-written programs x linkers
//!   cargo xtask mason census <group>...      what the objects contain
//!   cargo xtask mason setup                  libc++ tree, tests, kiln's runner
//!   cargo xtask mason libcxx [options] ...   libc++'s tests through the lane
//!   cargo xtask mason verify <out> <map>     plumb, for the lane's `ld` shim
//!
//! kiln and hod are not built here: their release builds are found in the
//! build root (`<root>/target-kiln/release/kiln++`,
//! `<root>/target-hod/release/hod`, the root being the parent of this
//! checkout's target directory) or named by `MASON_KILN` / `MASON_HOD`.
//! Without them every command but `census` and `verify` says so and exits
//! 0, unless `MASON_REQUIRE_TOOLS=1`.

use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::time::Duration;

use crate::process::{self, Limits, Output};

/// `println!` for the lane's commands: a reader that closes the pipe
/// (`| head`) ends the command with status 141 (as a shell's SIGPIPE
/// would) instead of a panic, which `panic = "abort"` turns into SIGABRT
/// and a core file.
macro_rules! say {
    () => { $crate::mason::emit(format_args!("\n")) };
    ($($t:tt)*) => { $crate::mason::emit(format_args!("{}\n", format_args!($($t)*))) };
}

/// `print!` likewise.
macro_rules! say_raw {
    ($($t:tt)*) => { $crate::mason::emit(format_args!($($t)*)) };
}

pub mod census;
pub mod libcxx;
pub mod outcomes;
pub mod programs;
pub mod setup;

/// Writes to standard output; a closed pipe exits with 141, another
/// error with 1 (`say!`).
pub fn emit(a: std::fmt::Arguments<'_>) {
    use std::io::Write;
    if let Err(e) = std::io::stdout().lock().write_fmt(a) {
        if e.kind() == std::io::ErrorKind::BrokenPipe {
            std::process::exit(141);
        }
        eprintln!("xtask mason: standard output: {e}");
        std::process::exit(1);
    }
}

pub const USAGE: &str = "usage: cargo xtask mason [programs] [--mortar <path>] [--filter <s>]... [--keep] [-j <n>]\n       \
     cargo xtask mason census [--group <name> <path>...]...\n       \
     cargo xtask mason setup\n       \
     cargo xtask mason libcxx [--mode <m>]... [--backend hod|llvm] [--sample <name>] [--shard K/N]\n                                \
     [--full] [--cflags <s>] [-j <n>] [--plumb] [--report <file>] [--mortar <path>] [--out <dir>]\n                                \
     [--results <file>] [--baseline <file>] [--write-baseline <file>] [<dir>...]\n       \
     cargo xtask mason libcxx --collect <dir> [--results <file>] [--baseline <file>]\n       \
     cargo xtask mason libcxx --compare <file>[:<mode>] <file>[:<mode>]\n       \
     cargo xtask mason verify <output> <map> [<line>]\n";

/// The entry point.
pub fn main(args: &[String]) -> ExitCode {
    let (cmd, rest) = match args.first().map(String::as_str) {
        Some(c @ ("programs" | "census" | "setup" | "libcxx" | "verify")) => (c, &args[1..]),
        _ => ("programs", args),
    };
    let result = match cmd {
        "census" => census::main(rest),
        "setup" => setup::main(rest),
        "libcxx" => libcxx::main(rest),
        "verify" => verify_main(rest),
        _ => programs::main(rest),
    };
    match result {
        Ok(code) => code,
        Err(message) => {
            eprint!("xtask mason: {message}\n{USAGE}");
            ExitCode::from(2)
        }
    }
}

/// The root of the build directories: the parent of this checkout's
/// target directory (`.build` of the wrapper repository).
pub fn build_root() -> PathBuf {
    let dir = crate::linktest::build_dir();
    dir.parent().map_or(dir.clone(), Path::to_path_buf)
}

/// Where the lane keeps what it materialises (the libc++ tree, the test
/// checkout, kiln's runner): `MASON_LANE_DIR`, else `<root>/mason-lane`.
pub fn lane_dir() -> PathBuf {
    std::env::var_os("MASON_LANE_DIR")
        .filter(|d| !d.is_empty())
        .map_or_else(|| build_root().join("mason-lane"), PathBuf::from)
}

/// `MASON_REQUIRE_TOOLS=1`: a missing kiln or hod build fails the command.
pub fn required() -> bool {
    std::env::var("MASON_REQUIRE_TOOLS").is_ok_and(|v| v == "1")
}

/// kiln's and hod's builds.
#[derive(Clone, Debug)]
pub struct Mason {
    /// `kiln++`.
    pub kiln: PathBuf,
    pub hod: PathBuf,
}

impl Mason {
    /// The builds, or `Err` naming what is missing.
    pub fn find() -> Result<Mason, String> {
        let root = build_root();
        let pick = |var: &str, default: PathBuf| {
            std::env::var_os(var).filter(|v| !v.is_empty()).map_or(default, PathBuf::from)
        };
        let kiln = pick("MASON_KILN", root.join("target-kiln/release/kiln++"));
        let hod = pick("MASON_HOD", root.join("target-hod/release/hod"));
        let missing: Vec<String> = [&kiln, &hod]
            .iter()
            .filter(|p| !p.is_file())
            .map(|p| p.display().to_string())
            .collect();
        if missing.is_empty() {
            Ok(Mason { kiln, hod })
        } else {
            Err(format!(
                "no kiln/hod build at {} (build kiln and hod with the wrapper's scripts/env.sh, \
                 or set MASON_KILN / MASON_HOD)",
                missing.join(", ")
            ))
        }
    }

    /// [`Mason::find`], or the skip message: `Ok(None)` to skip, `Err`
    /// under `MASON_REQUIRE_TOOLS=1`.
    pub fn find_or_skip(what: &str) -> Result<Option<Mason>, String> {
        match Mason::find() {
            Ok(m) => Ok(Some(m)),
            Err(e) if required() => Err(format!("{e} (MASON_REQUIRE_TOOLS=1)")),
            Err(e) => {
                say!("xtask mason {what}: skipped: {e}");
                Ok(None)
            }
        }
    }
}

/// mortar's release binary: `--mortar <path>`, else built in this checkout.
pub fn mortar_binary(given: Option<&Path>) -> Result<PathBuf, String> {
    if let Some(p) = given {
        return Ok(p.to_path_buf());
    }
    let cargo = std::env::var_os("CARGO").unwrap_or_else(|| "cargo".into());
    let mut command = std::process::Command::new(cargo);
    command.args(["build", "--release", "--quiet", "--package", "mortar", "--bin", "mortar"]);
    match process::run_step(&mut command, true) {
        Ok((status, _)) if status.success() => {}
        Ok((status, _)) => return Err(format!("cannot build mortar: cargo build: {status}")),
        Err(e) => return Err(format!("cannot build mortar: cannot run cargo: {e}")),
    }
    let binary = crate::linktest::build_dir().join("release").join("mortar");
    if binary.is_file() {
        Ok(binary)
    } else {
        Err(format!("cannot build mortar: no binary at '{}'", binary.display()))
    }
}

/// A tool on `PATH` (or an absolute path), `None` when absent.
pub fn tool(name: &str) -> Option<PathBuf> {
    if Path::new(name).is_absolute() {
        return Path::new(name).is_file().then(|| PathBuf::from(name));
    }
    mortar_testutil::find_tool(name)
}

/// The limits of a compile or link.
pub const TOOL_LIMITS: Limits = Limits { time: Duration::from_mins(3), memory_kib: 8 << 20 };
/// The limits of a program under test.
pub const RUN_LIMITS: Limits = Limits { time: Duration::from_secs(30), memory_kib: 2 << 20 };

/// Runs `program args` in `dir` with `env`.
pub fn run_in(
    program: &Path,
    args: &[String],
    dir: &Path,
    env: &[(&str, &str)],
    limits: Limits,
) -> Result<Output, String> {
    process::run(
        program,
        |c| {
            c.args(args).current_dir(dir);
            for (k, v) in env {
                c.env(k, v);
            }
        },
        b"",
        limits,
    )
}

/// Splits a `-###` command line: words, `"..."` with `\"` and `\\`.
pub fn words(line: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut chars = line.chars().peekable();
    while let Some(&c) = chars.peek() {
        if c.is_whitespace() {
            chars.next();
            continue;
        }
        let mut w = String::new();
        if c == '"' {
            chars.next();
            while let Some(c) = chars.next() {
                match c {
                    '\\' => {
                        if let Some(n) = chars.next() {
                            w.push(n);
                        }
                    }
                    '"' => break,
                    c => w.push(c),
                }
            }
        } else {
            while let Some(&c) = chars.peek() {
                if c.is_whitespace() {
                    break;
                }
                w.push(c);
                chars.next();
            }
        }
        out.push(w);
    }
    out
}

/// What the verifier says about one mortar link.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Plumbed {
    /// The verifier's findings.
    pub findings: usize,
    /// The first finding, rendered.
    pub first: Option<String>,
}

impl std::fmt::Display for Plumbed {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self.findings {
            0 => f.write_str("clean"),
            n => write!(f, "{n} findings"),
        }
    }
}

/// Runs the verifier on mortar's `output` and its mortar-map `map`, the
/// inputs read relative to `base`; `line`, the linker's words, lets stage 3
/// judge a dynamic output with its line (spec/plumb.md §12).
pub fn plumb(
    output: &Path,
    map: &Path,
    base: &Path,
    line: Option<Vec<String>>,
) -> Result<Plumbed, String> {
    let (_, report, _) = crate::plumb::verify_link_line(
        output,
        map,
        mortar_plumb::Format::Mortar,
        base,
        None,
        None,
        line,
    )?;
    let mut p = Plumbed::default();
    let name = output.display().to_string();
    for f in &report.findings {
        p.findings += 1;
        if p.first.is_none() {
            let one = mortar_plumb::Report { findings: vec![f.clone()], ..report.clone() };
            p.first = one.render(&name).lines().next().map(str::to_owned);
        }
    }
    Ok(p)
}

/// `verify <output> <map> [<line>]`: one line, `plumb: <result>[:
/// <first>]`; exit 0 when clean. `<line>` is a file of the linker's words,
/// one per line.
fn verify_main(args: &[String]) -> Result<ExitCode, String> {
    let (output, map, line) = match args {
        [output, map] => (output, map, None),
        [output, map, line] => {
            let text = std::fs::read_to_string(line).map_err(|e| format!("{line}: {e}"))?;
            (output, map, Some(text.lines().map(str::to_owned).collect()))
        }
        _ => return Err("verify needs <output> <map> [<line>]".into()),
    };
    let p = plumb(Path::new(output), Path::new(map), Path::new(""), line)?;
    match &p.first {
        Some(first) => say!("plumb: {p}: {first}"),
        None => say!("plumb: {p}"),
    }
    Ok(if p.findings == 0 { ExitCode::SUCCESS } else { ExitCode::FAILURE })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn words_split_driver_lines() {
        assert_eq!(
            words(r#" "/usr/bin/ld" "-o" "a b" -static "x\"y""#),
            ["/usr/bin/ld", "-o", "a b", "-static", "x\"y"]
        );
    }

    #[test]
    fn plumbed_renders() {
        assert_eq!(Plumbed::default().to_string(), "clean");
        assert_eq!(Plumbed { findings: 2, first: None }.to_string(), "2 findings");
    }
}
