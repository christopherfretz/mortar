//! `cargo xtask realbuild`: real open-source projects built by their own
//! build systems with mortar as the linker (`spec/harness.md`
//! "realbuild", layer L9 of `docs/VALIDATION.md`; WP-100).
//!
//!   cargo xtask realbuild [--project <p>]... [--row gcc|clang]... [--mortar <path>]
//!                         [--work <dir>] [--keep] [--keep-going] [--update-baseline]
//!   cargo xtask realbuild --report
//!   cargo xtask realbuild verify <output> <map> <argv file>
//!
//! For each project of `tests/realbuild/manifest` and each of its rows (the
//! compilers: GCC with GNU ld as the control, Clang with lld as the
//! control), the pinned archive is fetched, then the project is built and
//! its tests run twice, each time in a fresh copy: with the control linker
//! (`control` mode) and with mortar (`mortar` mode). The linker the
//! compiler drivers find is the wrapper `tests/realbuild/ld`, which records
//! every link line; in `mortar` mode it also links the line with the
//! control to a scratch output (the oracle), links it with mortar, has the
//! verifier judge mortar's output, and falls back to the control for a
//! line mortar does not link. The outcomes of the lines and of the tests
//! are held to `tests/realbuild/baseline.tsv` and `tests.tsv`; the report
//! is `docs/status/realbuild.md`. An off-gate lane: it fetches from the
//! network and takes hours; `cargo xtask ci` runs only its unit tests.

pub mod outcome;
pub mod report;

use std::collections::BTreeMap;
use std::fmt::Write as _;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use crate::process::{self, Limits};
use outcome::{Outcome, Row};

pub const USAGE: &str = "usage: cargo xtask realbuild [--project <p>]... [--row gcc|clang]... \
                         [--mortar <path>] [--work <dir>] [--keep] [--keep-going] [--update-baseline]\n       \
                         cargo xtask realbuild --report\n       \
                         cargo xtask realbuild verify <output> <map> <argv file>\n";

/// The lane's files in the checkout.
pub const MANIFEST: &str = "tests/realbuild/manifest";
pub const BASELINE: &str = "tests/realbuild/baseline.tsv";
pub const TESTS: &str = "tests/realbuild/tests.tsv";
pub const RUNS: &str = "tests/realbuild/runs.tsv";
pub const REPORT: &str = "docs/status/realbuild.md";

/// A build or a test run of one project (make and everything it starts).
const BUILD_LIMITS: Limits = Limits { time: Duration::from_hours(2), memory_kib: 8 << 20 };
/// `du`.
const DU_LIMITS: Limits = Limits { time: Duration::from_mins(5), memory_kib: 1 << 20 };
/// The 1-minute load average under which a build starts.
const MAX_LOAD: f64 = 10.0;
/// The job count of a test runner that is not a jobserver client.
const TEST_JOBS: u32 = 4;

macro_rules! say {
    ($($t:tt)*) => { eprintln!("[realbuild] {}", format_args!($($t)*)) };
}

/// A project of the manifest.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Project {
    pub name: String,
    pub version: String,
    pub url: String,
    pub sha256: String,
    /// The directory of the archive holding the sources.
    pub root: String,
    pub licence: String,
    /// Why it is in the list: one sentence.
    pub why: String,
    /// The rows: `gcc`, `clang`.
    pub rows: Vec<String>,
    /// A second archive (a test suite shipped apart): URL, SHA-256, root.
    pub extra: Option<(String, String, String)>,
    /// What the tests are, when not all of them: one sentence.
    pub tests: String,
}

impl Project {
    /// The project's script, `tests/realbuild/projects/<name>.sh`.
    pub fn script(&self, workspace: &Path) -> PathBuf {
        workspace.join("tests/realbuild/projects").join(format!("{}.sh", self.name))
    }
}

/// The rows a project may have.
pub const ROWS: [&str; 2] = ["gcc", "clang"];

const KEYS: [&str; 9] =
    ["version", "url", "sha256", "root", "licence", "why", "rows", "extra", "tests"];

