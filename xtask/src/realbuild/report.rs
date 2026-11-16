//! The real-builds lane's files and report (`spec/harness.md` "realbuild",
//! "Results", "The ranking"): the per-row results a run leaves, the
//! checked-in baseline, tests and runs files, and `docs/status/realbuild.md`
//! with the outcome table and the ranking of what mortar refuses.

use std::collections::{BTreeMap, BTreeSet};
use std::fmt::Write as _;
use std::path::Path;

use super::outcome::{self, Outcome, OutputKind, Row};
use super::{BASELINE, Project, RUNS, RowResult, TESTS};

/// One line of `tests.tsv`: project, row, test, control's result, mortar's.
pub type TestRow = (String, String, String, String, String);
/// One line of `runs.tsv`: project, row, mode, wall s, peak KiB, lines,
/// built, commit.
pub type RunRow = (String, String, String, u64, u64, usize, bool, String);

pub const TESTS_HEADER: &str = "# mortar-realbuild-tests 1\tproject\trow\ttest\tcontrol\tmortar";
pub const RUNS_HEADER: &str =
    "# mortar-realbuild-runs 1\tproject\trow\tmode\twall_s\tpeak_kib\tlines\tbuilt\tcommit";

fn fields(text: &str, n: usize, what: &str) -> Result<Vec<Vec<String>>, String> {
    let mut out = Vec::new();
    for (at, line) in text.lines().enumerate() {
        if line.starts_with('#') || line.trim().is_empty() {
            continue;
        }
        let f: Vec<String> = line.split('\t').map(str::to_owned).collect();
        if f.len() != n {
            return Err(format!("{what}: line {}: {n} tab-separated fields expected", at + 1));
        }
        out.push(f);
    }
    Ok(out)
}

pub fn parse_tests(text: &str) -> Result<Vec<TestRow>, String> {
    Ok(fields(text, 5, TESTS)?
        .into_iter()
        .map(|f| (f[0].clone(), f[1].clone(), f[2].clone(), f[3].clone(), f[4].clone()))
        .collect())
}

pub fn parse_runs(text: &str) -> Result<Vec<RunRow>, String> {
    fields(text, 8, RUNS)?
        .into_iter()
        .map(|f| {
            let num =
                |s: &str| s.parse::<u64>().map_err(|_| format!("{RUNS}: `{s}` is not a number"));
            Ok((
                f[0].clone(),
                f[1].clone(),
                f[2].clone(),
                num(&f[3])?,
                num(&f[4])?,
                num(&f[5])? as usize,
                f[6] == "yes",
                f[7].clone(),
            ))
        })
        .collect()
}

/// Counts of a row's lines.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Counts {
    pub lines: usize,
    pub probes: usize,
    pub linked: usize,
    pub refused: usize,
    pub both_fail: usize,
    pub defects: usize,
    pub probe_linked: usize,
    pub probe_refused: usize,
}

pub fn count(rows: &[&Row]) -> Counts {
    let mut c = Counts::default();
    for r in rows {
        if outcome::is_probe(&r.line) {
            c.probes += 1;
            match r.outcome {
                Outcome::Linked => c.probe_linked += 1,
                Outcome::Refused(_) => c.probe_refused += 1,
                _ => {}
            }
            if r.outcome.is_defect() {
                c.defects += 1;
            }
            continue;
        }
        c.lines += 1;
        match &r.outcome {
            Outcome::Linked => c.linked += 1,
            Outcome::Refused(_) => c.refused += 1,
            Outcome::BothFail => c.both_fail += 1,
            o if o.is_defect() => c.defects += 1,
            _ => {}
        }
    }
    c
}

/// One sentence for the lane's progress.
pub fn summarise(r: &RowResult) -> String {
    let rows: Vec<&Row> = r.rows.iter().collect();
    let c = count(&rows);
    let pass = |pick: fn(&(String, String)) -> &String| {
        r.tests.values().filter(|t| pick(t) == "PASS").count()
    };
    let lost = r.tests.values().filter(|(c, m)| c == "PASS" && m != "PASS").count();
    format!(
        "{} lines ({} linked and clean, {} refused, {} neither links, {} defects) and {} probes ({} linked, {} refused); \
         tests: {} pass with the control, {} with mortar, {} lost",
        c.lines,
        c.linked,
        c.refused,
        c.both_fail,
        c.defects,
        c.probes,
        c.probe_linked,
        c.probe_refused,
        pass(|t| &t.0),
        pass(|t| &t.1),
        lost
    )
}

