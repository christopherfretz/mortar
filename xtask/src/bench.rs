//! `cargo xtask bench`: how fast the linkers link the workloads and how
//! much memory they take; mortar judged against this machine's baseline.
//! Contract: `spec/harness.md` "bench".
//!
//! The parts: [`manifest`] lists the workloads; [`prepare`] builds one
//! from pinned sources and captures its link ([`response`], [`synth`]);
//! [`measure`] runs the linkers; [`record`] is the results and baseline
//! file; [`stats`] is the gate; [`report`] is what a person reads.

pub mod manifest;
pub mod measure;
pub mod prepare;
pub mod record;
pub mod report;
pub mod response;
pub mod run;
pub mod stats;
pub mod synth;
pub mod verify;

use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use manifest::{CALIBRATION, Kind, Workload};
use measure::{LINKERS, Linker};
use prepare::{Env, Lock, Prepared, which};
use record::{Link, Machine, Results, Status};
use stats::{Check, Thresholds, calibration_differs};

use crate::process::{self, GNU_TIME, Limits, Probe};

pub const USAGE: &str = "\
usage: cargo xtask bench [options]

Links the workloads of bench/workloads/manifest with GNU ld, lld, mold and
mortar, and prints the median wall time and peak memory of each. Prepares
(builds and captures) a workload first when its cache is not current.

actions (one at most; without one: measure and print):
  --check              measure, then judge mortar against this machine's
                       baseline
  --update-baseline    measure every workload with every linker, then record
                       the run as this machine's baseline and regenerate
                       docs/status/bench.md; needs --reason
  --report             regenerate docs/status/bench.md and docs/status/perf.md
                       from the baseline
  --judge <file>       judge a saved run (an earlier results.tsv) against the
                       baseline as --check does, measuring nothing
  --prepare            prepare the workloads and print their census
  --list               list the workloads of the manifest

options:
  --workload <name>    only this workload (repeatable)
  --linker <name>      only this linker: bfd, lld, mold, mortar (repeatable)
  --runs <N>           measured runs per link after the warm-up (default 5,
                       1 to 100)
  --reason <text>      why the baseline changes (kept in it and its log)
  --baseline <file>    the baseline (default: bench/baselines/<machine id>.tsv)
  --mortar <binary>    the mortar to measure (default: the checkout's, built
                       with the release profile)
  --work <dir>         the cache (default: <build directory>/bench-work)
  --no-verify          do not have the verifier judge mortar's outputs (for
                       timing-only loops; no output hash is recorded; not
                       with --update-baseline)
  -h, --help           this text

Before a workload's measured runs, mortar links it once more, untimed,
with --mortar-map, and the verifier (mortar_plumb) judges that output.

exit status: 0; 1 when the verifier finds a mortar output wrong, or when
--check finds a mortar link slower, larger or lost against the baseline; 3 when the run cannot be trusted (a build or a
reference linker failed, no baseline, a slower time on an unsteady
machine); 2 for a usage error.
";

/// Measured runs per link, by default and at most.
pub const DEFAULT_RUNS: u32 = 5;
pub const MAX_RUNS: u32 = 100;
/// Runs of the calibration link before and after the measurements.
pub const CALIBRATION_RUNS: u32 = 5;

/// What the command does.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Action {
    Measure,
    Check,
    UpdateBaseline,
    Report,
    Prepare,
    List,
    /// `--judge <file>`: a saved run against the baseline.
    Judge,
}

/// The command line.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Options {
    pub action: Action,
    pub workloads: Vec<String>,
    pub linkers: Vec<String>,
    pub runs: u32,
    pub reason: Option<String>,
    pub baseline: Option<PathBuf>,
    pub mortar: Option<PathBuf>,
    pub work: Option<PathBuf>,
    /// The run `--judge` reads.
    pub judge: Option<PathBuf>,
    /// The verifier on mortar's outputs (`--no-verify` turns it off).
    pub verify: bool,
    pub help: bool,
}