/// Reads the manifest (the bench manifest's format, `spec/harness.md`
/// "Workloads").
pub fn parse_manifest(text: &str) -> Result<Vec<Project>, String> {
    let mut out = Vec::new();
    for (at, name, entries) in crate::bench::manifest::sections(text, "project")? {
        let fail = |what: String| format!("line {at}: project `{name}`: {what}");
        for (line, key, _) in &entries {
            if !KEYS.contains(&key.as_str()) {
                return Err(format!("line {line}: project `{name}`: unknown key `{key}`"));
            }
        }
        let get = |k: &str| entries.iter().find(|(_, key, _)| key == k).map(|(_, _, v)| v.clone());
        let need =
            |k: &str| get(k).filter(|v| !v.is_empty()).ok_or_else(|| fail(format!("no `{k}`")));
        let hex = |s: &str| {
            s.len() == 64 && s.bytes().all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
        };
        let sha256 = need("sha256")?;
        if !hex(&sha256) {
            return Err(fail("`sha256` is 64 lower-case hexadecimal digits".into()));
        }
        let rows: Vec<String> = need("rows")?.split_whitespace().map(str::to_owned).collect();
        if let Some(bad) = rows.iter().find(|r| !ROWS.contains(&r.as_str())) {
            return Err(fail(format!("unknown row `{bad}` (gcc, clang)")));
        }
        let extra = match get("extra") {
            None => None,
            Some(v) => {
                let w: Vec<&str> = v.split_whitespace().collect();
                let [url, sha, root] = w.as_slice() else {
                    return Err(fail("`extra` is <url> <sha256> <root>".into()));
                };
                if !hex(sha) {
                    return Err(fail(
                        "`extra`'s SHA-256 is 64 lower-case hexadecimal digits".into(),
                    ));
                }
                Some(((*url).to_owned(), (*sha).to_owned(), (*root).to_owned()))
            }
        };
        out.push(Project {
            version: need("version")?,
            url: need("url")?,
            sha256,
            root: need("root")?,
            licence: need("licence")?,
            why: need("why")?,
            tests: get("tests").unwrap_or_default(),
            rows,
            extra,
            name,
        });
    }
    Ok(out)
}

/// The command line.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
#[allow(clippy::struct_excessive_bools, reason = "the command's switches")]
pub struct Args {
    pub projects: Vec<String>,
    pub rows: Vec<String>,
    pub mortar: Option<PathBuf>,
    pub work: Option<PathBuf>,
    pub keep: bool,
    pub keep_going: bool,
    pub update_baseline: bool,
    pub report: bool,
}

impl Args {
    pub fn parse(args: &[String]) -> Result<Self, String> {
        let mut a = Self::default();
        let mut it = args.iter();
        while let Some(arg) = it.next() {
            let (name, inline) = match arg.split_once('=') {
                Some((n, v)) if n.starts_with("--") => (n, Some(v.to_owned())),
                _ => (arg.as_str(), None),
            };
            let mut value = |what: &str| {
                inline.clone().or_else(|| it.next().cloned()).ok_or(format!("{name} needs {what}"))
            };
            match name {
                "--project" => a.projects.push(value("a project")?),
                "--row" => {
                    let r = value("a row")?;
                    if !ROWS.contains(&r.as_str()) {
                        return Err(format!("unknown row {r} (gcc, clang)"));
                    }
                    a.rows.push(r);
                }
                "--mortar" => a.mortar = Some(PathBuf::from(value("a path")?)),
                "--work" => a.work = Some(PathBuf::from(value("a directory")?)),
                "--keep" => a.keep = true,
                "--keep-going" => a.keep_going = true,
                "--update-baseline" => a.update_baseline = true,
                "--report" => a.report = true,
                other => return Err(format!("unknown argument {other}")),
            }
        }
        if a.report && (a.update_baseline || !a.projects.is_empty() || !a.rows.is_empty()) {
            return Err("--report measures nothing: it takes no other option".into());
        }
        Ok(a)
    }
}

