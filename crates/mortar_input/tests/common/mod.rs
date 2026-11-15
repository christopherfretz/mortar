//! Helpers of the `mortar_input` tests.
#![allow(dead_code, reason = "each test file uses a part")]

use std::ffi::{OsStr, OsString};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::Duration;

use mortar_core::par::Pool;
use mortar_core::plan::Plan;
use mortar_input::store::{Hooks, Store, StoreConfig, StorePolicy};
use mortar_input::walk::{Loaded, Options, WalkEnv, walk};
pub use mortar_testutil::target::Arch;

/// The plan of a command line (no program name).
pub fn plan<S: AsRef<OsStr>>(args: &[S]) -> Plan {
    let args: Vec<OsString> = args.iter().map(|a| a.as_ref().to_owned()).collect();
    mortar_cli::parse(&args).plan
}

/// The stores every test runs under (`spec/input.md` §11.1, §16): both,
/// unless `MORTAR_TEST_STORE` names one (`resident` or `staged`).
pub fn policies() -> Vec<StorePolicy> {
    match std::env::var("MORTAR_TEST_STORE").as_deref() {
        Ok("resident") => vec![StorePolicy::Resident],
        Ok("staged") => vec![StorePolicy::Staged],
        _ => vec![StorePolicy::Resident, StorePolicy::Staged],
    }
}

/// A store that lives as long as the test.
pub fn leaked(policy: StorePolicy) -> &'static Store {
    Box::leak(Box::new(Store::new(policy)))
}

/// Runs the walk in `cwd` with `threads` threads under every store of
/// [`policies`], asserts that they agree (§11.1: the tables are a function
/// of the files, not of the store), and returns the last run (the staged
/// store's, the default).
pub fn run_plan(cwd: &Path, plan: &Plan, options: Options, pool: &Pool) -> Loaded<'static> {
    let mut runs = policies().into_iter().map(|policy| {
        let store = leaked(policy);
        (policy, walk(plan, &WalkEnv { cwd: cwd.to_path_buf() }, store, pool, options))
    });
    let (first_policy, first) = runs.next().expect("a store");
    let print = fingerprint(&first);
    let mut last = first;
    for (policy, out) in runs {
        let other = fingerprint(&out);
        assert!(
            print == other,
            "the {first_policy:?} and {policy:?} stores disagree:\n{}",
            first_difference(&print, &other)
        );
        last = out;
    }
    last
}

/// What the walk produced, as text, without what may differ between the
/// stores by design: content hashes (the staged store computes them only
/// when asked, §11.6), the shape of a reader's parts view, the timings.
pub fn fingerprint(out: &Loaded<'_>) -> String {
    let text = format!(
        "{:?}\n{:?}\n{:?}\n{:?}\n{:?}\n{}",
        out.target, out.files, out.archives, out.link, out.problems, out.waves
    );
    mask(&text)
}

/// [`fingerprint`]'s masking, for any `Debug` text of tables.
pub fn mask(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    loop {
        let view = rest.find("View(base ");
        let hash = rest.find("ContentHash(");
        let at = match (view, hash) {
            (Some(v), Some(h)) => v.min(h),
            (Some(v), None) => v,
            (None, Some(h)) => h,
            (None, None) => break,
        };
        out.push_str(&rest[..at]);
        let open = at + rest[at..].find('(').expect("an open parenthesis");
        let close = open + rest[open..].find(')').expect("a close parenthesis");
        out.push_str(&rest[at..open]);
        out.push_str("(..)");
        rest = &rest[close + 1..];
    }
    out.push_str(rest);
    out
}

/// The first line where two texts differ, with its neighbours.
pub fn first_difference(a: &str, b: &str) -> String {
    let short = |s: &str| s.chars().take(400).collect::<String>();
    let (la, lb): (Vec<&str>, Vec<&str>) = (a.split(", ").collect(), b.split(", ").collect());
    match la.iter().zip(&lb).position(|(x, y)| x != y) {
        Some(i) => format!(
            "at item {i}:\n  {}\n  {}",
            short(&la[i.saturating_sub(2)..=i].join(", ")),
            short(&lb[i.saturating_sub(2)..=i].join(", "))
        ),
        None => format!("lengths {} and {}", la.len(), lb.len()),
    }
}

