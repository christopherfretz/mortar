//! `cargo xtask linktest`: the link tests (`spec/harness.md` "Link tests",
//! `docs/VALIDATION.md` layer L2).
//!
//! Each test file under `tests/link/` builds its inputs, links them with
//! every linker of the run and checks what came out. The reference linkers
//! (GNU ld, lld, mold) run a test first; mortar is judged by it only when
//! they establish it (the self-check).
//!
//! - [`format`]: the test file;
//! - [`pattern`]: the patterns of its expectations;
//! - [`target`]: the architectures, the linkers and their tools;
//! - [`engine`]: one test on one architecture;
//! - [`outcome`]: what a result means, and the self-check;
//! - [`report`]: the summary, the results file and the exit status.
//!
//! This module parses the command line, finds the tools, builds mortar,
//! and runs the units of the matrix on worker threads.

pub mod engine;
pub mod format;
pub mod odr;
pub mod outcome;
pub mod pattern;
pub mod report;
pub mod spelled;
pub mod target;

use std::fmt::Write as _;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::Mutex;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Instant;

use engine::{Matrix, Unit};
use outcome::Outcome;
use report::{Record, Report};
use target::{Arch, ArchTools, CommonTools, Linker};

pub const USAGE: &str = "usage: cargo xtask linktest [--linker bfd,lld,mold,mortar] \
                         [--arch aarch64,x86_64,i386] [--filter <text>]... [-j <n>] \
                         [--mortar <binary>] [--results <file>] [--keep] [--no-plumb] [-v] \
                         [--producer gcc|hod-objcopy|hod|mixed|kiln|kiln-hod] [<file or directory>...]\n";

/// The most worker threads a run starts by default.
const DEFAULT_JOBS: usize = 8;

/// What `cargo xtask linktest` is asked for.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
#[allow(clippy::struct_excessive_bools)] // independent switches of a command line
pub struct Args {
    /// The linkers to run, in [`Linker::ALL`] order; every one when empty.
    pub linkers: Vec<Linker>,
    /// The architectures, in [`Arch::ALL`] order; every one when empty.
    pub arches: Vec<Arch>,
    /// A test runs when its name contains one of these, or when there is
    /// none.
    pub filters: Vec<String>,
    /// Worker threads.
    pub jobs: Option<usize>,
    /// mortar's binary; otherwise the workspace's is built.
    pub mortar: Option<PathBuf>,
    /// Where the results file goes.
    pub results: Option<PathBuf>,
    /// Keep the work directory.
    pub keep: bool,
    /// List the passes too.
    pub verbose: bool,
    /// Do not run the verifier on the links' outputs (L3).
    pub no_plumb: bool,
    /// Test files and directories; `tests/link` when empty.
    pub paths: Vec<PathBuf>,
    /// The digest mode of `cargo xtask static-identical` (not an option of
    /// `linktest`: [`engine::Matrix::digest`]).
    pub digest: bool,
    /// Who makes the objects of C and C++ compile steps and `meta` steps
    /// (WP-099, [`engine::Producer`]).
    pub producer: engine::Producer,
    /// The digest mode's option variants ([`engine::Matrix::variants`];
    /// not an option of `linktest`).
    pub variants: Vec<Vec<String>>,
    /// Tests that require one of these are left out of the run (not an
    /// option of `linktest`: `cargo xtask mason producers` leaves out the
    /// tests with kiln steps, which compare producers themselves).
    pub without: Vec<target::Feature>,
}