/// `cargo xtask realbuild ...`: the exit status.
pub fn main(args: &[String]) -> u8 {
    if args.first().map(String::as_str) == Some("verify") {
        return verify_main(&args[1..]);
    }
    let a = match Args::parse(args) {
        Ok(a) => a,
        Err(e) => {
            eprint!("xtask realbuild: {e}\n{USAGE}");
            return 2;
        }
    };
    let workspace = crate::bench::workspace_root();
    let result =
        if a.report { write_report(&workspace).map(|()| true) } else { run(&a, &workspace) };
    match result {
        Ok(true) => 0,
        Ok(false) => 1,
        Err(e) => {
            eprintln!("xtask realbuild: error: {e}");
            3
        }
    }
}

/// `verify <output> <map> <argv file>`: the verifier on one of mortar's
/// outputs, with the line (NUL-terminated words; `@file` words expanded,
/// relative to the current directory, where the link ran). Prints
/// `plumb: clean`, or `plumb: <n> findings: <rule> x<count>, ...` and the
/// first finding; exit status 0 clean, 1 findings, 2 when it cannot judge.
fn verify_main(args: &[String]) -> u8 {
    let [output, map, words_file] = args else {
        eprintln!("xtask realbuild verify: <output> <map> <argv file>");
        return 2;
    };
    let words = outcome::nul_words(&std::fs::read(words_file).unwrap_or_default());
    let words = outcome::expand(&words, &|p| std::fs::read_to_string(p).ok());
    let verified = crate::plumb::verify_link_line(
        Path::new(output),
        Path::new(map),
        mortar_plumb::Format::Mortar,
        Path::new(""),
        None,
        None,
        Some(words),
    );
    let report = match verified {
        Ok((_, report, _)) => report,
        Err(e) => {
            println!("plumb: error: {e}");
            return 2;
        }
    };
    if report.findings.is_empty() {
        println!("plumb: clean");
        return 0;
    }
    let mut rules: BTreeMap<String, usize> = BTreeMap::new();
    for f in &report.findings {
        *rules.entry(f.rule.id()).or_default() += 1;
    }
    let by_rule: Vec<String> = rules.iter().map(|(r, n)| format!("{r} x{n}")).collect();
    println!("plumb: {} findings: {}", report.findings.len(), by_rule.join(", "));
    if let Some(first) = report.render(output).lines().next() {
        println!("{first}");
    }
    1
}

/// The compilers and the control linker of a row.
#[derive(Clone, Debug)]
struct RowTools {
    cc: PathBuf,
    cxx: PathBuf,
    fc: PathBuf,
    control: PathBuf,
}

fn found(name: &str) -> Result<PathBuf, String> {
    mortar_testutil::find_tool(name).ok_or_else(|| format!("`{name}` is not on PATH"))
}

fn row_tools(row: &str) -> Result<RowTools, String> {
    Ok(match row {
        "gcc" => RowTools {
            cc: found("gcc")?,
            cxx: found("g++")?,
            fc: found("gfortran")?,
            control: PathBuf::from(crate::bench::prepare::GNU_LD),
        },
        _ => RowTools {
            cc: found("clang")?,
            cxx: found("clang++")?,
            fc: found("gfortran")?,
            control: found("ld.lld")?,
        },
    })
}

/// The 1-minute load average.
fn load() -> f64 {
    std::fs::read_to_string("/proc/loadavg")
        .ok()
        .and_then(|t| t.split_whitespace().next()?.parse().ok())
        .unwrap_or(0.0)
}

/// Waits until the 1-minute load is under [`MAX_LOAD`].
fn wait_for_load() {
    let mut said = false;
    while load() >= MAX_LOAD {
        if !said {
            say!("load {:.1}: waiting until it is under {MAX_LOAD}", load());
            said = true;
        }
        std::thread::sleep(Duration::from_secs(20));
    }
}

/// `du -sk <dir>`, KiB.
fn du(dir: &Path) -> u64 {
    let Ok(du) = crate::bench::prepare::which("du") else { return 0 };
    process::run(
        &du,
        |c| {
            c.arg("-sk").arg(dir);
        },
        b"",
        DU_LIMITS,
    )
    .ok()
    .and_then(|o| String::from_utf8_lossy(&o.stdout).split_whitespace().next()?.parse().ok())
    .unwrap_or(0)
}

