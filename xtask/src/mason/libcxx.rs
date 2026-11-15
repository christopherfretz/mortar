//! `cargo xtask mason libcxx`: libc++'s tests through the Mason lane
//! (`tests/mason/README.md` "libc++"). kiln's own runner
//! (`kiln/spec/libcxx.md`, built by `setup`) runs the tests with the lane's
//! compiler (`tests/mason/lane/kiln++`) once per mode:
//!
//! - `dyn`: kiln's procedure unchanged (static libc++, dynamic glibc, the
//!   host's `ld`);
//! - `bfd`, `lld`, `mold`, `mortar`: fully static, with that linker;
//! - `mortar-noattrs`: mortar on the archives without `.ARM.attributes`
//!   (what mortar does once that is accepted);
//! - `mortar-stripped`: mortar on the archives without `.ARM.attributes`
//!   and `.deplibs` (what mortar does once those are accepted).
//!
//! The report: counts per mode, the tests a reference mode passes and a
//! mortar mode does not (a link refusal, or WRONG: linked and failed to
//! run as the reference's did), mortar's refusals by diagnostic, the
//! verifier's results (`--plumb`), and which back end compiled each job.

use std::collections::BTreeMap;
use std::fmt::Write as _;
use std::path::{Path, PathBuf};
use std::process::ExitCode;

use super::{outcomes, setup};

pub const MODES: &[&str] =
    &["dyn", "bfd", "lld", "mold", "mortar", "mortar-noattrs", "mortar-stripped"];

/// The samples: named sets of test directories (relative to
/// `libcxx/test`) with a shard.
pub const SAMPLES: &[(&str, &[&str], Option<&str>)] = &[
    // The whole suite (`--full`), as kiln's release runs take it.
    ("full", &["std", "libcxx"], None),
    ("vector", &["std/containers/sequences/vector"], None),
    (
        "wp033",
        &[
            "std/algorithms",
            "std/strings",
            "std/input.output",
            "std/language.support",
            "std/thread",
            "std/localization",
        ],
        Some("1/4"),
    ),
];

#[derive(Debug)]
struct Args {
    modes: Vec<String>,
    backend: String,
    dirs: Vec<String>,
    shard: Option<String>,
    jobs: usize,
    plumb: bool,
    report: Option<PathBuf>,
    mortar: Option<PathBuf>,
    keep: bool,
    /// `--out <dir>`: the run's directory (one subdirectory per mode),
    /// kept; else a scratch directory, removed.
    out: Option<PathBuf>,
    /// `--results <file>`: the per-test result file (`outcomes.rs`).
    results: Option<PathBuf>,
    /// `--compare <a> <b>`: compare two result files (`<file>[:<mode>]`),
    /// run nothing.
    compare: Option<(String, String)>,
    /// `--collect <dir>`: the result file of an earlier `--out <dir>` run.
    collect: Option<PathBuf>,
    /// `--baseline <file>`: mortar's expected not-passing tests.
    baseline: Option<PathBuf>,
    /// `--write-baseline <file>`: write mortar mode's baseline.
    write_baseline: Option<PathBuf>,
}