impl Options {
    /// Reads the arguments after `bench`. An error is a usage error.
    pub fn parse(args: &[String]) -> Result<Self, String> {
        let mut options = Self {
            action: Action::Measure,
            workloads: Vec::new(),
            linkers: Vec::new(),
            runs: DEFAULT_RUNS,
            reason: None,
            baseline: None,
            mortar: None,
            work: None,
            judge: None,
            verify: true,
            help: false,
        };
        let mut actions = Vec::new();
        let mut args = args.iter();
        while let Some(arg) = args.next() {
            let (flag, joined) = match arg.split_once('=') {
                Some((flag, value)) if flag.starts_with("--") => (flag, Some(value.to_owned())),
                _ => (arg.as_str(), None),
            };
            let mut value = || {
                joined
                    .clone()
                    .or_else(|| args.next().cloned())
                    .ok_or_else(|| format!("`{flag}` needs a value"))
            };
            match flag {
                "--check" => actions.push(Action::Check),
                "--update-baseline" => actions.push(Action::UpdateBaseline),
                "--report" => actions.push(Action::Report),
                "--prepare" => actions.push(Action::Prepare),
                "--list" => actions.push(Action::List),
                "--judge" => {
                    options.judge = Some(value()?.into());
                    actions.push(Action::Judge);
                }
                "--workload" => options.workloads.push(value()?),
                "--linker" => {
                    let name = value()?;
                    if !LINKERS.contains(&name.as_str()) {
                        return Err(format!(
                            "unknown linker `{name}` (one of {})",
                            LINKERS.join(", ")
                        ));
                    }
                    options.linkers.push(name);
                }
                "--runs" => {
                    let text = value()?;
                    options.runs =
                        text.parse().ok().filter(|n| (1..=MAX_RUNS).contains(n)).ok_or_else(
                            || format!("`--runs` is a number from 1 to {MAX_RUNS}, not `{text}`"),
                        )?;
                }
                "--reason" => options.reason = Some(value()?).filter(|r| !r.trim().is_empty()),
                "--baseline" => options.baseline = Some(value()?.into()),
                "--mortar" => options.mortar = Some(value()?.into()),
                "--work" => options.work = Some(value()?.into()),
                "--no-verify" => options.verify = false,
                "-h" | "--help" => options.help = true,
                _ => return Err(format!("unknown argument `{arg}`")),
            }
            if joined.is_some()
                && matches!(
                    flag,
                    "--check"
                        | "--update-baseline"
                        | "--report"
                        | "--prepare"
                        | "--list"
                        | "--no-verify"
                )
            {
                return Err(format!("`{flag}` takes no value"));
            }
        }
        match actions.as_slice() {
            [] => {}
            [one] => options.action = *one,
            _ => return Err("one action at most".into()),
        }
        if options.action == Action::UpdateBaseline {
            if options.reason.is_none() {
                return Err("`--update-baseline` needs `--reason <text>`".into());
            }
            if !options.workloads.is_empty() || !options.linkers.is_empty() {
                return Err("`--update-baseline` measures every workload with every linker: no `--workload` or `--linker`".into());
            }
            if !options.verify {
                return Err(
                    "`--update-baseline` records only verified outputs: no `--no-verify`".into()
                );
            }
        } else if options.reason.is_some() {
            return Err("`--reason` goes with `--update-baseline`".into());
        }
        Ok(options)
    }
}

/// The checkout this xtask was built from.
pub fn workspace_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .map_or_else(|| PathBuf::from("."), Path::to_path_buf)
}

/// The cargo target directory of this checkout.
fn target_dir(workspace: &Path) -> PathBuf {
    std::env::var_os("CARGO_TARGET_DIR").map_or_else(|| workspace.join("target"), PathBuf::from)
}