/// What one mode of one row did.
#[derive(Clone, Debug, Default)]
pub struct ModeRun {
    pub lines: Vec<(String, outcome::Record)>,
    /// The tests' results: name, PASS / FAIL / SKIP.
    pub tests: BTreeMap<String, String>,
    pub built: bool,
    pub build_error: String,
    pub wall: Duration,
    pub peak_kib: u64,
    pub queries: usize,
}

/// Where a run works.
struct Lane {
    workspace: PathBuf,
    work: PathBuf,
    mortar: PathBuf,
    xtask: PathBuf,
}

impl Lane {
    /// Builds and tests `p` in `mode` for `row`.
    #[allow(clippy::too_many_lines, reason = "one build and its tests, start to end")]
    fn mode(
        &self,
        p: &Project,
        row: &str,
        tools: &RowTools,
        mode: &str,
        src: &Path,
        extra: Option<&Path>,
    ) -> Result<ModeRun, String> {
        let dir = self.work.join("build").join(format!("{}-{row}-{mode}", p.name));
        if dir.exists() {
            std::fs::remove_dir_all(&dir)
                .map_err(|e| format!("cannot remove {}: {e}", dir.display()))?;
        }
        let (records, wrap, build, tmp) =
            (dir.join("records"), dir.join("wrap"), dir.join("build"), dir.join("tmp"));
        for d in [&records, &wrap, &build, &tmp] {
            std::fs::create_dir_all(d)
                .map_err(|e| format!("cannot create {}: {e}", d.display()))?;
        }
        let ld = std::fs::read(self.workspace.join("tests/realbuild/ld"))
            .map_err(|e| format!("tests/realbuild/ld: {e}"))?;
        for name in
            ["ld", "ld.bfd", "ld.lld", "ld.gold", "aarch64-linux-gnu-ld", "x86_64-linux-gnu-ld"]
        {
            write_executable(&wrap.join(name), &ld)?;
        }
        // A fresh copy of the sources for this build.
        let copy = dir.join("src");
        copy_tree(src, &copy)?;
        let results = dir.join("results");
        let log = dir.join("log");
        let path = std::env::var_os("PATH").unwrap_or_default();
        let mut dirs = vec![wrap.clone()];
        dirs.extend(std::env::split_paths(&path));
        let path = std::env::join_paths(dirs).map_err(|e| e.to_string())?;
        let run_sh = self.workspace.join("tests/realbuild/run.sh");
        let script = p.script(&self.workspace);
        let started = Instant::now();
        let mut run = ModeRun::default();
        let env = |c: &mut std::process::Command| {
            c.current_dir(&build)
                .env("LC_ALL", "C")
                .env("TMPDIR", &tmp)
                .env("COMPILER_PATH", &wrap)
                .env("PATH", &path)
                .env("RB_MODE", mode)
                .env("RB_CONTROL", &tools.control)
                .env("RB_MORTAR", &self.mortar)
                .env("RB_XTASK", &self.xtask)
                .env("RB_RECORDS", &records)
                .env("RB_SRC", &copy)
                .env("RB_BUILD", &build)
                .env("RB_ROW", row)
                .env("RB_RESULTS", &results)
                .env("RB_LOG", &log)
                .env("RB_TEST_JOBS", TEST_JOBS.to_string())
                .env("CC", &tools.cc)
                .env("CXX", &tools.cxx)
                .env("FC", &tools.fc)
                .env("F77", &tools.fc)
                .env("CFLAGS", "-g -O2")
                .env("CXXFLAGS", "-g -O2")
                .env("FFLAGS", "-g -O2")
                .env("FCFLAGS", "-g -O2");
            if let Some(e) = extra {
                c.env("RB_EXTRA", e);
            }
        };
        for step in ["build", "test"] {
            if step == "test" && !run.built {
                break;
            }
            say!("{} {row} {mode}: {step}", p.name);
            let bash = crate::bench::prepare::which("bash")?;
            let out = process::run(
                &bash,
                |c| {
                    c.arg("-c")
                        .arg("exec >> \"$RB_LOG\" 2>&1; exec bash \"$0\" \"$1\" \"$2\"")
                        .arg(&run_sh)
                        .arg(&script)
                        .arg(step);
                    env(c);
                },
                b"",
                BUILD_LIMITS,
            )?;
            run.peak_kib = run.peak_kib.max(du(&dir));
            if step == "build" {
                run.built = out.end.success();
                if !run.built {
                    let text = std::fs::read_to_string(&log).unwrap_or_default();
                    let tail: Vec<&str> = text.lines().rev().take(15).collect();
                    run.build_error = format!(
                        "{}: {}",
                        out.end,
                        tail.into_iter().rev().collect::<Vec<_>>().join("\n")
                    );
                    say!("{} {row} {mode}: the build failed ({})", p.name, out.end);
                }
            }
        }
        run.wall = started.elapsed();
        run.lines = outcome::read_records(&records, &build)?;
        // The ids are relative to the build directory; the sources' copy
        // is its sibling `src` and appears as `../src/...`.
        let copy_prefix = format!("{}/", copy.display());
        for (id, _) in &mut run.lines {
            if let Some(at) = id.find(&copy_prefix) {
                *id = format!("{}src/{}", &id[..at], &id[at + copy_prefix.len()..]);
            }
        }
        run.queries =
            std::fs::read_to_string(records.join("queries")).map_or(0, |t| t.lines().count());
        for line in std::fs::read_to_string(&results).unwrap_or_default().lines() {
            if let Some((name, word)) = line.split_once('\t') {
                run.tests.insert(name.to_owned(), word.trim().to_owned());
            }
        }
        // Evidence that outlives the build tree: the log's tail, the
        // records of every line mortar did not link.
        let keep = self.results().join(format!("{}.{row}.{mode}", p.name));
        if keep.exists() {
            std::fs::remove_dir_all(&keep)
                .map_err(|e| format!("cannot remove {}: {e}", keep.display()))?;
        }
        std::fs::create_dir_all(&keep)
            .map_err(|e| format!("cannot create {}: {e}", keep.display()))?;
        let _ = std::fs::copy(&log, keep.join("log"));
        let _ = std::fs::copy(&results, keep.join("tests"));
        let mut lines = String::new();
        for (id, r) in &run.lines {
            let o = if mode == "mortar" {
                Outcome::of(r).to_text()
            } else {
                format!("control {}", r.control.unwrap_or(-1))
            };
            let _ = writeln!(
                lines,
                "{id}\t{}\t{o}\t{}\t{}",
                r.kind().name(),
                r.cwd.display(),
                shell_line(&r.argv)
            );
            if mode == "mortar" && !matches!(Outcome::of(r), Outcome::Linked | Outcome::BothFail) {
                let _ = writeln!(
                    lines,
                    "  mortar: {}",
                    r.mortar_err.lines().take(6).collect::<Vec<_>>().join("\n  mortar: ")
                );
                if !r.plumb.is_empty() {
                    let _ =
                        writeln!(lines, "  plumb: {}", r.plumb.trim().replace('\n', "\n  plumb: "));
                }
            }
        }
        std::fs::write(keep.join("lines"), lines)
            .map_err(|e| format!("{}: {e}", keep.display()))?;
        Ok(run)
    }