fn parse(args: &[String]) -> Result<Args, String> {
    let mut a = Args {
        modes: Vec::new(),
        backend: "hod".into(),
        dirs: Vec::new(),
        shard: None,
        jobs: 4,
        plumb: false,
        report: None,
        mortar: None,
        keep: false,
        out: None,
        results: None,
        compare: None,
        collect: None,
        baseline: None,
        write_baseline: None,
    };
    let mut sample = None;
    let mut it = args.iter();
    while let Some(x) = it.next() {
        let mut value = |n: &str| it.next().cloned().ok_or_else(|| format!("{n} needs a value"));
        match x.as_str() {
            "--mode" => {
                let m = value("--mode")?;
                if !MODES.contains(&m.as_str()) {
                    return Err(format!("unknown mode '{m}' (modes: {})", MODES.join(", ")));
                }
                a.modes.push(m);
            }
            "--backend" => {
                a.backend = value("--backend")?;
                if !matches!(a.backend.as_str(), "hod" | "llvm") {
                    return Err("--backend is hod or llvm".into());
                }
            }
            "--sample" => sample = Some(value("--sample")?),
            "--full" => sample = Some("full".to_owned()),
            "--shard" => a.shard = Some(value("--shard")?),
            "-j" | "--jobs" => {
                a.jobs = value("-j")?
                    .parse()
                    .ok()
                    .filter(|n| (1..=8).contains(n))
                    .ok_or("-j needs 1 to 8")?;
            }
            "--plumb" => a.plumb = true,
            "--keep" => a.keep = true,
            "--report" => a.report = Some(PathBuf::from(value("--report")?)),
            "--mortar" => a.mortar = Some(PathBuf::from(value("--mortar")?)),
            "--out" => a.out = Some(PathBuf::from(value("--out")?)),
            "--results" => a.results = Some(PathBuf::from(value("--results")?)),
            "--collect" => a.collect = Some(PathBuf::from(value("--collect")?)),
            "--baseline" => a.baseline = Some(PathBuf::from(value("--baseline")?)),
            "--write-baseline" => {
                a.write_baseline = Some(PathBuf::from(value("--write-baseline")?));
            }
            "--compare" => {
                let x = value("--compare")?;
                let y = value("--compare")?;
                a.compare = Some((x, y));
            }
            s if s.starts_with('-') => return Err(format!("unknown argument '{s}'")),
            s => a.dirs.push(s.to_owned()),
        }
    }
    if a.modes.is_empty() {
        a.modes = ["bfd", "mortar"].map(str::to_owned).into();
    }
    if let Some(s) = sample {
        let (_, dirs, shard) = SAMPLES
            .iter()
            .find(|(n, _, _)| *n == s)
            .ok_or_else(|| format!("unknown sample '{s}'"))?;
        a.dirs.extend(dirs.iter().map(|d| (*d).to_owned()));
        if a.shard.is_none() {
            a.shard = shard.map(str::to_owned);
        }
    }
    if a.dirs.is_empty() && a.compare.is_none() && a.collect.is_none() {
        return Err(
            "libcxx needs test directories, --sample, --full, --compare or --collect".into()
        );
    }
    Ok(a)
}

/// One test's line of kiln's results file.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Line {
    pub outcome: String,
    pub detail: String,
    pub key: String,
}

/// kiln's results file (`kiln/spec/libcxx.md` "Results file"): test path
/// to its line.
pub fn results(text: &str) -> BTreeMap<String, Line> {
    text.lines()
        .filter(|l| !l.starts_with('#'))
        .filter_map(|l| {
            let f: Vec<&str> = l.split('\t').collect();
            (f.len() >= 4).then(|| {
                (
                    f[1].to_owned(),
                    Line { outcome: f[0].into(), detail: f[2].into(), key: f[3].into() },
                )
            })
        })
        .collect()
}

/// A diagnostic without its location: what follows the last `): ` or the
/// `link: ` prefix and `mortar: error: `.
pub fn refusal(key: &str) -> String {
    let k = key.strip_prefix("link: ").unwrap_or(key);
    let k = k.strip_prefix("mortar: error: ").unwrap_or(k);
    let k = k.rfind("): ").map_or(k, |i| &k[i + 3..]);
    k.to_owned()
}

/// The run of one mode.
struct Mode {
    name: String,
    results: BTreeMap<String, Line>,
    summary: String,
    /// The lane log's mortar lines: (status, diagnostic, plumb).
    links: Vec<(String, String, String)>,
    /// `backend` of each object job in kiln's statistics log.
    backends: BTreeMap<String, usize>,
}

