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
//! - kiln as the producer (WP-134, `spec/landed/meta/WP-134.md`): kiln's
//!   own containers, from the source (`kiln`: kiln's LLVM route;
//!   `kiln-hod`: its hod route), against GCC's objects with the test
//!   producer's rows. The rows describe; kiln writes what the source has:
//!   a test whose source has the violation its rows describe gives the
//!   same answer, one whose rows are synthetic gives none. Every test's
//!   class is the baseline's last column, and a kiln answer that differs
//!   from GCC's, or `kiln-hod` from `kiln`, without one fails. Where the
//!   kiln found writes no kind 3 and 5 rows (an older kiln), the kiln
//!   columns are not run and the baseline's are kept, with a note.
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

const HEADER: &str = "# The producer equality of WP-099 and WP-134 (`cargo xtask mason producers`, \
spec/landed/meta/WP-099.md, WP-134.md):\n\
# one mortar link step x variant per line: what GCC's objects, hod's objects, the mixed link,\n\
# kiln's objects with kiln's containers (LLVM route, hod route) answer (end; diagnostics;\n\
# meta-malformed warnings), then why hod's or the mixed answer differs from GCC's, then the\n\
# test's class for kiln (real: the source has the violation its rows describe; synthetic: the\n\
# rows describe one the source has not) and why kiln's answer differs.\n\
# Rewritten by `--update`, which keeps the last two columns.\n\
# test\tarch\tline\tvariant\tgcc\thod\tmixed\tkiln\tkiln-hod\twhy\tkiln-why\n";

type Key = (String, String, usize, usize);

/// The answers of a link: gcc, hod, mixed, kiln, kiln-hod.
type Answers = [String; 5];

/// The producers of [`Answers`], in its order.
const COLUMNS: [Producer; 5] =
    [Producer::Gcc, Producer::Hod, Producer::Mixed, Producer::Kiln, Producer::KilnHod];

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
        // The tests with kiln or hod steps compare producers themselves
        // (WP-134, WP-129).
        without: vec![linktest::target::Feature::Kiln, linktest::target::Feature::Hod],
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

/// A baseline row: the [`Answers`], then `why` and `kiln-why`.
type BaselineRow = ([String; 5], [String; 2]);

/// The baseline's rows by key.
fn parse_baseline(contents: &str) -> Result<BTreeMap<Key, BaselineRow>, String> {
    let mut rows = BTreeMap::new();
    for (n, line) in contents.lines().enumerate() {
        if line.starts_with('#') || line.trim().is_empty() {
            continue;
        }
        let f: Vec<&str> = line.split('\t').collect();
        let [test, arch, at, variant, gcc, hod, mixed, kiln, kiln_hod, why, kiln_why] =
            f.as_slice()
        else {
            return Err(format!("{BASELINE}:{}: not 11 columns", n + 1));
        };
        let number = |s: &str| {
            s.parse::<usize>().map_err(|_| format!("{BASELINE}:{}: '{s}' is not a number", n + 1))
        };
        let variant = VARIANTS
            .iter()
            .position(|v| variant_name(v) == *variant)
            .ok_or_else(|| format!("{BASELINE}:{}: unknown variant '{variant}'", n + 1))?;
        let own = |s: &&str| (*s).to_owned();
        rows.insert(
            ((*test).to_owned(), (*arch).to_owned(), number(at)?, variant),
            ([gcc, hod, mixed, kiln, kiln_hod].map(own), [why, kiln_why].map(own)),
        );
    }
    Ok(rows)
}

fn variant_name(variant: &[&str]) -> String {
    if variant.is_empty() { "-".to_owned() } else { variant.join(" ") }
}

type Runs = BTreeMap<Producer, BTreeMap<Key, DigestRow>>;