impl Args {
    /// Parses the arguments after `linktest`.
    ///
    /// # Errors
    /// An unknown option, linker or architecture, an option without its
    /// value, a number of jobs that is not from 1 to 64.
    pub fn parse(args: &[String]) -> Result<Self, String> {
        let mut parsed = Self::default();
        let mut rest = args.iter();
        while let Some(arg) = rest.next() {
            let (option, inline) = match arg.split_once('=') {
                Some((option, value)) if option.starts_with("--") => (option, Some(value)),
                _ => (arg.as_str(), None),
            };
            let mut value = |name: &str| -> Result<String, String> {
                match inline {
                    Some(value) => Ok(value.to_owned()),
                    None => rest.next().cloned().ok_or_else(|| format!("{name} needs a value")),
                }
            };
            match option {
                "--linker" => {
                    for word in list(&value("--linker")?) {
                        let linker = Linker::parse(word).ok_or_else(|| {
                            format!("unknown linker `{word}` (bfd, lld, mold, mortar)")
                        })?;
                        parsed.linkers.push(linker);
                    }
                }
                "--arch" => {
                    for word in list(&value("--arch")?) {
                        let arch = Arch::parse(word).ok_or_else(|| {
                            format!("unknown architecture `{word}` (aarch64, x86_64, i386)")
                        })?;
                        parsed.arches.push(arch);
                    }
                }
                "--filter" => parsed.filters.push(value("--filter")?),
                "-j" | "--jobs" => parsed.jobs = Some(jobs(&value("-j")?)?),
                "--mortar" => parsed.mortar = Some(PathBuf::from(value("--mortar")?)),
                "--results" => parsed.results = Some(PathBuf::from(value("--results")?)),
                "--producer" => {
                    let word = value("--producer")?;
                    parsed.producer = engine::Producer::parse(&word).ok_or_else(|| {
                        format!("unknown producer `{word}` (gcc, hod-objcopy, hod, mixed, kiln, kiln-hod)")
                    })?;
                }
                "--keep" if inline.is_none() => parsed.keep = true,
                "--no-plumb" if inline.is_none() => parsed.no_plumb = true,
                "-v" | "--verbose" if inline.is_none() => parsed.verbose = true,
                other => match other.strip_prefix("-j").filter(|n| !n.is_empty()) {
                    Some(number) => parsed.jobs = Some(jobs(number)?),
                    None if other.starts_with('-') => {
                        return Err(format!("unknown argument `{arg}`"));
                    }
                    None => parsed.paths.push(PathBuf::from(arg)),
                },
            }
        }
        parsed.linkers.sort_unstable();
        parsed.linkers.dedup();
        parsed.arches.sort_unstable();
        parsed.arches.dedup();
        Ok(parsed)
    }
}

/// The words of a comma-separated list.
fn list(value: &str) -> impl Iterator<Item = &str> {
    value.split(',').map(str::trim)
}

fn jobs(value: &str) -> Result<usize, String> {
    value
        .parse()
        .ok()
        .filter(|n| (1..=64).contains(n))
        .ok_or_else(|| format!("`-j {value}` is not a number of jobs from 1 to 64"))
}

/// The workspace's root directory.
pub fn workspace_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .map_or_else(|| PathBuf::from("."), Path::to_path_buf)
}

/// The build directory: `CARGO_TARGET_DIR`, else the one this executable
/// was built in, else `target` in the workspace.
pub fn build_dir() -> PathBuf {
    if let Some(dir) = std::env::var_os("CARGO_TARGET_DIR") {
        return PathBuf::from(dir);
    }
    let from_exe = std::env::current_exe().ok().and_then(|exe| {
        let mut dir = exe.parent()?;
        if dir.file_name()? == "deps" {
            dir = dir.parent()?;
        }
        let build = dir.parent()?;
        build.join("CACHEDIR.TAG").is_file().then(|| build.to_path_buf())
    });
    from_exe.unwrap_or_else(|| workspace_root().join("target"))
}

/// `cargo xtask linktest <args>`: runs, prints the summary on standard
/// output, and returns the exit status (`spec/harness.md` "Link tests",
/// "Exit status").
pub fn main(args: &[String]) -> u8 {
    let args = match Args::parse(args) {
        Ok(args) => args,
        Err(message) => {
            eprint!("xtask linktest: {message}\n{USAGE}");
            return 2;
        }
    };
    match run(&args) {
        Ok(report) => {
            print!("{}", report.summary(args.verbose));
            report.exit_status()
        }
        Err(message) => {
            eprint!("xtask linktest: {message}\n{USAGE}");
            2
        }
    }
}

