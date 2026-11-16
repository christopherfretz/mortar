//! `cargo xtask mason producers` (WP-099, `spec/landed/meta/WP-099.md`):
//! the one-definition-rule corpus (`tests/link/odr`, `tests/link/dyn/odr`)
//! linked by mortar with the objects of each [`Producer`], on AArch64 and
//! x86-64, plain and with each opt-in of [`VARIANTS`], and the answers
//! compared:
//!
//! - hod's container against the test producer's in hod's objects
//!   (`hod` against `hod-objcopy`): the same end and the same standard
//!   error, byte for byte. The objects differ only in who wrote the
//!   container, so a difference is WP-099's stop condition, never
//!   explained away.
//! - hod's objects against GCC's (`hod` against `gcc`) and one unit from
//!   each producer (`mixed` against `gcc`): the same end, the same
//!   one-definition-rule diagnostics (kind and files) and the same number
//!   of `meta-malformed` warnings; a difference carries its explanation
//!   in the baseline's last column, or the check fails.
//! - no output of a variant without `--keep-mason-meta` has a section
//!   named `.mason.meta`.
//!
//! The rows are checked against `tests/mason/producers.tsv`; `--update`
//! rewrites it, keeping each row's explanation.

use std::collections::BTreeMap;
use std::fmt::Write as _;
use std::process::ExitCode;

use crate::linktest::engine::{DigestRow, Producer};
use crate::linktest::outcome::Outcome;
use crate::linktest::target::{Arch, Linker};
use crate::linktest::{self, odr, workspace_root};

pub const USAGE: &str = "usage: cargo xtask mason producers [--update] [--mortar <binary>] [-j <n>] [--filter <s>]...\n";

/// The option variants every mortar link step runs with (variant 0: the
/// test's own line).
pub const VARIANTS: [&[&str]; 5] = [
    &[],
    &["--odr=strict"],
    &["--odr-check-archives"],
    &["--odr-table=full"],
    &["--keep-mason-meta"],
];

const BASELINE: &str = "tests/mason/producers.tsv";

const HEADER: &str = "# The producer equality of WP-099 (`cargo xtask mason producers`, \
spec/landed/meta/WP-099.md):\n\
# one mortar link step x variant per line: what GCC's objects, hod's objects and the mixed\n\
# link answer (end; diagnostics; meta-malformed warnings), then why hod's or the mixed answer\n\
# differs from GCC's. Rewritten by `--update`, which keeps the last column.\n\
# test\tarch\tline\tvariant\tgcc\thod\tmixed\twhy\n";

type Key = (String, String, usize, usize);

/// One link's answer, file names aside.
fn answer(row: &DigestRow) -> String {
    let mut text = row.end.clone();
    let reported = odr::reported(&row.stderr_text);
    text.push_str("; ");
    if reported.is_empty() {
        text.push('-');
    }
    for (i, r) in reported.iter().enumerate() {
        if i > 0 {
            text.push(' ');
        }
        text.push_str(r.id);
        text.push('(');
        text.push_str(&r.files.join(","));
        text.push(')');
    }
    let malformed = row.stderr_text.lines().filter(|l| l.contains("meta-malformed")).count();
    let _ = write!(text, "; meta-malformed {malformed}");
    text
}

fn key_of(row: &DigestRow) -> Key {
    (row.test.clone(), row.arch.name().to_owned(), row.line, row.variant)
}