#[allow(clippy::too_many_lines)] // one runner invocation per mode, set up in one place
pub fn main(args: &[String]) -> Result<ExitCode, String> {
    let args = parse(args)?;
    if let Some((x, y)) = &args.compare {
        return compare_files(x, y);
    }
    if let Some(dir) = &args.collect {
        return finish(&args, &collect_dir(dir, None)?);
    }
    let Some(mason) = super::Mason::find_or_skip("libcxx")? else { return Ok(ExitCode::SUCCESS) };
    let kx = setup::kiln_xtask();
    let tree = super::lane_dir().join("libcxx-22-static");
    let tests = setup::libcxx_tests();
    if !kx.is_file() || !tests.is_dir() || !tree.is_dir() {
        let e = format!("no lane at {} (cargo xtask mason setup)", super::lane_dir().display());
        if super::required() {
            return Err(e);
        }
        println!("xtask mason libcxx: skipped: {e}");
        return Ok(ExitCode::SUCCESS);
    }
    let lane_scripts = crate::linktest::workspace_root().join("tests/mason/lane");
    let mortar = if args.modes.iter().any(|m| m.starts_with("mortar")) {
        Some(super::mortar_binary(args.mortar.as_deref())?)
    } else {
        None
    };
    let me = std::env::current_exe().map_err(|e| format!("current_exe: {e}"))?;
    let scratch = args.out.clone().unwrap_or_else(|| {
        crate::linktest::build_dir().join("mason").join(format!("libcxx-{}", std::process::id()))
    });
    let mut modes = Vec::new();
    for m in &args.modes {
        let dir = scratch.join(m);
        let tmp = dir.join("tmp");
        std::fs::create_dir_all(&tmp).map_err(|e| format!("{}: {e}", tmp.display()))?;
        let log = dir.join("lane.log");
        let stats = dir.join("stats.jsonl");
        let results_file = dir.join("results.tsv");
        let out = std::fs::File::create(dir.join("out.log")).map_err(|e| e.to_string())?;
        let mut c = std::process::Command::new(&kx);
        c.arg("libcxx")
            .args(["--tag", setup::TAG, "--libcxx-tests"])
            .arg(&tests)
            .arg("--compiler")
            .arg(lane_scripts.join("kiln++"))
            .args(["--no-fail", "-j", &args.jobs.to_string(), "--results"])
            .arg(&results_file);
        if args.keep {
            c.arg("--keep");
        }
        if let Some(s) = &args.shard {
            c.args(["--shard", s]);
        }
        c.args(&args.dirs);
        c.current_dir(&dir)
            .stdout(out)
            .env("CARGO_TARGET_DIR", super::lane_dir().join("kiln-target"))
            .env("TMPDIR", &tmp)
            .env("MASON_KILN", &mason.kiln)
            .env("MASON_MODE", m)
            .env("MASON_LANE", &lane_scripts)
            .env("MASON_LD", if m.starts_with("mortar") { "mortar" } else { m.as_str() })
            .env("MASON_LOG", &log)
            .env("MASON_TIMES", dir.join("times.tsv"))
            .env("MASON_XTASK", &me)
            .env("MASON_PLUMB", if args.plumb { "1" } else { "0" })
            .env("KILN_STATS_LOG", &stats);
        if let Some(p) = &mortar {
            c.env("MASON_MORTAR", p);
        }
        if let Some(to) = m.strip_prefix("mortar-") {
            c.env(
                "MASON_LIB_FROM",
                super::lane_dir()
                    .join("kiln-src/.toolchain/libcxx-22-root/usr/lib/aarch64-linux-gnu"),
            )
            .env("MASON_LIB_TO", super::lane_dir().join(format!("libcxx-22-{to}")));
        }
        if args.backend == "hod" {
            c.env("KILN_BACKEND", "hod").env("KILN_HOD", &mason.hod).env("KILN_HOD_STRICT", "1");
        }
        println!("xtask mason libcxx: mode {m} ({} backend) ...", args.backend);
        match crate::process::run_step(&mut c, false) {
            Ok((s, _)) if s.success() => {}
            Ok((s, _)) => {
                return Err(format!(
                    "kiln's runner, mode {m}: {s} (see {})",
                    dir.join("out.log").display()
                ));
            }
            Err(e) => return Err(format!("kiln's runner: {e}")),
        }
        let read = |p: &Path| std::fs::read_to_string(p).unwrap_or_default();
        let summary = read(&dir.join("out.log"))
            .lines()
            .find(|l| l.contains(" tests: "))
            .unwrap_or("(no summary)")
            .to_owned();
        let links = read(&log)
            .lines()
            .filter_map(|l| {
                let f: Vec<&str> = l.split('\t').collect();
                (f.len() >= 5 && f[1] == "mortar")
                    .then(|| (f[2].to_owned(), f[3].to_owned(), f[4].to_owned()))
            })
            .collect();
        let mut backends = BTreeMap::new();
        for l in read(&stats).lines().filter(|l| l.contains("\"output_kind\":\"obj\"")) {
            let b = l
                .split("\"backend\":\"")
                .nth(1)
                .and_then(|r| r.split('"').next())
                .unwrap_or("?")
                .to_owned();
            *backends.entry(b).or_insert(0) += 1;
        }
        println!("xtask mason libcxx: mode {m}: {summary}");
        modes.push(Mode {
            name: m.clone(),
            results: results(&read(&results_file)),
            summary,
            links,
            backends,
        });
    }
    let report = render(&args, &modes);
    print!("{report}");
    if let Some(p) = &args.report {
        std::fs::write(p, &report).map_err(|e| format!("{}: {e}", p.display()))?;
    }
    if args.keep || args.out.is_some() {
        println!("xtask mason libcxx: kept {}", scratch.display());
    } else {
        let _ = std::fs::remove_dir_all(&scratch);
    }
    let wrong = modes.iter().any(|m| !wrong_outputs(&modes, m).is_empty());
    let names: Vec<String> = modes.iter().map(|m| m.name.clone()).collect();
    let code = if args.results.is_some() || args.baseline.is_some() || args.write_baseline.is_some()
    {
        finish(&args, &collect_dir(&scratch, Some(&names))?)?
    } else {
        ExitCode::SUCCESS
    };
    Ok(if wrong { ExitCode::FAILURE } else { code })
}