/// Writes a row's result beside the run's evidence: `<results>/<project>.<row>.tsv`.
pub fn write_row_result(results: &Path, r: &RowResult) -> Result<(), String> {
    let path = results.join(format!("{}.{}.tsv", r.project, r.row));
    let mut s = outcome::write_rows(&r.rows);
    for (name, (c, m)) in &r.tests {
        let _ = writeln!(s, "# test\t{name}\t{c}\t{m}");
    }
    std::fs::write(&path, s).map_err(|e| format!("{}: {e}", path.display()))
}

/// Replaces the rows of the (project, row) pairs of `done` in the
/// checked-in baseline, tests and runs files, keeping the others.
pub fn update_files(workspace: &Path, done: &[RowResult], commit: &str) -> Result<(), String> {
    let pairs: BTreeSet<(String, String)> =
        done.iter().map(|r| (r.project.clone(), r.row.clone())).collect();
    let read = |p: &str| std::fs::read_to_string(workspace.join(p)).unwrap_or_default();
    let keep = |p: &String, r: &String| !pairs.contains(&(p.clone(), r.clone()));
    let mut rows: Vec<Row> = outcome::parse_rows(&read(BASELINE))?
        .into_iter()
        .filter(|r| keep(&r.project, &r.row))
        .collect();
    rows.extend(done.iter().flat_map(|r| r.rows.iter().cloned()));
    rows.sort_by(|a, b| (&a.project, &a.row).cmp(&(&b.project, &b.row)));
    let mut tests: Vec<TestRow> =
        parse_tests(&read(TESTS))?.into_iter().filter(|t| keep(&t.0, &t.1)).collect();
    for r in done {
        for (n, (c, m)) in &r.tests {
            tests.push((r.project.clone(), r.row.clone(), n.clone(), c.clone(), m.clone()));
        }
    }
    tests.sort();
    let mut runs: Vec<RunRow> =
        parse_runs(&read(RUNS))?.into_iter().filter(|t| keep(&t.0, &t.1)).collect();
    for r in done {
        for (mode, (wall, peak, lines, built)) in &r.modes {
            runs.push((
                r.project.clone(),
                r.row.clone(),
                mode.clone(),
                *wall,
                *peak,
                *lines,
                *built,
                commit.to_owned(),
            ));
        }
    }
    runs.sort();
    let write = |p: &str, text: String| {
        std::fs::write(workspace.join(p), text).map_err(|e| format!("{p}: {e}"))
    };
    write(BASELINE, outcome::write_rows(&rows))?;
    let mut t = String::from(TESTS_HEADER);
    t.push('\n');
    for (p, r, n, c, m) in &tests {
        let _ = writeln!(t, "{p}\t{r}\t{}\t{c}\t{m}", n.replace('\t', " "));
    }
    write(TESTS, t)?;
    let mut t = String::from(RUNS_HEADER);
    t.push('\n');
    for (p, r, mode, wall, peak, lines, built, commit) in &runs {
        let _ = writeln!(
            t,
            "{p}\t{r}\t{mode}\t{wall}\t{peak}\t{lines}\t{}\t{commit}",
            if *built { "yes" } else { "no" }
        );
    }
    write(RUNS, t)
}

/// A refused construct's place in the ranking.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Rank {
    /// Lines (not probes) it is refused on.
    pub lines: usize,
    /// Probes it is refused on.
    pub probes: usize,
    /// project/row pairs whose lines it blocks.
    pub projects: BTreeSet<String>,
    /// Lines that would link if only it were supported: it is their only
    /// refusal.
    pub unlock: usize,
    /// The unlock count by project.
    pub unlock_by: BTreeMap<String, usize>,
}