/// Each link's [`Answers`], how many of hod's, the mixed, kiln's and
/// kiln-hod's answers are GCC's; the stop condition and an output's
/// `.mason.meta` are `problems`.
fn compare(runs: &Runs, problems: &mut Vec<String>) -> (BTreeMap<Key, Answers>, [usize; 4]) {
    let keys: std::collections::BTreeSet<Key> =
        runs.values().flat_map(|r| r.keys().cloned()).collect();
    let not_run = "not run".to_owned();
    let mut now: BTreeMap<Key, Answers> = BTreeMap::new();
    let mut same = [0usize; 4];
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
        let answers = COLUMNS.map(|p| get(p).map_or_else(|| not_run.clone(), answer));
        for (i, s) in same.iter_mut().enumerate() {
            if answers[i + 1] == answers[0] {
                *s += 1;
            }
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
#[allow(clippy::too_many_lines)] // the run, then one pass over the rows
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
    // WP-134: the kiln columns need a kiln that writes records.
    let kiln = super::KilnRecords::find(&linktest::build_dir().join("producers-kiln-probe"));
    if let (Err(e), true) = (&kiln, super::required()) {
        problems.push(format!("kiln columns: {e} (MASON_REQUIRE_TOOLS=1)"));
    }
    let producers: Vec<Producer> =
        Producer::ALL.into_iter().filter(|p| !p.is_kiln() || kiln.is_ok()).collect();
    let runs: Runs = producers
        .iter()
        .map(|&p| (p, run_one(p, mortar.clone(), jobs, &filters, &mut problems)))
        .collect();
    let (mut now, same) = compare(&runs, &mut problems);
    let path = workspace_root().join(BASELINE);
    let baseline = match std::fs::read_to_string(&path) {
        Ok(text) => parse_baseline(&text)?,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => BTreeMap::new(),
        Err(e) => return Err(format!("{}: {e}", path.display())),
    };
    // Without kiln the kiln columns are the baseline's, neither compared
    // nor changed.
    if kiln.is_err() {
        for (key, answers) in &mut now {
            if let Some((b, _)) = baseline.get(key) {
                answers[3].clone_from(&b[3]);
                answers[4].clone_from(&b[4]);
            }
        }
    }
    let mut text = String::from(HEADER);
    let mut changed = 0usize;
    for (key, answers) in &now {
        let [why, kiln_why] = baseline.get(key).map(|b| b.1.clone()).unwrap_or_default();
        let hod_same = answers[1] == answers[0] && answers[2] == answers[0];
        let kiln_same = answers[3] == answers[0] && answers[4] == answers[3];
        let label = label(key);
        match baseline.get(key) {
            Some((b, _)) if b == answers => {}
            Some((b, _)) => {
                changed += 1;
                if !update {
                    let mut text = format!("{label}: changed:");
                    for (i, p) in COLUMNS.iter().enumerate() {
                        if b[i] != answers[i] {
                            let _ =
                                write!(text, " {} was '{}', now '{}';", p.name(), b[i], answers[i]);
                        }
                    }
                    problems.push(text);
                }
            }
            None => {
                changed += 1;
                if !update {
                    problems.push(format!("{label}: not in {BASELINE}"));
                }
            }
        }
        if !hod_same && why.is_empty() {
            problems.push(format!(
                "{label}: differs without an explanation: gcc '{}' hod '{}' mixed '{}'",
                answers[0], answers[1], answers[2]
            ));
        }
        if !kiln_same && kiln_why.is_empty() && kiln.is_ok() {
            problems.push(format!(
                "{label}: kiln differs without an explanation: gcc '{}' kiln '{}' kiln-hod '{}'",
                answers[0], answers[3], answers[4]
            ));
        }
        let _ = writeln!(
            text,
            "{}\t{}\t{}\t{}\t{}\t{why}\t{kiln_why}",
            key.0,
            key.1,
            key.2,
            variant_name(VARIANTS[key.3]),
            answers.join("\t"),
        );
    }
    let gone = baseline.keys().filter(|k| !now.contains_key(*k)).count();
    if gone > 0 && !update && filters.is_empty() {
        problems.push(format!("{gone} rows of {BASELINE} were not run"));
    }
    if update && filters.is_empty() {
        std::fs::write(&path, &text).map_err(|e| format!("{}: {e}", path.display()))?;
    }
    if let Err(e) = &kiln {
        say!("xtask mason producers: the kiln columns are not run: {e}");
    }
    say!(
        "xtask mason producers: {} links x {} producers; hod = gcc {}, mixed = gcc {}, kiln = gcc {}, kiln-hod = gcc {}; {} changed, {} gone from {BASELINE}",
        now.len(),
        producers.len(),
        same[0],
        same[1],
        same[2],
        same[3],
        changed,
        gone
    );
    for p in &problems {
        say!("xtask mason producers: {p}");
    }
    Ok(if problems.is_empty() { ExitCode::SUCCESS } else { ExitCode::FAILURE })
}