/// The records of the mode directories of `dir` (`names`, else every
/// subdirectory with a `results.tsv`), in mode order.
fn collect_dir(dir: &Path, names: Option<&[String]>) -> Result<Vec<outcomes::Rec>, String> {
    let names: Vec<String> = match names {
        Some(n) => n.to_vec(),
        None => {
            let mut v: Vec<String> = std::fs::read_dir(dir)
                .map_err(|e| format!("{}: {e}", dir.display()))?
                .filter_map(Result::ok)
                .filter(|e| e.path().join("results.tsv").is_file())
                .map(|e| e.file_name().to_string_lossy().into_owned())
                .collect();
            v.sort_by_key(|m| MODES.iter().position(|x| x == m).unwrap_or(usize::MAX));
            v
        }
    };
    let mut recs = Vec::new();
    for m in &names {
        recs.extend(outcomes::collect(m, &dir.join(m))?);
    }
    Ok(recs)
}

/// Writes the result file, the comparisons of every mode with the
/// reference (`bfd`), the link times, and the baseline's verdict.
fn finish(args: &Args, recs: &[outcomes::Rec]) -> Result<ExitCode, String> {
    if let Some(p) = &args.results {
        std::fs::write(p, outcomes::render(recs)).map_err(|e| format!("{}: {e}", p.display()))?;
        println!("xtask mason libcxx: results: {} ({} lines)", p.display(), recs.len());
    }
    let mut modes: Vec<&str> = Vec::new();
    for r in recs {
        if !modes.contains(&r.mode.as_str()) {
            modes.push(&r.mode);
        }
    }
    let by: Vec<(&str, BTreeMap<&str, &outcomes::Rec>)> = modes
        .iter()
        .map(|m| outcomes::select(recs, Some(m)).map(|s| (*m, s)))
        .collect::<Result<_, _>>()?;
    println!(
        "
{}",
        outcomes::TIMES_HEADER
    );
    for (m, s) in &by {
        println!("{}", outcomes::times(m, s));
    }
    if let Some((_, r)) = by.iter().find(|(m, _)| *m == "bfd") {
        for (m, s) in by.iter().filter(|(m, _)| *m != "bfd") {
            println!(
                "
### {m} against bfd

{}",
                outcomes::compare("bfd", r, m, s).0
            );
        }
    }
    let mut code = ExitCode::SUCCESS;
    if let Some((_, m)) = by.iter().find(|(m, _)| *m == "mortar") {
        if let Some(p) = &args.write_baseline {
            std::fs::write(p, outcomes::baseline(m))
                .map_err(|e| format!("{}: {e}", p.display()))?;
        }
        if let Some(p) = &args.baseline {
            let base = std::fs::read_to_string(p).map_err(|e| format!("{}: {e}", p.display()))?;
            let (regressed, fixed) = outcomes::against_baseline(m, &base);
            println!(
                "
against {}: regressed {}, fixed {}",
                p.display(),
                regressed.len(),
                fixed.len()
            );
            for t in &regressed {
                println!("regressed: {t}");
            }
            for t in &fixed {
                println!("fixed: {t}");
            }
            if !regressed.is_empty() {
                code = ExitCode::FAILURE;
            }
        }
    }
    Ok(code)
}