/// The rows of one producer's run, and what made the run unusable.
fn run_one(
    producer: Producer,
    mortar: Option<std::path::PathBuf>,
    jobs: Option<usize>,
    filters: &[String],
    problems: &mut Vec<String>,
) -> BTreeMap<Key, DigestRow> {
    let root = workspace_root();
    let args = linktest::Args {
        linkers: vec![Linker::Mortar],
        arches: vec![Arch::Aarch64, Arch::X86_64],
        jobs,
        mortar,
        filters: filters.to_vec(),
        results: Some(linktest::build_dir().join(format!("producers-{}.tsv", producer.name()))),
        no_plumb: true,
        digest: true,
        producer,
        variants: VARIANTS.iter().map(|v| v.iter().map(|&o| o.to_owned()).collect()).collect(),
        paths: vec![root.join("tests/link/odr"), root.join("tests/link/dyn/odr")],
        ..linktest::Args::default()
    };
    let report = match linktest::run(&args) {
        Ok(report) => report,
        Err(e) => {
            problems.push(format!("{}: {e}", producer.name()));
            return BTreeMap::new();
        }
    };
    problems.extend(report.errors.iter().map(|e| format!("{}: {e}", producer.name())));
    problems.extend(report.missing.iter().map(|m| format!("{}: missing: {m}", producer.name())));
    problems.extend(report.records.iter().filter(|r| r.outcome == Outcome::Error).map(|r| {
        format!(
            "{}: {} [{}]: {}",
            producer.name(),
            r.test,
            r.arch.map_or("-", Arch::name),
            r.detail
        )
    }));
    report.digests.into_iter().map(|row| (key_of(&row), row)).collect()
}

/// The baseline's rows: key -> (gcc, hod, mixed, why).
fn parse_baseline(contents: &str) -> Result<BTreeMap<Key, [String; 4]>, String> {
    let mut rows = BTreeMap::new();
    for (n, line) in contents.lines().enumerate() {
        if line.starts_with('#') || line.trim().is_empty() {
            continue;
        }
        let f: Vec<&str> = line.split('\t').collect();
        let [test, arch, at, variant, gcc, hod, mixed, why] = f.as_slice() else {
            return Err(format!("{BASELINE}:{}: not 8 columns", n + 1));
        };
        let number = |s: &str| {
            s.parse::<usize>().map_err(|_| format!("{BASELINE}:{}: '{s}' is not a number", n + 1))
        };
        let variant = VARIANTS
            .iter()
            .position(|v| variant_name(v) == *variant)
            .ok_or_else(|| format!("{BASELINE}:{}: unknown variant '{variant}'", n + 1))?;
        rows.insert(
            ((*test).to_owned(), (*arch).to_owned(), number(at)?, variant),
            [(*gcc).to_owned(), (*hod).to_owned(), (*mixed).to_owned(), (*why).to_owned()],
        );
    }
    Ok(rows)
}

fn variant_name(variant: &[&str]) -> String {
    if variant.is_empty() { "-".to_owned() } else { variant.join(" ") }
}

type Runs = BTreeMap<Producer, BTreeMap<Key, DigestRow>>;

/// Each link's answers (GCC's objects, hod's, the mixed link), how many of
/// hod's and of the mixed answers are GCC's; the stop condition and an
/// output's `.mason.meta` are `problems`.
fn compare(runs: &Runs, problems: &mut Vec<String>) -> (BTreeMap<Key, [String; 3]>, [usize; 2]) {
    let keys: std::collections::BTreeSet<Key> =
        runs.values().flat_map(|r| r.keys().cloned()).collect();
    let not_run = "not run".to_owned();
    let mut now: BTreeMap<Key, [String; 3]> = BTreeMap::new();
    let mut same = [0usize; 2];
    for key in &keys {
        let get = |p: Producer| runs.get(&p).and_then(|r| r.get(key));
        let label = label(key);
        // The stop condition: the same container from two writers.
        match (get(Producer::Hod), get(Producer::HodObjcopy)) {
            (Some(h), Some(t)) if h.end == t.end && h.stderr_text == t.stderr_text => {}
            (Some(h), Some(t)) => problems.push(format!(
                "STOP: {label}: hod's container and the test producer's in hod's objects answer differently:\n  hod:         {}\n{}\n  hod-objcopy: {}\n{}",
                h.end, h.stderr_text, t.end, t.stderr_text
            )),
            (h, t) => problems.push(format!(
                "{label}: hod ran: {}, hod-objcopy ran: {}",
                h.is_some(),
                t.is_some()
            )),
        }
        for p in Producer::ALL {
            if let Some(row) = get(p)
                && row.meta_section
                && !row.keeps_meta
            {
                problems
                    .push(format!("{label}: {}: the output has a section .mason.meta", p.name()));
            }
        }
        let answers = [Producer::Gcc, Producer::Hod, Producer::Mixed]
            .map(|p| get(p).map_or_else(|| not_run.clone(), answer));
        if answers[1] == answers[0] {
            same[0] += 1;
        }
        if answers[2] == answers[0] {
            same[1] += 1;
        }
        now.insert(key.clone(), answers);
    }
    (now, same)
}

