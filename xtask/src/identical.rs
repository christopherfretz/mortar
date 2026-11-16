//! `cargo xtask static-identical [--update] [-j <n>]` (`spec/harness.md`
//! "static-identical"): the digests of every mortar link of a corpus —
//! every link step of `tests/link` (its dynamic, PIE and static-pie cases
//! among them) and `tests/identical` on every architecture — against the
//! checked-in baseline of the machine it runs on,
//! `tests/identical/baseline.on-<host>.tsv`.
//!
//! It holds mortar's output to being deterministic, which incremental
//! linking rests on: a link's bytes, its `--mortar-map` and its
//! diagnostics stay what they were. A change that means to change them
//! rewrites the baseline with `--update`, which prints the links whose
//! digests changed, and the commit carries both.

use std::collections::{BTreeMap, BTreeSet};
use std::fmt::Write as _;
use std::path::PathBuf;
use std::time::Instant;

use crate::linktest::engine::DigestRow;
use crate::linktest::outcome::Outcome;
use crate::linktest::target::{Arch, Linker};
use crate::linktest::{self, build_dir, workspace_root};

pub const USAGE: &str =
    "usage: cargo xtask static-identical [--update] [-j <n>] [--mortar <binary>]\n";

/// The baseline of a machine of architecture `host`, relative to the
/// workspace. One per host, because the inputs are the host's: a host's
/// own architecture is built by its native GCC and C library (Debian's
/// `libc6-dev`, `libgcc-14-dev`), the others by the cross packages
/// (`libc6-dev-<arch>-cross`), which are other builds of other revisions,
/// so the same test links other bytes on an AArch64 and an x86-64 machine
/// (spec/harness.md "static-identical", "One baseline per host").
pub fn baseline_file(host: Arch) -> String {
    format!("tests/identical/baseline.on-{}.tsv", host.name())
}

/// The first line of the baseline: its columns.
const HEADER: &str = "# test\tarch\tline\tend\toutput\tmap\tstderr";

/// What `cargo xtask static-identical` is asked for.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Args {
    /// Rewrite the baseline instead of checking against it.
    pub update: bool,
    pub jobs: Option<usize>,
    /// mortar's binary; otherwise the workspace's is built (to compare
    /// another commit's links with the baseline).
    pub mortar: Option<PathBuf>,
}

impl Args {
    /// Parses the arguments after `static-identical`.
    ///
    /// # Errors
    /// An unknown argument, or a number of jobs that is not from 1 to 64.
    pub fn parse(args: &[String]) -> Result<Self, String> {
        let mut parsed = Self::default();
        let mut rest = args.iter();
        while let Some(arg) = rest.next() {
            let jobs = match arg.as_str() {
                "--update" => {
                    parsed.update = true;
                    continue;
                }
                "--mortar" => {
                    parsed.mortar = Some(rest.next().ok_or("--mortar needs a value")?.into());
                    continue;
                }
                "-j" | "--jobs" => rest.next().ok_or("-j needs a value")?.as_str(),
                other if other.starts_with("--mortar=") => {
                    parsed.mortar = Some(other["--mortar=".len()..].into());
                    continue;
                }
                other => match other.strip_prefix("--jobs=").or_else(|| other.strip_prefix("-j")) {
                    Some(n) if !n.is_empty() => n,
                    _ => return Err(format!("unknown argument `{other}`")),
                },
            };
            parsed.jobs = Some(
                jobs.parse::<usize>()
                    .ok()
                    .filter(|n| (1..=64).contains(n))
                    .ok_or("-j takes a number from 1 to 64")?,
            );
        }
        Ok(parsed)
    }
}

/// A row's key: the link step.
type Key = (String, String, usize);

/// A row's digests: end, output, map, standard error.
type Digests = [String; 4];

const COLUMNS: [&str; 4] = ["end", "output", "map", "stderr"];

fn key_of(row: &DigestRow) -> Key {
    (row.test.clone(), row.arch.name().to_owned(), row.line)
}

fn digests_of(row: &DigestRow) -> Digests {
    [row.end.clone(), row.output.clone(), row.map.clone(), row.stderr.clone()]
}