/// `<build directory>/bench-work`: beside the target directory when it is
/// named by `CARGO_TARGET_DIR` (the build directory of the workspace,
/// shared by its checkouts), else inside the checkout's `target`.
pub fn default_work(workspace: &Path) -> PathBuf {
    match std::env::var_os("CARGO_TARGET_DIR") {
        Some(dir) => Path::new(&dir)
            .parent()
            .map_or_else(|| PathBuf::from(&dir), Path::to_path_buf)
            .join("bench-work"),
        None => workspace.join("target").join("bench-work"),
    }
}

/// The baseline of this machine, in the checkout.
pub fn baseline_path(workspace: &Path, machine: &Machine) -> PathBuf {
    workspace.join("bench/baselines").join(format!("{}.tsv", machine.id()))
}

fn say(line: &str) {
    eprintln!("[bench] {line}");
}

/// Runs the command; returns the exit status.
pub fn run(options: &Options) -> u8 {
    if options.help {
        print!("{USAGE}");
        return 0;
    }
    match run_inner(options) {
        Ok(code) => code,
        Err(e) => {
            eprintln!("xtask bench: error: {e}");
            3
        }
    }
}

fn run_inner(options: &Options) -> Result<u8, String> {
    let workspace = workspace_root();
    let manifest_path = workspace.join(manifest::PATH);
    let text = std::fs::read_to_string(&manifest_path)
        .map_err(|e| format!("cannot read {}: {e}", manifest_path.display()))?;
    let all = manifest::parse(&text).map_err(|e| format!("{}: {e}", manifest_path.display()))?;
    for name in &options.workloads {
        match all.iter().find(|w| &w.name == name) {
            None => return Err(format!("no workload `{name}` in {}", manifest::PATH)),
            Some(w) if !w.measured() => {
                return Err(format!("workload `{name}` is {}: it is not measured", w.kind.name()));
            }
            Some(_) => {}
        }
    }
    let machine = Machine::detect();
    let baseline = options.baseline.clone().unwrap_or_else(|| baseline_path(&workspace, &machine));
    let shown = baseline.strip_prefix(&workspace).unwrap_or(&baseline).display().to_string();
    match options.action {
        Action::List => {
            for w in &all {
                let what = match &w.kind {
                    Kind::Planned { needs } => format!("planned, needs {needs}"),
                    kind => kind.name().to_owned(),
                };
                println!("{:<28} {:<12} {:<10} {}", w.name, w.class, what, w.about);
            }
            return Ok(0);
        }
        Action::Report => {
            let results = Results::read(&baseline)?;
            write_status_page(&workspace, &results, &shown, &all)?;
            return Ok(0);
        }
        Action::Judge => {
            let path = options.judge.clone().unwrap_or_default();
            let results = Results::read(&path)?;
            return check(&results, &baseline, &shown, false);
        }
        _ => {}
    }

    let selected: Vec<&Workload> = all
        .iter()
        .filter(|w| {
            w.measured()
                && (options.workloads.contains(&w.name)
                    || (options.workloads.is_empty() && w.class != manifest::ON_REQUEST))
        })
        .collect();
    let calibration = all
        .iter()
        .find(|w| w.class == CALIBRATION)
        .ok_or_else(|| format!("{} has no workload of class `{CALIBRATION}`", manifest::PATH))?;
    let env = Env {
        workspace: workspace.clone(),
        work: options.work.clone().unwrap_or_else(|| default_work(&workspace)),
    };
    let _lock = Lock::take(&env.work)?;
    let scratch = env.scratch();
    std::fs::create_dir_all(&scratch)
        .map_err(|e| format!("cannot create {}: {e}", scratch.display()))?;
    say(&format!("cache: {}", env.work.display()));

    let log = |line: &str| say(line);
    let mut prepared = Vec::new();
    for w in &selected {
        prepared.push(prepare::prepare(w, &env, &log)?);
    }
    if options.action == Action::Prepare {
        for p in &prepared {
            let i = &p.info;
            println!(
                "{:<28} {} objects, {} archives, {} shared, {} other, {} bytes in, {} symbols, {} bytes out, {}",
                i.name,
                i.objects,
                i.archives,
                i.shared,
                i.others,
                i.input_bytes,
                i.symbols,
                i.output_bytes,
                i.input_hash
            );
        }
        return Ok(0);
    }
    let control = prepare::prepare(calibration, &env, &log)?;
    let results = measure_all(options, &env, &machine, &prepared, &control)?;
    conclude(options, &results, &baseline, &shown, &all, &env)
}