/// The ranking: every refused name, most lines first.
pub fn rank(rows: &[Row]) -> Vec<(String, Rank)> {
    let mut by: BTreeMap<String, Rank> = BTreeMap::new();
    for r in rows {
        let Outcome::Refused(names) = &r.outcome else { continue };
        let probe = outcome::is_probe(&r.line);
        for n in names {
            let e = by.entry(n.clone()).or_default();
            if probe {
                e.probes += 1;
                continue;
            }
            e.lines += 1;
            e.projects.insert(r.project.clone());
            if names.len() == 1 {
                e.unlock += 1;
                *e.unlock_by.entry(r.project.clone()).or_default() += 1;
            }
        }
    }
    let mut out: Vec<(String, Rank)> = by.into_iter().collect();
    out.sort_by(|a, b| {
        (b.1.lines, b.1.projects.len(), b.1.probes)
            .cmp(&(a.1.lines, a.1.projects.len(), a.1.probes))
            .then(a.0.cmp(&b.0))
    });
    out
}

/// A set of refusals: the lines it is printed on, the projects.
pub type RefusalSet = (Vec<String>, (usize, BTreeSet<String>));

/// The lines (probes apart) by their set of refusals: lines and projects,
/// most lines first.
pub fn refusal_sets(rows: &[Row]) -> Vec<RefusalSet> {
    let mut by: BTreeMap<Vec<String>, (usize, BTreeSet<String>)> = BTreeMap::new();
    for r in rows {
        let Outcome::Refused(names) = &r.outcome else { continue };
        if outcome::is_probe(&r.line) {
            continue;
        }
        let mut set = names.clone();
        set.sort();
        let e = by.entry(set).or_default();
        e.0 += 1;
        e.1.insert(r.project.clone());
    }
    let mut out: Vec<_> = by.into_iter().collect();
    out.sort_by(|a, b| b.1.0.cmp(&a.1.0).then(a.0.cmp(&b.0)));
    out
}

fn cell(s: &str) -> String {
    s.replace('|', "\\|")
}