    fn results(&self) -> PathBuf {
        self.work.join("results")
    }
}

/// `argv` as one shell line.
fn shell_line(argv: &[String]) -> String {
    argv.iter()
        .map(|a| {
            if !a.is_empty()
                && a.bytes().all(|b| b.is_ascii_alphanumeric() || b"_@%+=:,./-".contains(&b))
            {
                a.clone()
            } else {
                crate::bench::response::sh_quote(a)
            }
        })
        .collect::<Vec<_>>()
        .join(" ")
}

fn write_executable(path: &Path, bytes: &[u8]) -> Result<(), String> {
    use std::os::unix::fs::PermissionsExt;
    std::fs::write(path, bytes).map_err(|e| format!("cannot write {}: {e}", path.display()))?;
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o755))
        .map_err(|e| format!("cannot make {} executable: {e}", path.display()))
}

/// `cp -a <from>/. <to>`.
fn copy_tree(from: &Path, to: &Path) -> Result<(), String> {
    std::fs::create_dir_all(to).map_err(|e| format!("cannot create {}: {e}", to.display()))?;
    let cp = crate::bench::prepare::which("cp")?;
    let out = process::run(
        &cp,
        |c| {
            c.arg("-a").arg(from.join(".")).arg(to);
        },
        b"",
        DU_LIMITS,
    )?;
    if out.end.success() {
        Ok(())
    } else {
        Err(format!("cp -a {}: {}", from.display(), out.first_error_line()))
    }
}