/// Finds the linkers, calibrates, measures every prepared workload,
/// calibrates again; the results with their header.
fn measure_all(
    options: &Options,
    env: &Env,
    machine: &Machine,
    prepared: &[Prepared],
    control: &Prepared,
) -> Result<Results, String> {
    let workspace = &env.workspace;
    let scratch = env.scratch();
    let wanted =
        |name: &str| options.linkers.is_empty() || options.linkers.iter().any(|l| l == name);
    let mut linkers = Vec::new();
    for name in LINKERS {
        if wanted(name) {
            linkers.push(find_linker(name, workspace, options.mortar.as_deref())?);
        }
    }
    // The control is a reference linker whether or not it is measured.
    let control_linker = match linkers.iter().find(|l| l.name == "lld") {
        Some(lld) => lld.clone(),
        None => find_linker("lld", workspace, None)?,
    };
    let probe = Probe::detect(Path::new(GNU_TIME), &scratch);
    if probe.time.is_none() {
        say("GNU time is not usable: wall time only, no memory figures");
    }

    let load_before = load_average();
    say(&format!(
        "calibration: {} on {} ({CALIBRATION_RUNS} runs)",
        control_linker.name, control.workload.name
    ));
    let before = measure::calibrate(control, &control_linker, CALIBRATION_RUNS, &probe, &scratch)?;
    let mut links = Vec::new();
    let mut outputs = Vec::new();
    let mut verdicts = Vec::new();
    let mut runs = Vec::new();
    let mortar = linkers.iter().find(|l| l.judged());
    if !options.verify {
        say("--no-verify: mortar's outputs are NOT verified; no output hash is recorded");
    }
    for p in prepared {
        let (verdict, ran) = verify_first(p, mortar.filter(|_| options.verify), env)?;
        say(&format!("measuring {} ({} runs per link)", p.workload.name, options.runs));
        let (mut measured, output_hash) =
            measure::measure(p, &linkers, options.runs, &probe, &scratch)?;
        let golden = match &verdict {
            Some(v) => verify::apply(v, ran.as_ref(), &mut measured, output_hash.as_deref()),
            None => None,
        };
        links.extend(measured);
        if let Some(h) = golden {
            outputs.push((p.workload.name.clone(), h));
        }
        if let Some(v) = verdict {
            verdicts.push((p.workload.name.clone(), v.text()));
        }
        if let Some(r) = ran {
            runs.push((p.workload.name.clone(), r.text()));
        }
    }
    let after = measure::calibrate(control, &control_linker, CALIBRATION_RUNS, &probe, &scratch)?;
    let noisy = calibration_differs(before, after, Thresholds::default().calibration_permille);

    let mut results = Results::default();
    let now = SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |d| d.as_secs());
    results.set("date", record::utc_date(now));
    results.set("machine", machine.id());
    results.set("cpu", machine.cpu.clone());
    results.set("cores", machine.cores.to_string());
    results.set("commit", commit(workspace));
    results.set("runs", options.runs.to_string());
    results.set("probe", probe.describe());
    let limits = measure::LINK_LIMITS;
    results.set(
        "limits",
        format!("{} s, {} KiB of address space", limits.time.as_secs(), limits.memory_kib),
    );
    results.set(
        "calibration",
        format!("{} default on {}", control_linker.name, control.info.input_hash),
    );
    results.set("calibration_before_us", before.to_string());
    results.set("calibration_after_us", after.to_string());
    results.set("noisy", if noisy { "yes" } else { "no" });
    results.set("load_before", load_before);
    results.set("load_after", load_average());
    for l in &linkers {
        results.set(&format!("version.{}", l.name), l.version.clone());
    }
    for (workload, h) in outputs {
        results.set(&format!("output_hash.{workload}"), h);
    }
    if mortar.is_some() {
        results.set("verify", if options.verify { "on" } else { "off" });
    }
    for (workload, v) in verdicts {
        results.set(&format!("verify.{workload}"), v);
    }
    for (workload, r) in runs {
        results.set(&format!("run.{workload}"), r);
    }
    if options.action == Action::UpdateBaseline {
        results.set("reason", options.reason.clone().unwrap_or_default());
    }
    results.workloads = prepared.iter().map(|p| p.info.clone()).collect();
    results.links = links;
    Ok(results)
}