/// The report, `docs/status/realbuild.md`.
#[allow(clippy::too_many_lines, reason = "one document")]
pub fn render(projects: &[Project], rows: &[Row], tests: &[TestRow], runs: &[RunRow]) -> String {
    let mut s = String::new();
    s.push_str("# Real builds\n\nGenerated by `cargo xtask realbuild --report` from `tests/realbuild/manifest`, \
                `baseline.tsv`, `tests.tsv` and `runs.tsv`; do not edit. The lane is `spec/harness.md` \
                \"realbuild\"; the record that planned M4 from it is `spec/landed/harness/WP-100.md`.\n\n");
    let commits: BTreeSet<&String> = runs.iter().map(|r| &r.7).collect();
    let _ = writeln!(
        s,
        "mortar at {}. AArch64 host.\n",
        commits.into_iter().cloned().collect::<Vec<_>>().join(", ")
    );
    s.push_str("## Projects\n\n| project | version | licence | rows | why | tests |\n|---|---|---|---|---|---|\n");
    for p in projects {
        let _ = writeln!(
            s,
            "| {} | {} | {} | {} | {} | {} |",
            p.name,
            p.version,
            cell(&p.licence),
            p.rows.join(", "),
            cell(&p.why),
            cell(if p.tests.is_empty() { "all" } else { &p.tests })
        );
    }
    s.push_str("\nSources (fetched from the projects' public homes, checked by SHA-256, never committed):\n\n");
    for p in projects {
        let _ = writeln!(s, "- {}: <{}> `{}`", p.name, p.url, p.sha256);
        if let Some((u, h, _)) = &p.extra {
            let _ = writeln!(s, "  - and <{u}> `{h}`");
        }
    }
    s.push_str("\n## Outcomes\n\nPer project and row (GCC with GNU ld as the control; Clang with lld): the link lines \
                of the mortar build (configure probes apart), what became of them with mortar, and the project's \
                tests with each linker. *Linked* is linked by mortar, the verifier clean, the control linking it \
                too; *refused* is refused by name (the control linked it for the build); *neither* is a line \
                neither linker links; a *defect* is anything else (a wrong output, a crash, a wrong diagnostic, a \
                finding). *Lost* tests pass with the control and not with mortar.\n\n\
                | project | row | lines | linked | refused | neither | defects | probes (linked / refused) | tests | pass: control | pass: mortar | lost |\n\
                |---|---|---|---|---|---|---|---|---|---|---|---|\n");
    let mut pairs: Vec<(String, String)> = Vec::new();
    for r in rows {
        let k = (r.project.clone(), r.row.clone());
        if !pairs.contains(&k) {
            pairs.push(k);
        }
    }
    let mut total = Counts::default();
    let (mut tt, mut tc, mut tm, mut tl) = (0, 0, 0, 0);
    for (p, row) in &pairs {
        let rs: Vec<&Row> = rows.iter().filter(|r| &r.project == p && &r.row == row).collect();
        let c = count(&rs);
        let ts: Vec<&TestRow> = tests.iter().filter(|t| &t.0 == p && &t.1 == row).collect();
        let pc = ts.iter().filter(|t| t.3 == "PASS").count();
        let pm = ts.iter().filter(|t| t.4 == "PASS").count();
        let lost = ts.iter().filter(|t| t.3 == "PASS" && t.4 != "PASS").count();
        let _ = writeln!(
            s,
            "| {p} | {row} | {} | {} | {} | {} | {} | {} ({} / {}) | {} | {pc} | {pm} | {lost} |",
            c.lines,
            c.linked,
            c.refused,
            c.both_fail,
            c.defects,
            c.probes,
            c.probe_linked,
            c.probe_refused,
            ts.len()
        );
        total.lines += c.lines;
        total.linked += c.linked;
        total.refused += c.refused;
        total.both_fail += c.both_fail;
        total.defects += c.defects;
        total.probes += c.probes;
        total.probe_linked += c.probe_linked;
        total.probe_refused += c.probe_refused;
        tt += ts.len();
        tc += pc;
        tm += pm;
        tl += lost;
    }
    let _ = writeln!(
        s,
        "| **all** | | {} | {} | {} | {} | {} | {} ({} / {}) | {tt} | {tc} | {tm} | {tl} |",
        total.lines,
        total.linked,
        total.refused,
        total.both_fail,
        total.defects,
        total.probes,
        total.probe_linked,
        total.probe_refused
    );
    s.push_str("\n### By the kind of output\n\nAll rows' lines, probes apart.\n\n| kind | lines | linked | refused | neither | defects |\n|---|---|---|---|---|---|\n");
    for kind in OutputKind::ALL {
        let rs: Vec<&Row> =
            rows.iter().filter(|r| r.kind == kind && !outcome::is_probe(&r.line)).collect();
        if rs.is_empty() {
            continue;
        }
        let c = count(&rs);
        let _ = writeln!(
            s,
            "| {} | {} | {} | {} | {} | {} |",
            kind.name(),
            c.lines,
            c.linked,
            c.refused,
            c.both_fail,
            c.defects
        );
    }
    s.push_str("\n## The ranking\n\nEvery construct mortar refused, by the lines (probes apart) it is refused on. \
                *Unlock* counts the lines on which it is the only refusal mortar printed: the lines that would link \
                if only it were supported. mortar stops at the first phase that refuses (options before inputs), so \
                a line's later refusals are not seen and an unlock count is an upper bound.\n\n\
                | refused | lines | projects | unlock | unlock by project | probes |\n|---|---|---|---|---|---|\n");
    for (name, r) in rank(rows) {
        let by: Vec<String> = r.unlock_by.iter().map(|(p, n)| format!("{p} {n}")).collect();
        let _ = writeln!(
            s,
            "| `{}` | {} | {} | {} | {} | {} |",
            cell(&name),
            r.lines,
            r.projects.len(),
            r.unlock,
            by.join(", "),
            r.probes
        );
    }
    s.push_str("\n### Refused together\n\nThe lines (probes apart) by the whole set of refusals mortar printed \
                for each: the set a package must close for those lines to link.\n\n\
                | refusals | lines | projects |\n|---|---|---|\n");
    for (set, (n, ps)) in refusal_sets(rows) {
        let _ = writeln!(
            s,
            "| {} | {n} | {} |",
            set.iter().map(|x| format!("`{}`", cell(x))).collect::<Vec<_>>().join(", "),
            ps.into_iter().collect::<Vec<_>>().join(", ")
        );
    }
    let defects: Vec<&Row> = rows.iter().filter(|r| r.outcome.is_defect()).collect();
    s.push_str("\n## Defects\n\n");
    if defects.is_empty() {
        s.push_str("None.\n");
    }
    for d in defects {
        let _ = writeln!(
            s,
            "- {} {} `{}` ({}): {}",
            d.project,
            d.row,
            d.line,
            d.kind.name(),
            cell(&d.outcome.to_text())
        );
    }
    s.push_str("\n## Tests that differ\n\n");
    let differ: Vec<&TestRow> = tests.iter().filter(|t| t.3 != t.4).collect();
    if differ.is_empty() {
        s.push_str("None: every test has the same result with mortar as with the control.\n");
    }
    for t in differ {
        let _ = writeln!(s, "- {} {} `{}`: control {}, mortar {}", t.0, t.1, cell(&t.2), t.3, t.4);
    }
    s.push_str("\n## Runs\n\n| project | row | mode | wall s | peak disk MiB | lines | built |\n|---|---|---|---|---|---|---|\n");
    for r in runs {
        let _ = writeln!(
            s,
            "| {} | {} | {} | {} | {} | {} | {} |",
            r.0,
            r.1,
            r.2,
            r.3,
            r.4 / 1024,
            r.5,
            if r.6 { "yes" } else { "no" }
        );
    }
    let wall: u64 = runs.iter().map(|r| r.3).sum();
    let peak = runs.iter().map(|r| r.4).max().unwrap_or(0);
    let _ = writeln!(
        s,
        "\nThe builds' wall time summed: {} min; the largest build tree: {} MiB.",
        wall / 60,
        peak / 1024
    );
    s
}