/// The default work directory: `real-builds` beside the checkout's target
/// directory (the workspace's build directory).
fn default_work(workspace: &Path) -> PathBuf {
    match std::env::var_os("CARGO_TARGET_DIR") {
        Some(dir) => Path::new(&dir)
            .parent()
            .map_or_else(|| workspace.join("target/real-builds"), |p| p.join("real-builds")),
        None => workspace.join("target/real-builds"),
    }
}

/// The checkout's commit, for the runs file.
fn head(workspace: &Path) -> String {
    let Ok(git) = crate::bench::prepare::which("git") else { return "unknown".into() };
    process::run(
        &git,
        |c| {
            c.args(["rev-parse", "--short=12", "HEAD"]).current_dir(workspace);
        },
        b"",
        DU_LIMITS,
    )
    .ok()
    .map_or_else(|| "unknown".into(), |o| String::from_utf8_lossy(&o.stdout).trim().to_owned())
}

/// One row's result, as the report and the baseline keep it.
#[derive(Clone, Debug, Default)]
pub struct RowResult {
    pub project: String,
    pub row: String,
    /// The mortar mode's lines.
    pub rows: Vec<Row>,
    /// test name -> (control, mortar).
    pub tests: BTreeMap<String, (String, String)>,
    /// mode -> (wall seconds, peak KiB, lines, built).
    pub modes: BTreeMap<String, (u64, u64, usize, bool)>,
}