/// A test file selected for the run.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TestFile {
    /// Its path relative to `tests/link`, or as it was named.
    pub name: String,
    pub path: PathBuf,
}

/// The test files named by `paths` (every `*.test` under a directory),
/// in name order, those whose name contains a filter.
///
/// # Errors
/// A path that does not exist.
pub fn collect(paths: &[PathBuf], filters: &[String]) -> Result<Vec<TestFile>, String> {
    let base = workspace_root().join("tests").join("link");
    let paths = if paths.is_empty() { vec![base.clone()] } else { paths.to_vec() };
    let canonical_base = base.canonicalize().unwrap_or(base);
    let tests_dir = workspace_root().join("tests");
    let canonical_tests = tests_dir.canonicalize().unwrap_or(tests_dir);
    let mut files = Vec::new();
    for path in paths {
        if path.is_dir() {
            walk(&path, &mut files)
                .map_err(|e| format!("cannot read '{}': {e}", path.display()))?;
        } else if path.is_file() {
            files.push(path);
        } else {
            return Err(format!("no such test file or directory: '{}'", path.display()));
        }
    }
    let mut tests: Vec<TestFile> = files
        .into_iter()
        .map(|path| {
            let name = path
                .canonicalize()
                .ok()
                .and_then(|full| {
                    let name = full
                        .strip_prefix(&canonical_base)
                        .or_else(|_| full.strip_prefix(&canonical_tests))
                        .ok()?;
                    Some(name.to_path_buf())
                })
                .unwrap_or_else(|| path.clone());
            TestFile { name: name.display().to_string(), path }
        })
        .filter(|test| filters.is_empty() || filters.iter().any(|f| test.name.contains(f.as_str())))
        .collect();
    tests.sort_by(|a, b| a.name.cmp(&b.name));
    tests.dedup_by(|a, b| a.name == b.name);
    Ok(tests)
}

/// Every `*.test` file under `dir`.
fn walk(dir: &Path, files: &mut Vec<PathBuf>) -> std::io::Result<()> {
    for entry in std::fs::read_dir(dir)? {
        let path = entry?.path();
        if path.is_dir() {
            walk(&path, files)?;
        } else if path.extension().is_some_and(|ext| ext.eq_ignore_ascii_case("test")) {
            files.push(path);
        }
    }
    Ok(())
}

/// Builds the workspace's mortar binary and returns its path.
pub fn build_mortar() -> Result<PathBuf, String> {
    let cargo = std::env::var_os("CARGO").unwrap_or_else(|| "cargo".into());
    let mut command = Command::new(cargo);
    command.args(["build", "--quiet", "--package", "mortar", "--bin", "mortar"]);
    match crate::process::run_step(&mut command, true) {
        Ok((status, _)) if status.success() => {}
        Ok((status, _)) => return Err(format!("cannot build mortar: cargo build: {status}")),
        Err(e) => return Err(format!("cannot build mortar: cannot run cargo: {e}")),
    }
    let binary = build_dir().join("debug").join("mortar");
    if binary.is_file() {
        Ok(binary)
    } else {
        Err(format!("cannot build mortar: no binary at '{}'", binary.display()))
    }
}