/// The verifier on mortar's output of `p`, untimed, and its program run
/// against GNU ld's (spec/harness.md "Verification", "Running the
/// outputs"); nothing without `mortar`.
fn verify_first(
    p: &Prepared,
    mortar: Option<&Linker>,
    env: &Env,
) -> Result<(Option<verify::Verdict>, Option<run::Ran>), String> {
    let Some(m) = mortar else { return Ok((None, None)) };
    say(&format!("verifying mortar's output of {} (untimed)", p.workload.name));
    let sources = run::source_dir(p, &env.work, &env.workspace);
    let (v, ran) = verify::verify(p, m, &env.scratch(), &sources)?;
    say(&format!("{}: verifier: {}", p.workload.name, v.text()));
    if let Some(r) = &ran {
        say(&format!("{}: run against GNU ld's: {}", p.workload.name, r.text()));
    }
    Ok((Some(v), ran))
}

/// The golden output check against the baseline: per workload whose
/// inputs are unchanged, mortar's output hash, if it changed (WP-032a: a
/// performance commit must not change it; a feature commit may, and says
/// so in its reason when it updates the baseline). Information, not a
/// verdict: the check's exit status does not depend on it.
fn output_changes(run: &Results, base: &Results) -> Vec<String> {
    let mut lines = Vec::new();
    for w in &run.workloads {
        let key = format!("output_hash.{}", w.name);
        let same_inputs = base.workload(&w.name).is_some_and(|b| b.input_hash == w.input_hash);
        if let (Some(now), Some(then), true) = (run.get(&key), base.get(&key), same_inputs)
            && now != then
        {
            lines.push(format!("OUTPUT CHANGED {}: {then} -> {now} (same inputs)", w.name));
        }
    }
    lines
}