/// `--compare <a> <b>`: each `<file>[:<mode>]`.
fn compare_files(a: &str, b: &str) -> Result<ExitCode, String> {
    let load = |spec: &str| -> Result<(String, Vec<outcomes::Rec>, Option<String>), String> {
        let (file, mode) = match spec.rsplit_once(':') {
            Some((f, m)) if !m.contains('/') && Path::new(f).is_file() => (f, Some(m.to_owned())),
            _ => (spec, None),
        };
        let text = std::fs::read_to_string(file).map_err(|e| format!("{file}: {e}"))?;
        Ok((file.to_owned(), outcomes::parse(&text).map_err(|e| format!("{file}: {e}"))?, mode))
    };
    let (fa, ra, ma) = load(a)?;
    let (fb, rb, mb) = load(b)?;
    let sa = outcomes::select(&ra, ma.as_deref()).map_err(|e| format!("{fa}: {e}"))?;
    let sb = outcomes::select(&rb, mb.as_deref()).map_err(|e| format!("{fb}: {e}"))?;
    let name = |s: &BTreeMap<&str, &outcomes::Rec>| {
        s.values().next().map_or_else(String::new, |r| r.mode.clone())
    };
    let (na, nb) = (name(&sa), name(&sb));
    let (na, nb) = if na == nb { (format!("{na} (a)"), format!("{nb} (b)")) } else { (na, nb) };
    let (text, _) = outcomes::compare(&na, &sa, &nb, &sb);
    println!(
        "{text}
{}",
        outcomes::TIMES_HEADER
    );
    println!(
        "{}
{}",
        outcomes::times(&na, &sa),
        outcomes::times(&nb, &sb)
    );
    Ok(ExitCode::SUCCESS)
}

/// The reference mode: `bfd`, else `dyn`.
fn reference(modes: &[Mode]) -> Option<&Mode> {
    modes.iter().find(|m| m.name == "bfd").or_else(|| modes.iter().find(|m| m.name == "dyn"))
}

/// Tests the reference passes and `m` fails after its link succeeded.
fn wrong_outputs<'a>(modes: &'a [Mode], m: &'a Mode) -> Vec<(&'a String, &'a Line)> {
    let Some(r) = reference(modes) else { return Vec::new() };
    if std::ptr::eq(r, m) || m.name == "dyn" {
        return Vec::new();
    }
    m.results
        .iter()
        .filter(|(t, l)| {
            l.outcome != "PASS"
                && l.detail != "link"
                && l.detail != "compile"
                && r.results.get(*t).is_some_and(|rl| rl.outcome == "PASS")
        })
        .collect()
}