/// The tools of the run: the architectures whose tools are there, the
/// tools every architecture shares, the linkers of the run and mortar's
/// binary (built unless `--mortar` names one). What is missing is a note,
/// or an error under `MORTAR_REQUIRE_TOOLS=1`.
fn find_tools(
    args: &Args,
    host: Option<Arch>,
    any_test: bool,
    report: &mut Report,
) -> (Vec<ArchTools>, CommonTools, Vec<Linker>, Option<PathBuf>) {
    let required = mortar_testutil::tools_required();
    let absent = |report: &mut Report, text: String| {
        if required { report.missing.push(text) } else { report.notes.push(text) }
    };
    let find = mortar_testutil::find_listed_tool;
    let mut wanted_arches =
        if args.arches.is_empty() { Arch::ALL.to_vec() } else { args.arches.clone() };
    // hod and kiln target AArch64 and x86-64 only (WP-099).
    if args.producer != engine::Producer::Gcc && wanted_arches.contains(&Arch::I386) {
        wanted_arches.retain(|&arch| arch != Arch::I386);
        report.notes.push(format!(
            "--producer {}: i386 is not run (kiln and hod do not target it)",
            args.producer.name()
        ));
    }
    let mut arches: Vec<ArchTools> = Vec::new();
    for arch in wanted_arches {
        match ArchTools::find(arch, host, find) {
            Ok(tools) => arches.push(tools),
            Err(missing) => {
                absent(report, format!("{} is not run: {} not found", arch.name(), missing.what));
            }
        }
    }
    let common = CommonTools::find(find);
    let wanted_linkers =
        if args.linkers.is_empty() { Linker::ALL.to_vec() } else { args.linkers.clone() };
    let mut linkers = Vec::new();
    let mut mortar = None;
    for linker in wanted_linkers {
        let found = match linker {
            Linker::Bfd => {
                for tools in arches.iter().filter(|tools| tools.bfd.is_none()) {
                    let name = tools.arch.gnu_tool(host, "ld.bfd");
                    absent(
                        report,
                        format!("bfd is not run on {}: {name} not found", tools.arch.name()),
                    );
                }
                true
            }
            Linker::Lld => common.lld.is_some(),
            Linker::Mold => common.mold.is_some(),
            Linker::Mortar => {
                let binary = match &args.mortar {
                    Some(path) if path.is_file() => Ok(path.clone()),
                    Some(path) => Err(format!("no mortar binary at '{}'", path.display())),
                    None if !any_test || arches.is_empty() => Err(String::new()),
                    None => build_mortar(),
                };
                match binary {
                    Ok(binary) => mortar = Some(binary),
                    Err(e) if e.is_empty() => {}
                    Err(e) => report.errors.push(e),
                }
                mortar.is_some()
            }
        };
        if found {
            linkers.push(linker);
        } else if linker != Linker::Mortar {
            let tool = if linker == Linker::Lld { "ld.lld of a pinned LLVM major" } else { "mold" };
            absent(report, format!("{} is not run: {tool} not found", linker.name()));
        }
    }
    if linkers.contains(&Linker::Mortar)
        && !linkers.iter().any(|linker| linker.is_reference())
        && !args.digest
    {
        report.notes.push(
            "no reference linker in the run: mortar is judged by the tests' expectations, which \
             the self-check has not proven"
                .to_owned(),
        );
    }

    (arches, common, linkers, mortar)
}

/// The run-wide default anomalies' file, in `tests/link` (`spec/harness.md`
/// "L3").
pub const DEFAULTS: &str = "anomalies";

/// `tests/link/anomalies` of the workspace.
fn defaults_file() -> PathBuf {
    workspace_root().join("tests").join("link").join(DEFAULTS)
}

/// The run-wide mortar xfails' file, in `tests/link` (`spec/harness.md`
/// "Markers").
pub const XFAILS: &str = "xfails";

/// The run-wide mortar xfails of the file at `path`; none when there is no
/// file.
///
/// # Errors
/// A file that cannot be read or does not parse.
pub fn load_xfails(path: &Path) -> Result<Vec<format::ArchXfail>, String> {
    let text = match std::fs::read_to_string(path) {
        Ok(text) => text,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(e) => return Err(format!("{}: {e}", path.display())),
    };
    format::parse_xfails(&text).map_err(|errors| {
        let lines: Vec<String> = errors
            .iter()
            .map(|e| format!("{}:{}: {}", path.display(), e.line, e.message))
            .collect();
        lines.join("; ")
    })
}