/// [`run`] with the content hashes filled (§11.6) by every store of
/// [`policies`], which must agree on them too.
pub fn run_hashed<S: AsRef<OsStr>>(cwd: &Path, args: &[S]) -> Loaded<'static> {
    let plan = plan(args);
    let mut last: Option<Loaded<'static>> = None;
    for policy in policies() {
        let config = StoreConfig { hashes: Some(true), ..StoreConfig::default() };
        let store: &'static Store =
            Box::leak(Box::new(Store::with_config(policy, config, Hooks::default())));
        let out = walk(
            &plan,
            &WalkEnv { cwd: cwd.to_path_buf() },
            store,
            &Pool::sequential(),
            Options::default(),
        );
        if let Some(prev) = &last {
            let hashes = |o: &Loaded<'_>| o.files.iter().map(|f| f.hash).collect::<Vec<_>>();
            assert_eq!(hashes(prev), hashes(&out), "{policy:?}: the hashes of the stores differ");
            assert_eq!(fingerprint(prev), fingerprint(&out));
        }
        last = Some(out);
    }
    last.expect("a store")
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

/// The architecture of a target name: `native` (AArch64, the subject of
/// these tests on any host), `x86`, `i686`.
pub fn arch(target: &str) -> Arch {
    match target {
        "x86" => Arch::X86_64,
        "i686" => Arch::I386,
        _ => Arch::Aarch64,
    }
}

/// GNU tool `tool` of a target ([`arch`]), or `None` with a note (a
/// failure under `MORTAR_REQUIRE_TOOLS`).
pub fn gnu(target: &str, tool: &str) -> Option<PathBuf> {
    arch(target).tool(tool)
}

/// A compiler for a target: `native` (AArch64), `x86`, `i686`.
pub fn compiler(target: &str) -> Option<PathBuf> {
    gnu(target, "gcc")
}

/// Where the compiler of `target` finds file `name`
/// (`-print-file-name`): the native layout on its own host, the cross
/// system root's elsewhere. `None` when the compiler is missing.
///
/// # Panics
/// Panics when the compiler does not find the file.
pub fn print_file_name(target: &str, name: &str) -> Option<PathBuf> {
    let gcc = compiler(target)?;
    let o = tool_run(&gcc, &[format!("-print-file-name={name}").into()], Path::new("/"));
    assert!(o.success(), "{}", o.stderr_text());
    let file = PathBuf::from(o.stdout_text().trim());
    assert!(file.is_absolute(), "{target}: {name} not found by {}", gcc.display());
    Some(file)
}

/// The directory of the C library of `target` (that of `libc.so`), its
/// `..` resolved. `None` when the compiler is missing.
pub fn libc_dir(target: &str) -> Option<PathBuf> {
    let libc = print_file_name(target, "libc.so")?;
    Some(libc.parent().unwrap().canonicalize().unwrap())
}

/// The path of `libc.so.6` of `target` as its `libc.so` script names it.
/// `None` when the compiler is missing.
pub fn libc_so_6(target: &str) -> Option<String> {
    let script = std::fs::read_to_string(print_file_name(target, "libc.so")?).unwrap();
    let path = script.split_whitespace().find(|w| w.ends_with("/libc.so.6"));
    Some(
        path.unwrap_or_else(|| panic!("{target}: libc.so names no libc.so.6:\n{script}"))
            .to_owned(),
    )
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

/// `ar rc` (or `rcT`, thin) of `members` (paths relative to `dir`), with
/// the AArch64 `ar` (the archive index needs the members' format).
pub fn ar(dir: &Path, name: &str, members: &[&str], thin: bool) -> Option<PathBuf> {
    ar_for(dir, "native", name, members, thin)
}

/// [`ar`] with the `ar` of `target`.
pub fn ar_for(
    dir: &Path,
    target: &str,
    name: &str,
    members: &[&str],
    thin: bool,
) -> Option<PathBuf> {
    let ar = gnu(target, "ar")?;
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
    targets.iter().all(|t| compiler(t).is_some() && gnu(t, "ar").is_some())
}