/// Runs the lane; `Ok(passed)`.
#[allow(clippy::too_many_lines, reason = "the lane, start to end")]
fn run(a: &Args, workspace: &Path) -> Result<bool, String> {
    let started = Instant::now();
    let text = std::fs::read_to_string(workspace.join(MANIFEST))
        .map_err(|e| format!("{MANIFEST}: {e}"))?;
    let projects = parse_manifest(&text)?;
    for name in &a.projects {
        if !projects.iter().any(|p| &p.name == name) {
            return Err(format!("no project `{name}` in {MANIFEST}"));
        }
    }
    let work = a.work.clone().unwrap_or_else(|| default_work(workspace));
    std::fs::create_dir_all(&work).map_err(|e| format!("cannot create {}: {e}", work.display()))?;
    let mortar = match &a.mortar {
        Some(m) => m.clone(),
        None => crate::bench::build_mortar(workspace)?,
    };
    let xtask = std::env::current_exe().map_err(|e| format!("this xtask's path: {e}"))?;
    let lane = Lane { workspace: workspace.to_path_buf(), work: work.clone(), mortar, xtask };
    std::fs::create_dir_all(lane.results()).map_err(|e| format!("{e}"))?;
    let baseline = outcome::parse_rows(
        &std::fs::read_to_string(workspace.join(BASELINE)).unwrap_or_default(),
    )?;
    let mut done: Vec<RowResult> = Vec::new();
    let mut stop = None;
    let mut peak_total = 0u64;
    'projects: for p in
        projects.iter().filter(|p| a.projects.is_empty() || a.projects.contains(&p.name))
    {
        let log = |m: &str| say!("{m}");
        let src_root =
            crate::bench::prepare::fetch_archive(&p.name, &p.url, &p.sha256, &work, &log)?;
        let src = src_root.join(&p.root);
        let extra = match &p.extra {
            Some((url, sha, root)) => Some(
                crate::bench::prepare::fetch_archive(&p.name, url, sha, &work, &log)?.join(root),
            ),
            None => None,
        };
        for row in p.rows.iter().filter(|r| a.rows.is_empty() || a.rows.contains(r)) {
            let tools = row_tools(row)?;
            let mut result =
                RowResult { project: p.name.clone(), row: row.clone(), ..RowResult::default() };
            let mut runs = BTreeMap::new();
            for mode in ["control", "mortar"] {
                wait_for_load();
                let m = lane.mode(p, row, &tools, mode, &src, extra.as_deref())?;
                peak_total = peak_total.max(du(&work));
                result.modes.insert(
                    mode.to_owned(),
                    (m.wall.as_secs(), m.peak_kib, m.lines.len(), m.built),
                );
                say!(
                    "{} {row} {mode}: {} lines, {} queries, {} tests, built {}, {} s, peak {} MiB",
                    p.name,
                    m.lines.len(),
                    m.queries,
                    m.tests.len(),
                    m.built,
                    m.wall.as_secs(),
                    m.peak_kib / 1024
                );
                if !a.keep {
                    let dir = work.join("build").join(format!("{}-{row}-{mode}", p.name));
                    std::fs::remove_dir_all(&dir)
                        .map_err(|e| format!("cannot remove {}: {e}", dir.display()))?;
                }
                runs.insert(mode, m);
            }
            let (control, mortar) = (&runs["control"], &runs["mortar"]);
            for (id, r) in &mortar.lines {
                result.rows.push(Row {
                    project: p.name.clone(),
                    row: row.clone(),
                    line: id.clone(),
                    kind: r.kind(),
                    outcome: Outcome::of(r),
                });
            }
            let names: std::collections::BTreeSet<&String> =
                control.tests.keys().chain(mortar.tests.keys()).collect();
            for n in names {
                let c = control.tests.get(n).cloned().unwrap_or_else(|| "-".into());
                let m = mortar.tests.get(n).cloned().unwrap_or_else(|| "-".into());
                result.tests.insert(n.clone(), (c, m));
            }
            let summary = report::summarise(&result);
            say!("{} {row}: {summary}", p.name);
            if control.lines.len() != mortar.lines.len() {
                say!(
                    "{} {row}: the control build linked {} lines, the mortar build {}",
                    p.name,
                    control.lines.len(),
                    mortar.lines.len()
                );
            }
            // The stop condition: a test passes with the control and not
            // with mortar while every line mortar linked was judged clean.
            let lost: Vec<&String> = result
                .tests
                .iter()
                .filter(|(_, (c, m))| c == "PASS" && m != "PASS")
                .map(|(n, _)| n)
                .collect();
            let unclean = result.rows.iter().any(|r| r.outcome.is_defect());
            if !lost.is_empty() && !unclean {
                stop = Some(format!(
                    "{} {row}: {} test(s) pass with the control and not with mortar, and the verifier accepted every output mortar wrote: {}",
                    p.name,
                    lost.len(),
                    lost.iter().take(10).map(|s| s.as_str()).collect::<Vec<_>>().join(", ")
                ));
            }
            report::write_row_result(&lane.results(), &result)?;
            done.push(result);
            if stop.is_some() && !a.keep_going {
                break 'projects;
            }
        }
        if !a.keep {
            let s = work.join("src").join(&p.sha256[..16]);
            let _ = std::fs::remove_dir_all(&s);
            if let Some((_, sha, _)) = &p.extra {
                let _ = std::fs::remove_dir_all(work.join("src").join(&sha[..16]));
            }
        }
    }
    let run_rows: Vec<Row> = done.iter().flat_map(|r| r.rows.iter().cloned()).collect();
    let cmp = outcome::compare(&baseline, &run_rows);
    say!(
        "against {BASELINE}: regressed {}, fixed {}, changed {}, new {}, lost {}",
        cmp.regressed.len(),
        cmp.fixed.len(),
        cmp.changed.len(),
        cmp.new.len(),
        cmp.lost.len()
    );
    for r in cmp.regressed.iter().take(40) {
        say!("  regressed: {r}");
    }
    for r in cmp.changed.iter().take(40) {
        say!("  changed: {r}");
    }
    let defects: Vec<&Row> = run_rows.iter().filter(|r| r.outcome.is_defect()).collect();
    for d in &defects {
        say!("  DEFECT {}/{}/{}: {}", d.project, d.row, d.line, d.outcome.to_text());
    }
    if let Some(s) = &stop {
        say!("STOP: {s}");
    }
    say!(
        "{} rows in {} s; peak disk of the work directory {} MiB",
        done.len(),
        started.elapsed().as_secs(),
        peak_total / 1024
    );
    if a.update_baseline {
        report::update_files(workspace, &done, &head(workspace))?;
        write_report(workspace)?;
        say!("wrote {BASELINE}, {TESTS}, {RUNS} and {REPORT}");
    }
    Ok(cmp.regressed.is_empty() && stop.is_none())
}