/// The baseline's text for `rows`.
pub fn format_baseline(rows: &[DigestRow]) -> String {
    let mut text = String::from(
        "# The static-identical baseline (spec/harness.md \"static-identical\"):\n\
         # one mortar link step per line; rewritten by `cargo xtask static-identical --update`.\n",
    );
    text.push_str(HEADER);
    text.push('\n');
    text.push_str(&rows_text(rows));
    text
}

/// `rows` as baseline lines.
fn rows_text(rows: &[DigestRow]) -> String {
    let mut text = String::new();
    for row in rows {
        let _ = writeln!(
            text,
            "{}\t{}\t{}\t{}\t{}\t{}\t{}",
            row.test,
            row.arch.name(),
            row.line,
            row.end,
            row.output,
            row.map,
            row.stderr
        );
    }
    text
}

/// Parses a baseline.
///
/// # Errors
/// A line that is not seven tab-separated fields, or a repeated link.
pub fn parse_baseline(name: &str, text: &str) -> Result<BTreeMap<Key, Digests>, String> {
    let mut rows = BTreeMap::new();
    for (n, line) in text.lines().enumerate() {
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let fields: Vec<&str> = line.split('\t').collect();
        let bad = || format!("{name}:{}: not a baseline row", n + 1);
        let [test_name, arch, step, end, output, map, stderr] = fields[..] else {
            return Err(bad());
        };
        let step = step.parse().map_err(|_| bad())?;
        let key = (test_name.to_owned(), arch.to_owned(), step);
        let digests = [end, output, map, stderr].map(str::to_owned);
        if rows.insert(key, digests).is_some() {
            return Err(format!("{name}:{}: a repeated link", n + 1));
        }
    }
    Ok(rows)
}

/// How the run's rows differ from the baseline's, one line each, in key
/// order: `+` a new link, `-` a link no longer made, `~` changed digests
/// (named). Baseline rows of `not_run` units (`(test, arch)`) are not
/// compared.
pub fn diff(
    baseline: &BTreeMap<Key, Digests>,
    rows: &[DigestRow],
    not_run: &BTreeSet<(String, String)>,
) -> Vec<String> {
    let current: BTreeMap<Key, Digests> = rows.iter().map(|r| (key_of(r), digests_of(r))).collect();
    let name = |(test, arch, line): &Key| format!("{test} [{arch}] line {line}");
    let mut lines = Vec::new();
    let keys: BTreeSet<&Key> = baseline.keys().chain(current.keys()).collect();
    for key in keys {
        match (baseline.get(key), current.get(key)) {
            (Some(_), None) if not_run.contains(&(key.0.clone(), key.1.clone())) => {}
            (Some(_), None) => lines.push(format!("- {}", name(key))),
            (None, Some(_)) => lines.push(format!("+ {}", name(key))),
            (Some(old), Some(new)) if old != new => {
                let changed: Vec<&str> = COLUMNS
                    .iter()
                    .zip(old.iter().zip(new))
                    .filter(|(_, (o, n))| o != n)
                    .map(|(c, _)| *c)
                    .collect();
                let end = if old[0] == new[0] {
                    String::new()
                } else {
                    format!(" ({} -> {})", old[0], new[0])
                };
                lines.push(format!("~ {}: {}{end}", name(key), changed.join(", ")));
            }
            _ => {}
        }
    }
    lines
}

/// `cargo xtask static-identical <args>`: 0 when the digests are the
/// baseline's (or `--update` wrote it), 1 when they differ or the run
/// could not do its work, 2 for a usage error.
pub fn main(args: &[String]) -> u8 {
    match Args::parse(args) {
        Ok(args) => u8::from(!run(&args)),
        Err(e) => {
            eprint!("xtask static-identical: {e}\n{USAGE}");
            2
        }
    }
}

/// The corpus: every directory of `tests/link` (the dynamic, PIE and
/// static-pie cases of `tests/link/dyn` among them, `spec/harness.md`
/// "static-identical", "The corpus"), and `tests/identical`.
fn corpus(root: &std::path::Path) -> Result<Vec<PathBuf>, String> {
    let link = root.join("tests").join("link");
    let entries =
        std::fs::read_dir(&link).map_err(|e| format!("cannot read '{}': {e}", link.display()))?;
    let mut paths = Vec::new();
    for entry in entries {
        let entry = entry.map_err(|e| format!("cannot read '{}': {e}", link.display()))?;
        let path = entry.path();
        if path.is_dir() {
            paths.push(path);
        }
    }
    paths.sort();
    paths.push(root.join("tests").join("identical"));
    Ok(paths)
}