/// Prints the run, keeps it, and does the action with it.
fn conclude(
    options: &Options,
    results: &Results,
    baseline: &Path,
    shown: &str,
    all: &[Workload],
    env: &Env,
) -> Result<u8, String> {
    print!("{}", report::table(results));
    let number = |key: &str| results.number(key).unwrap_or(0);
    let noisy = results.get("noisy") == Some("yes");
    println!(
        "\ncalibration {} ms before, {} ms after{}",
        report::ms(number("calibration_before_us")),
        report::ms(number("calibration_after_us")),
        if noisy { ": NOISY (more than 10% apart)" } else { "" }
    );
    if let Some(s) = verify::summary(results) {
        print!("{s}");
    }
    let results_path = env.work.join("results.tsv");
    results.write(&results_path)?;
    say(&format!("results: {}", results_path.display()));

    let line = |l: &Link| {
        format!(
            "{} {} {}: {} {}",
            l.workload,
            l.linker,
            l.threads.text(),
            l.status.text(),
            l.detail
        )
    };
    let mut killed = false;
    for l in results.links.iter().filter(|l| l.status == Status::Killed) {
        eprintln!("xtask bench: KILLED by a limit: {}", line(l));
        killed = true;
    }
    // A wrong output outranks every other verdict of the run.
    let wrong = verify::found(results);
    if !wrong.is_empty() {
        for w in &wrong {
            eprintln!("xtask bench: mortar's output is WRONG: {w}");
        }
        if options.action == Action::UpdateBaseline {
            eprintln!("xtask bench: not recording a baseline with a wrong output");
        }
        return Ok(1);
    }
    let broken: Vec<&Link> = results
        .links
        .iter()
        .filter(|l| l.linker != stats::JUDGED && l.status != Status::Ok)
        .collect();
    if !broken.is_empty() {
        for l in &broken {
            eprintln!("xtask bench: a reference linker did not link: {}", line(l));
        }
        if options.action == Action::UpdateBaseline {
            return Err("not recording a baseline in which a reference linker did not link".into());
        }
        return Ok(3);
    }
    // A noisy baseline would make every later time unjudgeable
    // (spec/harness.md "Baselines").
    if options.action == Action::UpdateBaseline && results.get("noisy") == Some("yes") {
        return Err("not recording a baseline from a noisy run (the calibrations before and \
                    after differ by more than 10%); run it again"
            .into());
    }
    match options.action {
        Action::Check => check(results, baseline, shown, killed),
        Action::UpdateBaseline => {
            results.write(baseline)?;
            let entry = format!(
                "{}\t{}\t{}\n",
                results.get("date").unwrap_or(""),
                results.get("commit").unwrap_or(""),
                record::escape(options.reason.as_deref().unwrap_or(""))
            );
            append(&baseline.with_extension("log"), &entry)?;
            say(&format!("baseline: {shown}"));
            write_status_page(&env.workspace, results, shown, all)?;
            Ok(0)
        }
        _ => Ok(u8::from(killed) * 3),
    }
}

/// Judges `results` against the baseline (`--check`, `--judge`); the exit
/// status.
fn check(results: &Results, baseline: &Path, shown: &str, killed: bool) -> Result<u8, String> {
    let base = Results::read(baseline).map_err(|e| format!("no baseline to check against: {e}"))?;
    let check = Check::new(results, &base, &Thresholds::default());
    print!("\ncheck against {shown}:\n{}", report::check_lines(&check));
    for line in output_changes(results, &base) {
        println!("{line}");
    }
    Ok(if check.failed() {
        1
    } else if check.noisy() || killed {
        3
    } else {
        0
    })
}

fn write_status_page(
    workspace: &Path,
    results: &Results,
    shown: &str,
    all: &[Workload],
) -> Result<(), String> {
    let path = workspace.join(report::STATUS_PAGE);
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)
            .map_err(|e| format!("cannot create {}: {e}", dir.display()))?;
    }
    std::fs::write(&path, report::status_page(results, shown, all))
        .map_err(|e| format!("cannot write {}: {e}", path.display()))?;
    say(&format!("status page: {}", report::STATUS_PAGE));
    let path = workspace.join(report::PERF_PAGE);
    std::fs::write(&path, report::perf_page(results, shown))
        .map_err(|e| format!("cannot write {}: {e}", path.display()))?;
    say(&format!("performance page: {}", report::PERF_PAGE));
    Ok(())
}

fn append(path: &Path, text: &str) -> Result<(), String> {
    use std::io::Write;
    std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
        .and_then(|mut file| file.write_all(text.as_bytes()))
        .map_err(|e| format!("cannot write {}: {e}", path.display()))
}

/// The 1, 5 and 15 minute load averages, as `/proc/loadavg` has them.
fn load_average() -> String {
    let text = std::fs::read_to_string("/proc/loadavg").unwrap_or_default();
    let fields: Vec<&str> = text.split_whitespace().take(3).collect();
    if fields.len() == 3 { fields.join(" ") } else { "unknown".to_owned() }
}