/// A default no output of the run has is stale; a run of part of the
/// corpus may well not meet it, so only a run of all of it says so
/// (`spec/harness.md` "L3", "The defaults").
fn check_defaults(
    args: &Args,
    plumb: &engine::Plumb,
    defaults: &[format::Anomaly],
    host: Option<Arch>,
    report: &mut Report,
) {
    if args.digest {
        return;
    }
    if args.paths.is_empty() && args.filters.is_empty() {
        report.errors.extend(plumb.stale_defaults(defaults, host));
    } else if !defaults.is_empty() && plumb.verified > 0 {
        report.notes.push(
            "L3: a run of part of the corpus does not check that the default anomalies occur"
                .to_owned(),
        );
    }
}

/// The directory of the shared fixtures, `tests/link/fixtures`
/// (`spec/harness.md` "Fixtures").
pub const FIXTURES: &str = "fixtures";

/// The fixture `name`, `tests/link/fixtures/<name>.fixture`, parsed; or
/// why it cannot be.
fn load_fixture(name: &str) -> Result<format::Fixture, String> {
    let path =
        workspace_root().join("tests").join("link").join(FIXTURES).join(format!("{name}.fixture"));
    let text = std::fs::read_to_string(&path).map_err(|e| format!("{}: {e}", path.display()))?;
    format::parse_fixture(&text).map_err(|errors| {
        let lines: Vec<String> = errors
            .iter()
            .map(|e| format!("{}:{}: {}", path.display(), e.line, e.message))
            .collect();
        lines.join("; ")
    })
}

/// The test xfails of `tests/link/xfails` against the tests: a test that
/// also marks mortar `xfail` itself on such an architecture is an error
/// of the test (two markers for one linker); a line naming no test of the
/// corpus is stale, which only a run of all of it can say (as for the
/// default anomalies).
fn check_xfails(
    args: &Args,
    xfails: &[format::ArchXfail],
    tests: &[TestFile],
    parsed: &[(usize, &TestFile, format::Test)],
    report: &mut Report,
) {
    for (_, file, test) in parsed {
        let twice = xfails.iter().find(|x| {
            x.test.as_deref() == Some(file.name.as_str())
                && test
                    .marker(x.arch, Linker::Mortar)
                    .is_some_and(|m| m.kind == format::MarkerKind::Xfail)
        });
        if let Some(x) = twice {
            report.errors.push(format!(
                "tests/link/xfails line {}: {} marks mortar xfail on {} itself as well",
                x.line,
                file.name,
                x.arch.name()
            ));
        }
    }
    if args.paths.is_empty() && args.filters.is_empty() {
        let mut stale: Vec<&format::ArchXfail> = xfails
            .iter()
            .filter(|x| x.test.as_ref().is_some_and(|t| !tests.iter().any(|f| &f.name == t)))
            .collect();
        stale.dedup_by_key(|x| x.line);
        for x in stale {
            report.errors.push(format!(
                "tests/link/xfails line {}: there is no test {}: remove the line",
                x.line,
                x.test.as_deref().unwrap_or_default()
            ));
        }
    }
}

/// The run-wide default anomalies of the file at `path`; none when there
/// is no file.
///
/// # Errors
/// A file that cannot be read or does not parse.
pub fn load_defaults(path: &Path) -> Result<Vec<format::Anomaly>, String> {
    let text = match std::fs::read_to_string(path) {
        Ok(text) => text,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(e) => return Err(format!("{}: {e}", path.display())),
    };
    format::parse_defaults(&text).map_err(|errors| {
        let lines: Vec<String> = errors
            .iter()
            .map(|e| format!("{}:{}: {}", path.display(), e.line, e.message))
            .collect();
        lines.join("; ")
    })
}

/// Loads the fixtures the parsed tests name, each once.
fn load_fixtures(matrix: &mut Matrix, parsed: &[(usize, &TestFile, format::Test)]) {
    for (_, _, test) in parsed {
        for name in &test.fixtures {
            if !matrix.fixtures.contains_key(name) {
                matrix.fixtures.insert(name.clone(), load_fixture(name));
            }
        }
    }
}