// spec/harness.md "realbuild", "The ranking".
#[cfg(test)]
mod tests {
    use super::*;

    fn row(project: &str, line: &str, o: Outcome) -> Row {
        Row {
            project: project.into(),
            row: "gcc".into(),
            line: line.into(),
            kind: OutputKind::Pie,
            outcome: o,
        }
    }

    fn refused(names: &[&str]) -> Outcome {
        Outcome::Refused(names.iter().map(|n| (*n).to_owned()).collect())
    }

    #[test]
    fn the_ranking_counts_lines_projects_and_unlocks() {
        let rows = vec![
            row("a", "x.so", refused(&["option: -shared"])),
            row("a", "y.so", refused(&["option: -shared", "option: --version-script=<file>"])),
            row("b", "z.so", refused(&["option: -shared"])),
            row("b", "conftest", refused(&["option: -shared"])),
            row("b", "bin", Outcome::Linked),
        ];
        let r = rank(&rows);
        assert_eq!(r[0].0, "option: -shared");
        assert_eq!(
            (r[0].1.lines, r[0].1.projects.len(), r[0].1.unlock, r[0].1.probes),
            (3, 2, 2, 1)
        );
        assert_eq!(r[1].1.unlock, 0);
        let sets = refusal_sets(&rows);
        assert_eq!(sets[0].0, ["option: -shared"]);
        assert_eq!(sets[0].1.0, 2);
        assert_eq!(sets[1].1.0, 1);
        let c = count(&rows.iter().collect::<Vec<_>>());
        assert_eq!((c.lines, c.linked, c.refused, c.probes, c.probe_refused), (4, 1, 3, 1, 1));
    }

    #[test]
    fn tests_and_runs_round_trip_through_the_report() {
        let tests =
            parse_tests(&format!("{TESTS_HEADER}\nzlib\tgcc\tteststatic\tPASS\tPASS\n")).unwrap();
        let runs =
            parse_runs(&format!("{RUNS_HEADER}\nzlib\tgcc\tmortar\t12\t2048\t5\tyes\tabc\n"))
                .unwrap();
        let text = render(&[], &[row("zlib", "example", Outcome::Linked)], &tests, &runs);
        assert!(
            text.contains("| zlib | gcc | 1 | 1 | 0 | 0 | 0 | 0 (0 / 0) | 1 | 1 | 1 | 0 |"),
            "{text}"
        );
        assert!(parse_tests("zlib\tgcc\n").is_err());
    }
}
