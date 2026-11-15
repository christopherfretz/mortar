//! Helpers of the `mortar_input` tests.
#![allow(dead_code, reason = "each test file uses a part")]

use std::ffi::{OsStr, OsString};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::Duration;

use mortar_core::par::Pool;
use mortar_core::plan::Plan;
use mortar_input::store::{Store, StorePolicy};
use mortar_input::walk::{Loaded, Options, WalkEnv, walk};

/// The plan of a command line (no program name).
pub fn plan<S: AsRef<OsStr>>(args: &[S]) -> Plan {
    let args: Vec<OsString> = args.iter().map(|a| a.as_ref().to_owned()).collect();
    mortar_cli::parse(&args).plan
}

/// Runs the walk in `cwd` with `threads` threads; the store lives as long
/// as the test.
pub fn run_plan(cwd: &Path, plan: &Plan, options: Options, pool: &Pool) -> Loaded<'static> {
    let store: &'static Store = Box::leak(Box::new(Store::new(StorePolicy::Resident)));
    walk(plan, &WalkEnv { cwd: cwd.to_path_buf() }, store, pool, options)
}

/// [`run_plan`] on a command line, sequentially.
pub fn run<S: AsRef<OsStr>>(cwd: &Path, args: &[S]) -> Loaded<'static> {
    run_plan(cwd, &plan(args), Options::default(), &Pool::sequential())
}

/// [`run`] with the closure.
pub fn run_closure<S: AsRef<OsStr>>(cwd: &Path, args: &[S]) -> Loaded<'static> {
    run_plan(cwd, &plan(args), Options { closure: true }, &Pool::sequential())
}

/// Each problem rendered.
pub fn report(out: &Loaded<'_>) -> Vec<String> {
    out.problems.iter().map(mortar_input::problem::Problem::render).collect()
}

/// The problem ids.
pub fn ids(out: &Loaded<'_>) -> Vec<&'static str> {
    out.problems.iter().map(mortar_input::problem::Problem::id).collect()
}

/// The `opened` paths.
pub fn opened(out: &Loaded<'_>) -> Vec<String> {
    out.link.opened.iter().map(|o| String::from_utf8_lossy(&o.path).into_owned()).collect()
}

/// The display names of the rows.
pub fn names(out: &Loaded<'_>) -> Vec<String> {
    out.files
        .iter()
        .map(|f| match f.name.member() {
            Some(m) => format!("{}({m})", f.name.path()),
            None => f.name.path().to_owned(),
        })
        .collect()
}

/// Runs a tool, bounded.
pub fn tool_run(program: &Path, args: &[OsString], cwd: &Path) -> mortar_testutil::Outcome {
    let mut c = Command::new(program);
    c.args(args).current_dir(cwd);
    mortar_testutil::run_with(&mut c, b"", Duration::from_mins(2))
}

/// A tool, or `None` with a note (a failure under `MORTAR_REQUIRE_TOOLS`).
pub fn need(name: &str) -> Option<PathBuf> {
    let t = mortar_testutil::tool(name);
    if t.is_none() {
        assert!(!mortar_testutil::tools_required(), "{name} is required (MORTAR_REQUIRE_TOOLS)");
        eprintln!("note: {name} not found; skipped");
    }
    t
}

/// The linker command line a compiler driver runs (`-###`): the
/// arguments after the linker's path.
pub fn driver_link_args(driver: &Path, args: &[&str], cwd: &Path) -> Option<Vec<OsString>> {
    let mut a: Vec<OsString> = vec!["-###".into()];
    a.extend(args.iter().map(OsString::from));
    let out = tool_run(driver, &a, cwd);
    let text = out.stderr_text();
    let line = text.lines().rev().find(|l| {
        let first = shell_words(l).into_iter().next().unwrap_or_default();
        let name = first.rsplit('/').next().unwrap_or_default();
        first.starts_with('/')
            && (name == "collect2"
                || name == "ld"
                || name.ends_with("-ld")
                || name.ends_with("ld.bfd"))
    })?;
    let mut words = shell_words(line);
    if words.is_empty() {
        return None;
    }
    words.remove(0);
    Some(words.into_iter().map(OsString::from).collect())
}

/// The words of a `-###` line: separated by blanks, double quotes and
/// backslash escapes.
fn shell_words(line: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut w = String::new();
    let mut in_word = false;
    let mut quoted = false;
    let mut chars = line.chars();
    while let Some(c) = chars.next() {
        match c {
            '"' => {
                quoted = !quoted;
                in_word = true;
            }
            '\\' => {
                if let Some(n) = chars.next() {
                    w.push(n);
                }
                in_word = true;
            }
            c if c.is_whitespace() && !quoted => {
                if in_word {
                    out.push(std::mem::take(&mut w));
                    in_word = false;
                }
            }
            c => {
                w.push(c);
                in_word = true;
            }
        }
    }
    if in_word {
        out.push(w);
    }
    out
}

/// A compiler for a target: `native` (AArch64), `x86`, `i686`.
pub fn compiler(target: &str) -> Option<PathBuf> {
    match target {
        "x86" => need("x86_64-linux-gnu-gcc"),
        "i686" => need("i686-linux-gnu-gcc"),
        _ => need("gcc"),
    }
}

/// Compiles `src` (C) into `dir/name` with `flags`; `None` when the
/// compiler is missing. Compiles only (`-c`) unless `flags` has
/// `-shared` or the pseudo-flag `--link` (a linked executable).
pub fn cc(dir: &Path, target: &str, name: &str, src: &str, flags: &[&str]) -> Option<PathBuf> {
    let gcc = compiler(target)?;
    let c = dir.join(format!("{name}.src.c"));
    std::fs::write(&c, src).unwrap();
    let mut a: Vec<OsString> =
        flags.iter().filter(|f| **f != "--link").map(OsString::from).collect();
    a.extend(["-o".into(), dir.join(name).into_os_string(), c.into_os_string()]);
    if !flags.iter().any(|f| *f == "-shared" || *f == "--link") {
        a.insert(0, "-c".into());
    }
    let o = tool_run(&gcc, &a, dir);
    assert!(o.success(), "{}: {}", name, o.stderr_text());
    Some(dir.join(name))
}

/// `ar rc` (or `rcT`, thin) of `members` (paths relative to `dir`).
pub fn ar(dir: &Path, name: &str, members: &[&str], thin: bool) -> Option<PathBuf> {
    let ar = need("ar")?;
    let mut a: Vec<OsString> = vec![if thin { "rcT" } else { "rc" }.into(), name.into()];
    a.extend(members.iter().map(OsString::from));
    let o = tool_run(&ar, &a, dir);
    assert!(o.success(), "ar {name}: {}", o.stderr_text());
    Some(dir.join(name))
}

/// The machine-name flag of a target.
pub fn emulation(target: &str) -> &'static str {
    match target {
        "x86" => "-melf_x86_64",
        "i686" => "-melf_i386",
        _ => "-maarch64linux",
    }
}

/// `true` when every named compiler and `ar` exist.
pub fn have(targets: &[&str]) -> bool {
    targets.iter().all(|t| compiler(t).is_some()) && need("ar").is_some()
}