fn render(args: &Args, modes: &[Mode]) -> String {
    let mut s = String::new();
    let _ = writeln!(
        s,
        "## libc++ through the Mason lane\n\ndirs: {}; shard: {}; backend: {}\n",
        args.dirs.join(" "),
        args.shard.as_deref().unwrap_or("all"),
        args.backend
    );
    let _ = writeln!(s, "| mode | summary | object jobs by back end |\n|---|---|---|");
    for m in modes {
        let b: Vec<String> = m.backends.iter().map(|(k, n)| format!("{k} {n}")).collect();
        let _ = writeln!(s, "| {} | {} | {} |", m.name, m.summary.trim(), b.join(", "));
    }
    let reference = reference(modes);
    for m in modes {
        let failed: Vec<&String> = m
            .results
            .iter()
            .filter(|(_, l)| !matches!(l.outcome.as_str(), "PASS" | "UNSUPPORTED"))
            .map(|(t, _)| t)
            .collect();
        if !failed.is_empty() && failed.len() <= 10 {
            let _ = writeln!(
                s,
                "\n{} does not pass: {}",
                m.name,
                failed.iter().map(|t| t.as_str()).collect::<Vec<_>>().join(", ")
            );
        }
    }
    for m in modes.iter().filter(|m| m.name.starts_with("mortar")) {
        let _ = writeln!(s, "\n### {}", m.name);
        if let Some(r) = reference {
            let lost: Vec<&String> = m
                .results
                .iter()
                .filter(|(t, l)| {
                    l.outcome != "PASS" && r.results.get(*t).is_some_and(|rl| rl.outcome == "PASS")
                })
                .map(|(t, _)| t)
                .collect();
            let _ = writeln!(s, "\npass with {} and not with {}: {}", r.name, m.name, lost.len());
            let wrong = wrong_outputs(modes, m);
            let _ =
                writeln!(s, "WRONG (linked, then failed where {} passed): {}", r.name, wrong.len());
            for (t, l) in wrong.iter().take(40) {
                let _ = writeln!(s, "- {t}: {} {}", l.detail, l.key);
            }
        }
        let mut hist: BTreeMap<String, usize> = BTreeMap::new();
        for (st, diag, _) in &m.links {
            if st != "0" {
                *hist.entry(refusal(diag)).or_insert(0) += 1;
            }
        }
        let ok = m.links.iter().filter(|(st, _, _)| st == "0").count();
        let _ =
            writeln!(s, "\nlinks: {}; linked {ok}; refused {}", m.links.len(), m.links.len() - ok);
        let mut hist: Vec<(String, usize)> = hist.into_iter().collect();
        hist.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(&b.0)));
        if !hist.is_empty() {
            let _ = writeln!(s, "\n| refusals | diagnostic |\n|---|---|");
            for (d, n) in hist {
                let _ = writeln!(s, "| {n} | `{}` |", d.replace('|', "\\|"));
            }
        }
        if args.plumb {
            let mut ph: BTreeMap<String, usize> = BTreeMap::new();
            for (st, _, p) in m.links.iter().filter(|(st, _, _)| st == "0") {
                let _ = st;
                let k = p
                    .split(": ")
                    .nth(1)
                    .unwrap_or(p)
                    .split(" (")
                    .next()
                    .unwrap_or(p)
                    .trim()
                    .to_owned();
                *ph.entry(k).or_insert(0) += 1;
            }
            let _ = writeln!(s, "\nplumb on linked outputs:");
            for (k, n) in ph {
                let _ = writeln!(s, "- {n}: {k}");
            }
        }
    }
    s
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn results_file() {
        let r = results(
            "# outcome\ttest\tdetail\tfirst-key\twall-s\nFAIL\tstd/a.pass.cpp\tlink\tlink: x\t0.4\nPASS\tstd/b.pass.cpp\t-\t-\t0.1\n",
        );
        assert_eq!(r.len(), 2);
        assert_eq!(r["std/a.pass.cpp"].detail, "link");
        assert_eq!(r["std/b.pass.cpp"].outcome, "PASS");
    }

    #[test]
    fn refusal_drops_the_location() {
        assert_eq!(
            refusal(
                "mortar: error: /x/libc++.a(cxa_guard.cpp.o):(.deplibs+0x0): unsupported section type: 0x6fff4c04"
            ),
            "unsupported section type: 0x6fff4c04"
        );
        assert_eq!(refusal("link: a.o: undefined symbol: f"), "a.o: undefined symbol: f");
    }

    #[test]
    fn samples_parse() {
        let a = parse(&["--sample".into(), "wp033".into()]).unwrap();
        assert_eq!(a.shard.as_deref(), Some("1/4"));
        assert_eq!(a.dirs.len(), 6);
        assert!(parse(&["--mode".into(), "gold".into(), "x".into()]).is_err());
    }
}