/// The note of a run whose verifier has no stage 3.
fn stage3_note() -> String {
    format!(
        "L3: the verifier has no stage 3 (spec/plumb.md §1): dynamic outputs are not judged \
         ('{}')",
        engine::STAGE3_ABSENT
    )
}

/// Parses the test files: a file that does not parse gives one `ERROR`
/// record of the file.
fn parse_tests<'a>(
    tests: &'a [TestFile],
    report: &mut Report,
) -> Vec<(usize, &'a TestFile, format::Test)> {
    let mut parsed = Vec::new();
    for (index, test) in tests.iter().enumerate() {
        let text = std::fs::read_to_string(&test.path);
        match text.map_err(|e| e.to_string()).and_then(|text| {
            format::parse(&text).map_err(|errors| {
                errors.iter().map(ToString::to_string).collect::<Vec<_>>().join("; ")
            })
        }) {
            Ok(parsed_test) => parsed.push((index, test, parsed_test)),
            Err(why) => report.records.push(Record {
                test: test.name.clone(),
                arch: None,
                linker: None,
                outcome: Outcome::Error,
                detail: why,
                spec: String::new(),
            }),
        }
    }
    parsed
}

/// Runs the tests `args` selects.
///
/// # Errors
/// A test path that does not exist (a usage error).
#[allow(clippy::too_many_lines)] // the tools, the matrix, the units, then the report
pub fn run(args: &Args) -> Result<Report, String> {
    let started = Instant::now();
    let tests = collect(&args.paths, &args.filters)?;
    let mut report = Report { tests: tests.len(), ..Report::default() };
    let host = Arch::host();
    let (arches, common, linkers, mortar) = find_tools(args, host, !tests.is_empty(), &mut report);

    let root = build_dir().join("linktest");
    let work = root.join(format!("run-{}", std::process::id()));
    if work.exists() {
        std::fs::remove_dir_all(&work)
            .map_err(|e| format!("cannot remove '{}': {e}", work.display()))?;
    }
    let jobs = args.jobs.unwrap_or_else(|| {
        std::thread::available_parallelism().map_or(1, std::num::NonZero::get).min(DEFAULT_JOBS)
    });
    report.arches = arches.iter().map(|tools| tools.arch).collect();
    report.linkers.clone_from(&linkers);
    report.configuration = configuration(&report, mortar.as_deref(), jobs);
    let defaults =
        if args.no_plumb || args.digest { Ok(Vec::new()) } else { load_defaults(&defaults_file()) };
    let defaults = defaults.unwrap_or_else(|e| {
        report.errors.push(e);
        Vec::new()
    });
    let xfails = load_xfails(&workspace_root().join("tests").join("link").join(XFAILS))
        .unwrap_or_else(|e| {
            report.errors.push(e);
            Vec::new()
        });
    let wrappers = work.join("bin");
    let stage3 = !args.no_plumb && engine::stage3_present();
    let mut matrix = Matrix {
        host,
        common,
        linkers,
        mortar,
        wrappers,
        plumb: !args.no_plumb && !args.digest,
        defaults,
        xfails,
        digest: args.digest,
        stage3,
        fixtures: std::collections::BTreeMap::new(),
        fixture_dir: work.join("fixtures"),
        built: engine::FixtureCache::default(),
        producer: args.producer,
        mason: None,
        kiln: None,
        variants: args.variants.clone(),
    };
    if args.producer != engine::Producer::Gcc {
        match crate::mason::Mason::find() {
            Ok(mason) => matrix.mason = Some(mason),
            Err(e) => report.errors.push(format!("--producer {}: {e}", args.producer.name())),
        }
    }
    if let Err(e) = matrix.write_wrappers(&arches) {
        report.errors.push(e);
    }

    let parsed = parse_tests(&tests, &mut report);
    let parsed: Vec<_> = parsed
        .into_iter()
        .filter(|(_, _, test)| !args.without.iter().any(|f| test.requires.contains(f)))
        .collect();
    load_fixtures(&mut matrix, &parsed);
    // WP-134: a kiln that writes records, probed once, when a test asks
    // for one or the producer is kiln.
    if args.producer.is_kiln()
        || parsed.iter().any(|(_, _, test)| test.requires.contains(&target::Feature::Kiln))
    {
        let found = crate::mason::KilnRecords::find(&work.join("kiln-probe"));
        if let (true, Err(e)) = (args.producer.is_kiln(), &found) {
            report.errors.push(format!("--producer {}: {e}", args.producer.name()));
        }
        matrix.kiln = Some(found);
    }
    check_xfails(args, &matrix.xfails, &tests, &parsed, &mut report);
    if matrix.plumb && !matrix.stage3 {
        report.notes.push(stage3_note());
    }
    let matrix = &matrix;
    let units: Vec<Unit> = if report.errors.is_empty() {
        parsed
            .iter()
            .flat_map(|(index, file, test)| {
                let dir = work.join(format!("{index:04}-{}", file.name.replace(['/', ' '], "_")));
                arches.iter().filter(|tools| test.arches.contains(&tools.arch)).map(move |tools| {
                    Unit { matrix, tools, test, name: &file.name, dir: dir.join(tools.arch.name()) }
                })
            })
            .collect()
    } else {
        Vec::new()
    };
    let mut plumb = engine::Plumb::default();
    for results in run_units(&units, jobs) {
        plumb.add(&results.plumb);
        if results.optional_skip {
            report.optional_skips.extend(
                results.records.iter().take(1).filter_map(|r| Some((r.test.clone(), r.arch?))),
            );
        }
        report.records.extend(results.records);
        report.digests.extend(results.digests);
        report.missing.extend(results.missing.into_iter().map(|tool| format!("{tool} not found")));
    }
    report.missing.sort();
    report.missing.dedup();
    report.digests.sort();
    if plumb != engine::Plumb::default() {
        report.notes.push(plumb.note());
    }
    check_defaults(args, &plumb, &matrix.defaults, matrix.host, &mut report);
    // Deterministic: by test, architecture, linker, and a record without a
    // linker after those with one.
    report.records.sort_by_key(|record| {
        (record.test.clone(), record.arch, record.linker.map_or(usize::MAX, |l| l as usize))
    });
    let results = args.results.clone().unwrap_or_else(|| root.join("results.tsv"));
    match report.write_results(&results) {
        Ok(()) => report.results_file = Some(results),
        Err(e) => report.errors.push(e),
    }
    if args.keep {
        report.kept = Some(work);
    } else if work.exists()
        && let Err(e) = std::fs::remove_dir_all(&work)
    {
        report.notes.push(format!("cannot remove '{}': {e}", work.display()));
    }
    report.elapsed = started.elapsed();
    Ok(report)
}