/// Regenerates `docs/status/realbuild.md` from the checked-in files.
fn write_report(workspace: &Path) -> Result<(), String> {
    let read =
        |p: &str| std::fs::read_to_string(workspace.join(p)).map_err(|e| format!("{p}: {e}"));
    let projects = parse_manifest(&read(MANIFEST)?)?;
    let rows = outcome::parse_rows(&read(BASELINE)?)?;
    let tests = report::parse_tests(&read(TESTS)?)?;
    let runs = report::parse_runs(&read(RUNS)?)?;
    let text = report::render(&projects, &rows, &tests, &runs);
    std::fs::write(workspace.join(REPORT), text).map_err(|e| format!("{REPORT}: {e}"))
}

// spec/harness.md "realbuild".
#[cfg(test)]
mod tests {
    use super::*;

    fn s(a: &[&str]) -> Vec<String> {
        a.iter().map(|w| (*w).to_owned()).collect()
    }

    #[test]
    fn arguments_parse() {
        let a = Args::parse(&s(&["--project", "zlib", "--row=gcc", "--keep"])).unwrap();
        assert_eq!(a.projects, ["zlib"]);
        assert_eq!(a.rows, ["gcc"]);
        assert!(a.keep);
        assert!(Args::parse(&s(&["--row", "icc"])).is_err());
        assert!(Args::parse(&s(&["--report", "--project", "x"])).is_err());
        assert!(Args::parse(&s(&["--frob"])).is_err());
    }

    #[test]
    fn the_manifest_parses_and_names_its_errors() {
        let ok = "[zlib]\nversion = 1.3.1\nurl = https://x/zlib.tar.gz\nsha256 = \
                  9a93b2b7dfdac77ceba5a558a580e74667dd6fede4585b91eefb60f03b72df23\nroot = zlib-1.3.1\n\
                  licence = Zlib\nwhy = C\nrows = gcc clang\n";
        let p = parse_manifest(ok).unwrap();
        assert_eq!(p[0].rows, ["gcc", "clang"]);
        assert!(
            parse_manifest(&ok.replace("rows = gcc clang", "rows = icc"))
                .unwrap_err()
                .contains("unknown row")
        );
        assert!(
            parse_manifest(&ok.replace("licence = Zlib\n", ""))
                .unwrap_err()
                .contains("no `licence`")
        );
        assert!(parse_manifest(&format!("{ok}frob = 1\n")).unwrap_err().contains("unknown key"));
    }

    #[test]
    fn the_checked_in_manifest_parses_and_each_project_has_a_script() {
        let ws = crate::bench::workspace_root();
        let projects =
            parse_manifest(&std::fs::read_to_string(ws.join(MANIFEST)).unwrap()).unwrap();
        assert!(!projects.is_empty());
        for p in &projects {
            assert!(p.script(&ws).is_file(), "{} has no script", p.name);
        }
    }
}