fn label(key: &Key) -> String {
    format!("{} [{}] line {} {}", key.0, key.1, key.2, variant_name(VARIANTS[key.3]))
}

/// The entry point.
///
/// # Errors
/// An unknown option.
pub fn main(args: &[String]) -> Result<ExitCode, String> {
    let mut update = false;
    let mut mortar = None;
    let mut jobs = None;
    let mut filters = Vec::new();
    let mut rest = args.iter();
    while let Some(arg) = rest.next() {
        match arg.as_str() {
            "--update" => update = true,
            "--mortar" => mortar = Some(rest.next().ok_or("--mortar needs a value")?.into()),
            "-j" => {
                let n = rest.next().ok_or("-j needs a value")?;
                jobs = Some(n.parse().map_err(|_| format!("-j {n}: not a number"))?);
            }
            "--filter" => filters.push(rest.next().ok_or("--filter needs a value")?.clone()),
            other => return Err(format!("unknown argument `{other}`")),
        }
    }
    if super::Mason::find_or_skip("producers")?.is_none() {
        return Ok(ExitCode::SUCCESS);
    }
    let mut problems = Vec::new();
    let runs: Runs = Producer::ALL
        .into_iter()
        .map(|p| (p, run_one(p, mortar.clone(), jobs, &filters, &mut problems)))
        .collect();
    let (now, same) = compare(&runs, &mut problems);
    let path = workspace_root().join(BASELINE);
    let baseline = match std::fs::read_to_string(&path) {
        Ok(text) => parse_baseline(&text)?,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => BTreeMap::new(),
        Err(e) => return Err(format!("{}: {e}", path.display())),
    };
    let mut text = String::from(HEADER);
    let mut changed = 0usize;
    for (key, answers) in &now {
        let why = baseline.get(key).map(|b| b[3].clone()).unwrap_or_default();
        let explained = answers[1] == answers[0] && answers[2] == answers[0] || !why.is_empty();
        let label = label(key);
        match baseline.get(key) {
            Some(b) if b[..3] == answers[..] => {}
            Some(b) => {
                changed += 1;
                if !update {
                    problems.push(format!(
                        "{label}: changed: was gcc '{}' hod '{}' mixed '{}', now gcc '{}' hod '{}' mixed '{}'",
                        b[0], b[1], b[2], answers[0], answers[1], answers[2]
                    ));
                }
            }
            None => {
                changed += 1;
                if !update {
                    problems.push(format!("{label}: not in {BASELINE}"));
                }
            }
        }
        if !explained {
            problems.push(format!(
                "{label}: differs without an explanation: gcc '{}' hod '{}' mixed '{}'",
                answers[0], answers[1], answers[2]
            ));
        }
        let _ = writeln!(
            text,
            "{}\t{}\t{}\t{}\t{}\t{}\t{}\t{why}",
            key.0,
            key.1,
            key.2,
            variant_name(VARIANTS[key.3]),
            answers[0],
            answers[1],
            answers[2]
        );
    }
    let gone = baseline.keys().filter(|k| !now.contains_key(*k)).count();
    if gone > 0 && !update && filters.is_empty() {
        problems.push(format!("{gone} rows of {BASELINE} were not run"));
    }
    if update && filters.is_empty() {
        std::fs::write(&path, &text).map_err(|e| format!("{}: {e}", path.display()))?;
    }
    say!(
        "xtask mason producers: {} links x {} producers; hod = gcc {}, mixed = gcc {}; {} changed, {} gone from {BASELINE}",
        now.len(),
        Producer::ALL.len(),
        same[0],
        same[1],
        changed,
        gone
    );
    for p in &problems {
        say!("xtask mason producers: {p}");
    }
    Ok(if problems.is_empty() { ExitCode::SUCCESS } else { ExitCode::FAILURE })
}