/// Runs `units` on `jobs` worker threads; each takes the next unit when
/// it is done with one. The order of the results is the order they ended.
fn run_units(units: &[Unit], jobs: usize) -> Vec<engine::Results> {
    let next = AtomicUsize::new(0);
    let done = Mutex::new(Vec::new());
    std::thread::scope(|scope| {
        for _ in 0..jobs.min(units.len()) {
            scope.spawn(|| {
                loop {
                    let index = next.fetch_add(1, Ordering::Relaxed);
                    let Some(unit) = units.get(index) else { break };
                    let results = unit.run();
                    done.lock().unwrap_or_else(std::sync::PoisonError::into_inner).push(results);
                }
            });
        }
    });
    done.into_inner().unwrap_or_else(std::sync::PoisonError::into_inner)
}

/// The first line of the summary: what ran.
fn configuration(report: &Report, mortar: Option<&Path>, jobs: usize) -> String {
    let names =
        |names: Vec<&str>| if names.is_empty() { "none".to_owned() } else { names.join(" ") };
    let mut text = format!(
        "{} tests; architectures: {}; linkers: {}; {jobs} jobs",
        report.tests,
        names(report.arches.iter().map(|arch| arch.name()).collect()),
        names(report.linkers.iter().map(|linker| linker.name()).collect()),
    );
    if let Some(mortar) = mortar {
        let _ = write!(text, "; mortar: {}", mortar.display());
    }
    text
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(args: &[&str]) -> Result<Args, String> {
        Args::parse(&args.iter().map(|a| (*a).to_owned()).collect::<Vec<_>>())
    }

    // spec/harness.md "Link tests", "The command": the options, in both
    // spellings; lists are comma-separated and kept in matrix order.
    #[test]
    fn the_command_line() {
        assert_eq!(parse(&[]).unwrap(), Args::default());
        let args = parse(&[
            "--linker",
            "mortar,bfd",
            "--linker=lld",
            "--arch=i386,aarch64",
            "--filter",
            "resolve/",
            "--filter=tls",
            "-j3",
            "--mortar",
            "/x/mortar",
            "--results=/x/r.tsv",
            "--keep",
            "--no-plumb",
            "-v",
            "tests/link/start",
            "a.test",
        ])
        .unwrap();
        assert_eq!(args.linkers, [Linker::Bfd, Linker::Lld, Linker::Mortar]);
        assert_eq!(args.arches, [Arch::Aarch64, Arch::I386]);
        assert_eq!(args.filters, ["resolve/", "tls"]);
        assert_eq!(args.jobs, Some(3));
        assert_eq!(args.mortar.as_deref(), Some(Path::new("/x/mortar")));
        assert_eq!(args.results.as_deref(), Some(Path::new("/x/r.tsv")));
        assert!(args.keep && args.verbose && args.no_plumb);
        assert!(!parse(&[]).unwrap().no_plumb);
        assert_eq!(args.paths, [PathBuf::from("tests/link/start"), PathBuf::from("a.test")]);
        assert_eq!(parse(&["-j", "2"]).unwrap().jobs, Some(2));
        assert_eq!(parse(&["--jobs=4"]).unwrap().jobs, Some(4));
    }

    // "The command": what is not understood is a usage error that says so.
    #[test]
    fn usage_errors() {
        let error = |args: &[&str]| parse(args).unwrap_err();
        assert!(error(&["--linker", "gold"]).contains("unknown linker `gold`"));
        assert!(error(&["--arch=arm"]).contains("unknown architecture `arm`"));
        assert!(error(&["--linker"]).contains("needs a value"));
        assert!(error(&["-j", "0"]).contains("from 1 to 64"));
        assert!(error(&["-jx"]).contains("from 1 to 64"));
        assert!(error(&["--fast"]).contains("unknown argument `--fast`"));
        assert!(error(&["--keep=yes"]).contains("unknown argument `--keep=yes`"));
    }

    // "Test files": a directory gives its `*.test` files, named relative
    // to `tests/link`; a filter selects by name; a missing path is an
    // error.
    #[test]
    fn collecting_test_files() {
        let scratch = mortar_testutil::Scratch::new("linktest-collect");
        std::fs::create_dir_all(scratch.join("d/e")).unwrap();
        for name in ["d/b.test", "d/e/a.test", "d/notes.txt"] {
            scratch.write(name, "");
        }
        let tests = collect(&[scratch.join("d")], &[]).unwrap();
        let names: Vec<&str> = tests.iter().map(|t| t.name.as_str()).collect();
        let outside = scratch.join("d").display().to_string();
        assert_eq!(names, [format!("{outside}/b.test"), format!("{outside}/e/a.test")]);
        let filtered = collect(&[scratch.join("d")], &["/e/a.".to_owned()]).unwrap();
        assert_eq!(filtered.len(), 1);
        assert!(collect(&[scratch.join("nothing")], &[]).unwrap_err().contains("no such test"));
        // The corpus is named relative to tests/link.
        let corpus = collect(&[], &[]).unwrap();
        assert!(
            corpus.iter().all(
                |t| !t.name.starts_with('/') && t.path.extension().is_some_and(|e| e == "test")
            )
        );
    }
}