/// Runs the corpus in the digest mode and checks (or rewrites) the
/// baseline; returns whether it passed.
#[allow(clippy::too_many_lines)] // the run, its problems, the units not run, then the verdict
pub fn run(args: &Args) -> bool {
    let started = Instant::now();
    let root = workspace_root();
    let linktest_args = linktest::Args {
        linkers: vec![Linker::Mortar],
        jobs: args.jobs,
        mortar: args.mortar.clone(),
        results: Some(build_dir().join("static-identical-results.tsv")),
        no_plumb: true,
        digest: true,
        paths: match corpus(&root) {
            Ok(paths) => paths,
            Err(e) => {
                eprintln!("static-identical: {e}");
                return false;
            }
        },
        ..linktest::Args::default()
    };
    let report = match linktest::run(&linktest_args) {
        Ok(report) => report,
        Err(e) => {
            eprintln!("static-identical: {e}");
            return false;
        }
    };
    for note in &report.notes {
        eprintln!("static-identical: note: {note}");
    }
    let mut problems: Vec<String> = report.errors.clone();
    problems.extend(report.missing.iter().map(|m| format!("missing: {m}")));
    problems.extend(
        report
            .records
            .iter()
            .filter(|r| r.outcome == Outcome::Error)
            .map(|r| format!("{} [{}]: {}", r.test, r.arch.map_or("-", Arch::name), r.detail)),
    );
    // A unit that did not run (a tool that is not there, a skip) has no
    // rows to compare.
    let ran: BTreeSet<(String, String)> = report
        .digests
        .iter()
        .map(|r| (r.test.clone(), r.arch.name().to_owned()))
        .chain(
            report
                .records
                .iter()
                .filter(|r| r.outcome != Outcome::Skip)
                .filter_map(|r| Some((r.test.clone(), r.arch?.name().to_owned()))),
        )
        .collect();
    let Some(host) = Arch::host() else {
        eprintln!("static-identical: this machine is none of the architectures with a baseline");
        return false;
    };
    let name = baseline_file(host);
    let baseline_path: PathBuf = root.join(&name);
    let baseline = match std::fs::read_to_string(&baseline_path) {
        Ok(text) => parse_baseline(&name, &text),
        // No baseline yet for this host: every link differs (and is
        // printed as a row), so the check fails and the rows cut one.
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(BTreeMap::new()),
        Err(e) => Err(format!("{}: {e}", baseline_path.display())),
    };
    let baseline = match baseline {
        Ok(baseline) => baseline,
        Err(e) => {
            eprintln!("static-identical: {e}");
            return false;
        }
    };
    let not_run: BTreeSet<(String, String)> = baseline
        .keys()
        .map(|(test, arch, _)| (test.clone(), arch.clone()))
        .filter(|unit| !ran.contains(unit) && !mortar_testutil::tools_required())
        .collect();
    let changes = diff(&baseline, &report.digests, &not_run);
    let elapsed = started.elapsed().as_secs_f64();
    let summary = format!(
        "static-identical: {} links of {} tests on {}; {} differ from {name}; {elapsed:.1}s",
        report.digests.len(),
        report.tests,
        report.arches.iter().map(|a| a.name()).collect::<Vec<_>>().join(" "),
        changes.len(),
    );
    if !not_run.is_empty() {
        eprintln!("static-identical: note: {} units not run here are not compared", not_run.len());
    }
    if !problems.is_empty() {
        for p in &problems {
            eprintln!("static-identical: {p}");
        }
        eprintln!("{summary}\nstatic-identical: FAILED (the run did not do its work)");
        return false;
    }
    for line in &changes {
        eprintln!("{line}");
    }
    if !changes.is_empty() && !args.update {
        // The run's own rows of the links that differ, as baseline rows:
        // digests and test names only, so that the baseline of a host one
        // does not have can be cut from its hosted lane's log.
        let rows: Vec<DigestRow> = report
            .digests
            .iter()
            .filter(|r| baseline.get(&key_of(r)) != Some(&digests_of(r)))
            .cloned()
            .collect();
        eprint!("static-identical: the run's rows of the links that differ:\n{}", rows_text(&rows));
    }
    if args.update {
        if !not_run.is_empty() {
            eprintln!(
                "{summary}\nstatic-identical: refusing to --update without every unit: the tools \
                 of {} units are not here",
                not_run.len()
            );
            return false;
        }
        if let Err(e) = std::fs::write(&baseline_path, format_baseline(&report.digests)) {
            eprintln!("static-identical: cannot write '{}': {e}", baseline_path.display());
            return false;
        }
        eprintln!("{summary}\nstatic-identical: wrote {name}");
        return true;
    }
    eprintln!("{summary}");
    if changes.is_empty() {
        eprintln!("static-identical: ok");
        true
    } else {
        eprintln!(
            "static-identical: FAILED: a link changed; if the change is meant, \
             `cargo xtask static-identical --update` and commit the baseline"
        );
        false
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // spec/harness.md "static-identical", "The corpus": every directory
    // of `tests/link`, the dynamic cases of `dyn/` among them, and
    // `tests/identical`.
    #[test]
    fn the_corpus_is_every_link_test() {
        let root = workspace_root();
        let paths = corpus(&root).unwrap();
        let link = root.join("tests").join("link");
        assert!(paths.contains(&link.join("resolve")));
        assert!(paths.contains(&link.join("dyn")));
        assert_eq!(paths.last(), Some(&root.join("tests").join("identical")));
    }

    fn row(test: &str, line: usize, output: &str) -> DigestRow {
        DigestRow {
            test: test.to_owned(),
            arch: Arch::Aarch64,
            line,
            end: "exit status 0".to_owned(),
            output: output.to_owned(),
            map: "m".to_owned(),
            stderr: "e".to_owned(),
        }
    }

    // spec/harness.md "static-identical": the baseline round-trips, and
    // the diff names each link by test, architecture and line.
    #[test]
    fn baseline_and_diff() {
        let rows = [row("a.test", 3, "x"), row("b.test", 5, "y")];
        let baseline = parse_baseline("b", &format_baseline(&rows)).unwrap();
        assert_eq!(baseline.len(), 2);
        assert!(diff(&baseline, &rows, &BTreeSet::new()).is_empty());
        let now = [row("a.test", 3, "z"), row("c.test", 1, "y")];
        assert_eq!(
            diff(&baseline, &now, &BTreeSet::new()),
            [
                "~ a.test [aarch64] line 3: output",
                "- b.test [aarch64] line 5",
                "+ c.test [aarch64] line 1"
            ]
        );
        let not_run = BTreeSet::from([("b.test".to_owned(), "aarch64".to_owned())]);
        assert_eq!(diff(&baseline, &now, &not_run).len(), 2, "a unit not run is not compared");
        assert!(parse_baseline("b", "a\tb\n").is_err());
        assert_eq!(baseline_file(Arch::Aarch64), "tests/identical/baseline.on-aarch64.tsv");
        for host in [Arch::Aarch64, Arch::X86_64] {
            assert!(
                workspace_root().join(baseline_file(host)).is_file(),
                "{}",
                baseline_file(host)
            );
        }
        assert!(
            parse_baseline("b", "a\taarch64\t1\te\to\tm\ts\na\taarch64\t1\te\to\tm\ts\n").is_err()
        );
    }

    #[test]
    fn the_command_line() {
        let parse =
            |a: &[&str]| Args::parse(&a.iter().map(|s| (*s).to_owned()).collect::<Vec<_>>());
        assert_eq!(parse(&[]).unwrap(), Args::default());
        assert_eq!(
            parse(&["--update", "-j3"]).unwrap(),
            Args { update: true, jobs: Some(3), mortar: None }
        );
        assert_eq!(parse(&["--mortar=/m"]).unwrap().mortar, Some(PathBuf::from("/m")));
        assert_eq!(parse(&["-j", "2"]).unwrap().jobs, Some(2));
        assert!(parse(&["-j0"]).is_err() && parse(&["--fast"]).is_err());
    }
}