/// The checkout's commit, with `+changes` when its tree differs from it.
fn commit(workspace: &Path) -> String {
    let limits = Limits { time: Duration::from_mins(1), memory_kib: 4 << 20 };
    let git = |args: &[&str]| -> Option<String> {
        let output = process::run(
            &which("git").ok()?,
            |c| {
                c.arg("-C").arg(workspace).args(args);
            },
            b"",
            limits,
        )
        .ok()?;
        output.end.success().then(|| String::from_utf8_lossy(&output.stdout).trim().to_owned())
    };
    let Some(head) = git(&["rev-parse", "--short=12", "HEAD"]) else { return "unknown".to_owned() };
    match git(&["status", "--porcelain", "--untracked-files=no"]) {
        Some(changes) if changes.is_empty() => head,
        _ => format!("{head}+changes"),
    }
}

/// Finds a linker; mortar is built (release profile) unless given.
fn find_linker(
    name: &'static str,
    workspace: &Path,
    mortar: Option<&Path>,
) -> Result<Linker, String> {
    let path = match name {
        "bfd" => PathBuf::from(prepare::GNU_LD),
        "lld" => mortar_testutil::find_tool("ld.lld")
            .ok_or("lld (ld.lld-22 or ld.lld-19) is not installed")?,
        "mold" => mortar_testutil::find_tool("mold").ok_or("mold is not on PATH")?,
        _ => match mortar {
            Some(path) => path.to_path_buf(),
            None => build_mortar(workspace)?,
        },
    };
    if !path.is_file() {
        return Err(format!("{name}: {} is not a file", path.display()));
    }
    let version = prepare::version_line(&path)?;
    Ok(Linker { name, path, version })
}

/// `cargo build --release -p mortar` in the checkout.
pub fn build_mortar(workspace: &Path) -> Result<PathBuf, String> {
    say("building mortar (release profile)");
    let mut cargo = Command::new(std::env::var_os("CARGO").unwrap_or_else(|| "cargo".into()));
    cargo
        .args(["build", "--release", "--quiet", "-p", "mortar", "--bin", "mortar"])
        .current_dir(workspace);
    let (status, _) =
        process::run_step(&mut cargo, true).map_err(|e| format!("cannot run cargo: {e}"))?;
    if !status.success() {
        return Err(format!("building mortar: cargo {status}"));
    }
    Ok(target_dir(workspace).join("release/mortar"))
}

// spec/harness.md "bench", "Command".
#[cfg(test)]
mod tests {
    use super::*;

    fn parse(args: &[&str]) -> Result<Options, String> {
        Options::parse(&args.iter().map(|a| (*a).to_owned()).collect::<Vec<_>>())
    }

    // "Command": the options and their errors.
    #[test]
    fn options() {
        let plain = parse(&[]).unwrap();
        assert_eq!((plain.action, plain.runs), (Action::Measure, DEFAULT_RUNS));
        let o = parse(&[
            "--check",
            "--workload",
            "hello-c",
            "--workload=hello-cxx",
            "--linker",
            "mortar",
            "--runs=3",
        ])
        .unwrap();
        assert_eq!(o.action, Action::Check);
        assert_eq!(
            (o.workloads.len(), o.linkers.as_slice(), o.runs),
            (2, ["mortar".to_owned()].as_slice(), 3)
        );
        let o = parse(&["--update-baseline", "--reason", "first numbers"]).unwrap();
        assert_eq!(
            (o.action, o.reason.as_deref()),
            (Action::UpdateBaseline, Some("first numbers"))
        );
        assert!(parse(&["--update-baseline"]).unwrap_err().contains("needs `--reason"));
        assert!(
            parse(&["--update-baseline", "--reason", " "]).unwrap_err().contains("needs `--reason")
        );
        assert!(
            parse(&["--update-baseline", "--reason", "x", "--linker", "lld"])
                .unwrap_err()
                .contains("every linker")
        );
        assert!(parse(&["--reason", "x"]).unwrap_err().contains("goes with"));
        assert!(parse(&["--check", "--report"]).unwrap_err().contains("one action"));
        assert!(parse(&["--linker", "gold"]).unwrap_err().contains("unknown linker `gold`"));
        assert!(parse(&["--runs", "0"]).unwrap_err().contains("1 to 100"));
        assert!(parse(&["--runs", "101"]).is_err());
        assert!(parse(&["--workload"]).unwrap_err().contains("needs a value"));
        assert!(parse(&["--check=yes"]).unwrap_err().contains("takes no value"));
        assert!(parse(&["-j4"]).unwrap_err().contains("unknown argument"));
        assert!(parse(&["--help"]).unwrap().help);
        // "Verification": on by default; `--no-verify` for timing loops,
        // never for a baseline.
        assert!(plain.verify);
        assert!(!parse(&["--check", "--no-verify"]).unwrap().verify);
        assert!(
            parse(&["--update-baseline", "--reason", "x", "--no-verify"])
                .unwrap_err()
                .contains("only verified outputs")
        );
        assert!(parse(&["--no-verify=1"]).unwrap_err().contains("takes no value"));
        // `--judge <file>` is an action of its own.
        let o = parse(&["--judge", "r.tsv"]).unwrap();
        assert_eq!((o.action, o.judge.as_deref()), (Action::Judge, Some(Path::new("r.tsv"))));
        assert!(parse(&["--judge=r.tsv", "--check"]).unwrap_err().contains("one action"));
        assert!(parse(&["--judge"]).unwrap_err().contains("needs a value"));
    }

    // "Workloads": the checked-in manifest reads, has a calibration
    // workload, and its sources exist.
    #[test]
    fn the_checked_in_manifest() {
        let root = workspace_root();
        let workloads =
            manifest::parse(&std::fs::read_to_string(root.join(manifest::PATH)).unwrap()).unwrap();
        assert_eq!(workloads.iter().filter(|w| w.class == CALIBRATION).count(), 1);
        for w in &workloads {
            if let Kind::Sources { sources } = &w.kind {
                for source in sources {
                    assert!(
                        root.join(manifest::SOURCES).join(source).is_file(),
                        "{}: {source}",
                        w.name
                    );
                }
            }
        }
        assert!(workloads.iter().any(|w| matches!(w.kind, Kind::Planned { .. })));
    }

    // "Baselines": one file per machine, named by its id.
    #[test]
    fn baseline_paths() {
        let machine = Machine {
            arch: "aarch64".into(),
            cpu: "implementer 0x61 part 0x000".into(),
            cores: 24,
        };
        assert_eq!(
            baseline_path(Path::new("/w"), &machine),
            Path::new("/w/bench/baselines/aarch64-implementer-0x61-part-0x000-24c.tsv")
        );
    }

    #[test]
    fn output_changes_are_reported_for_unchanged_inputs() {
        let info = |name: &str, input: &str| record::WorkloadInfo {
            name: name.to_owned(),
            class: "c".to_owned(),
            objects: 1,
            archives: 0,
            shared: 0,
            others: 0,
            input_bytes: 1,
            symbols: 1,
            output_bytes: 1,
            input_hash: input.to_owned(),
        };
        let results = |hashes: [&str; 3], inputs: [&str; 3]| {
            let mut r = Results::default();
            for (i, w) in ["a", "b", "c"].into_iter().enumerate() {
                r.workloads.push(info(w, inputs[i]));
                r.set(&format!("output_hash.{w}"), hashes[i]);
            }
            r
        };
        let base = results(["h1", "h2", "h3"], ["i1", "i2", "i3"]);
        // a: same; b: output changed; c: output and inputs changed.
        let run = results(["h1", "h9", "h8"], ["i1", "i2", "i7"]);
        assert_eq!(
            output_changes(&run, &base),
            vec!["OUTPUT CHANGED b: h2 -> h9 (same inputs)".to_owned()]
        );
    }
}
