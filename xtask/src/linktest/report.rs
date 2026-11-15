//! The results of a link-test run (`spec/harness.md` "Link tests":
//! "Results file", "Summary", "Exit status").

use std::fmt::Write as _;
use std::path::{Path, PathBuf};
use std::time::Duration;

use super::outcome::Outcome;
use super::target::{Arch, Linker};

/// One result: a test for one linker on one architecture, or (without a
/// linker) a test on one architecture as a whole, or (without either) a
/// test that could not be read.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Record {
    pub test: String,
    pub arch: Option<Arch>,
    pub linker: Option<Linker>,
    pub outcome: Outcome,
    /// What failed, why it was skipped, the marker's reason; may be empty.
    pub detail: String,
    /// The test's `spec` directives, joined with `; `.
    pub spec: String,
}

/// What a run did.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Report {
    /// What ran: the architectures, the linkers, the jobs.
    pub configuration: String,
    pub notes: Vec<String>,
    /// The tools that are missing although `MORTAR_REQUIRE_TOOLS=1`.
    pub missing: Vec<String>,
    /// What kept the run from doing its work (mortar could not be built).
    pub errors: Vec<String>,
    /// How many test files were selected.
    pub tests: usize,
    /// The architectures and the linkers of the run, in matrix order.
    pub arches: Vec<Arch>,
    pub linkers: Vec<Linker>,
    /// In order of test name, then architecture, then linker, with a
    /// result that has no linker after those that have one.
    pub records: Vec<Record>,
    pub elapsed: Duration,
    pub results_file: Option<PathBuf>,
    /// The work directory, when `--keep` kept some of it.
    pub kept: Option<PathBuf>,
}

/// A field of the results file: `\`, tab, newline and carriage return
/// escaped, and `-` for an empty value.
fn field(value: &str) -> String {
    if value.is_empty() {
        return "-".to_owned();
    }
    let mut escaped = String::with_capacity(value.len());
    for c in value.chars() {
        match c {
            '\\' => escaped.push_str("\\\\"),
            '\t' => escaped.push_str("\\t"),
            '\n' => escaped.push_str("\\n"),
            '\r' => escaped.push_str("\\r"),
            other => escaped.push(other),
        }
    }
    escaped
}

/// The most of a detail a summary line shows.
const SHOWN_DETAIL: usize = 300;

impl Report {
    /// How many records have `outcome`.
    pub fn count(&self, outcome: Outcome) -> usize {
        self.records.iter().filter(|record| record.outcome == outcome).count()
    }

    /// The process exit status of the run: 0 when nothing failed; 1 when
    /// a linker failed, passed against its marker or ran into a limit; 3
    /// when none did but a test is wrong (`ERROR`, `ORACLE`), a required
    /// tool is missing or the run could not do its work.
    ///
    /// The records of the reference linkers on an architecture where the
    /// test is `ORACLE` are that wrong test, not a linker's failure: they
    /// count as the `ORACLE` does, except `KILLED`, which nothing hides.
    pub fn exit_status(&self) -> u8 {
        let refuted = |record: &Record| {
            record.linker.is_some_and(Linker::is_reference)
                && self.records.iter().any(|other| {
                    other.outcome == Outcome::Oracle
                        && other.test == record.test
                        && other.arch == record.arch
                })
        };
        let failed = self.records.iter().any(|record| match record.outcome {
            Outcome::Killed => true,
            Outcome::Fail | Outcome::Xpass => !refuted(record),
            _ => false,
        });
        let wrong = self.records.iter().any(|record| record.outcome.fails());
        if failed {
            1
        } else if wrong || !self.missing.is_empty() || !self.errors.is_empty() {
            3
        } else {
            0
        }
    }

    /// The results file: one line per record, six fields separated by
    /// tabs.
    pub fn results_text(&self) -> String {
        let mut text = String::new();
        for record in &self.records {
            let fields = [
                record.outcome.name().to_owned(),
                field(&record.test),
                record.arch.map_or("-", Arch::name).to_owned(),
                record.linker.map_or("-", Linker::name).to_owned(),
                field(&record.detail),
                field(&record.spec),
            ];
            text.push_str(&fields.join("\t"));
            text.push('\n');
        }
        text
    }

    /// Writes the results file, replacing it.
    pub fn write_results(&self, path: &Path) -> Result<(), String> {
        let cannot = |e: std::io::Error| format!("cannot write '{}': {e}", path.display());
        if let Some(parent) = path.parent().filter(|parent| !parent.as_os_str().is_empty()) {
            std::fs::create_dir_all(parent).map_err(cannot)?;
        }
        std::fs::write(path, self.results_text()).map_err(cannot)
    }

    /// The lines for the results that are listed: every one that is not
    /// a `PASS` (every one with `verbose`). Results of one test with the
    /// same outcome and text share a line.
    fn listed(&self, verbose: bool) -> Vec<String> {
        // Outcome, test, text; who it is for.
        type Group<'a> = ((Outcome, &'a str, &'a str), Vec<(Option<Arch>, Option<Linker>)>);
        let mut groups: Vec<Group> = Vec::new();
        for record in &self.records {
            if record.outcome == Outcome::Pass && !verbose {
                continue;
            }
            let key = (record.outcome, record.test.as_str(), record.detail.as_str());
            let who = (record.arch, record.linker);
            match groups.iter_mut().find(|(group, _)| *group == key) {
                Some((_, members)) => members.push(who),
                None => groups.push((key, vec![who])),
            }
        }
        groups
            .iter()
            .map(|((outcome, test, detail), members)| {
                let mut line = format!("{:<7} {test}", outcome.name());
                let who = who(members);
                if !who.is_empty() {
                    let _ = write!(line, " [{who}]");
                }
                if !detail.is_empty() {
                    let shown: String = detail.chars().take(SHOWN_DETAIL).collect();
                    let more = if detail.chars().count() > SHOWN_DETAIL { "..." } else { "" };
                    let _ = write!(line, ": {}{more}", shown.replace('\n', " "));
                }
                line
            })
            .collect()
    }

    /// The table of counts: a row per linker (and one, `-`, for the
    /// results without a linker, if there are any), a column per
    /// architecture.
    fn table(&self) -> Vec<String> {
        let cell = |linker: Option<Linker>, arch: Arch| {
            let counts: Vec<String> = Outcome::ALL
                .into_iter()
                .filter_map(|outcome| {
                    let count = self
                        .records
                        .iter()
                        .filter(|r| r.linker == linker && r.arch == Some(arch))
                        .filter(|r| r.outcome == outcome)
                        .count();
                    (count > 0).then(|| format!("{count} {}", outcome.name()))
                })
                .collect();
            if counts.is_empty() { "-".to_owned() } else { counts.join(", ") }
        };
        let mut rows: Vec<Vec<String>> = Vec::new();
        let mut header = vec![String::new()];
        header.extend(self.arches.iter().map(|arch| arch.name().to_owned()));
        rows.push(header);
        let mut linkers: Vec<Option<Linker>> = self.linkers.iter().copied().map(Some).collect();
        if self.records.iter().any(|r| r.linker.is_none() && r.arch.is_some()) {
            linkers.push(None);
        }
        for linker in linkers {
            let mut row = vec![linker.map_or("-", Linker::name).to_owned()];
            row.extend(self.arches.iter().map(|arch| cell(linker, *arch)));
            rows.push(row);
        }
        let columns = self.arches.len() + 1;
        let widths: Vec<usize> = (0..columns)
            .map(|column| rows.iter().map(|row| row[column].len()).max().unwrap_or(0))
            .collect();
        rows.iter()
            .map(|row| {
                let mut line = String::from("linktest:  ");
                for (text, width) in row.iter().zip(&widths) {
                    let _ = write!(line, " {text:<width$} ");
                }
                line.trim_end().to_owned()
            })
            .collect()
    }

    /// The summary, as it is printed on standard output.
    pub fn summary(&self, verbose: bool) -> String {
        let mut text = String::new();
        let _ = writeln!(text, "linktest: {}", self.configuration);
        for note in &self.notes {
            let _ = writeln!(text, "linktest: note: {note}");
        }
        for missing in &self.missing {
            let _ = writeln!(text, "linktest: error: {missing} (MORTAR_REQUIRE_TOOLS=1)");
        }
        for error in &self.errors {
            let _ = writeln!(text, "linktest: error: {error}");
        }
        for line in self.listed(verbose) {
            let _ = writeln!(text, "{line}");
        }
        if !self.arches.is_empty() && !self.records.is_empty() {
            for line in self.table() {
                let _ = writeln!(text, "{line}");
            }
        }
        let counts: Vec<String> = Outcome::ALL
            .into_iter()
            .map(|outcome| format!("{} {}", self.count(outcome), outcome.name()))
            .collect();
        let _ = writeln!(
            text,
            "linktest: {} tests, {} results: {} in {:.1}s",
            self.tests,
            self.records.len(),
            counts.join(", "),
            self.elapsed.as_secs_f64()
        );
        if let Some(results) = &self.results_file {
            let _ = writeln!(text, "linktest: results: {}", results.display());
        }
        if let Some(kept) = &self.kept {
            let _ = writeln!(text, "linktest: kept: {}", kept.display());
        }
        text
    }
}

/// Who a line is for: `mortar: aarch64 x86_64`, with the linkers that have
/// the same architectures sharing them (`bfd lld: i386; mold: aarch64`);
/// only architectures for results without a linker; nothing for a result
/// without either.
fn who(members: &[(Option<Arch>, Option<Linker>)]) -> String {
    let mut by_linker: Vec<(Option<Linker>, Vec<Arch>)> = Vec::new();
    for (arch, linker) in members {
        let Some(arch) = arch else { continue };
        match by_linker.iter_mut().find(|(known, _)| known == linker) {
            Some((_, arches)) => arches.push(*arch),
            None => by_linker.push((*linker, vec![*arch])),
        }
    }
    let mut by_arches: Vec<(Vec<Arch>, Vec<Option<Linker>>)> = Vec::new();
    for (linker, arches) in by_linker {
        match by_arches.iter_mut().find(|(known, _)| *known == arches) {
            Some((_, linkers)) => linkers.push(linker),
            None => by_arches.push((arches, vec![linker])),
        }
    }
    let parts: Vec<String> = by_arches
        .iter()
        .map(|(arches, linkers)| {
            let arches: Vec<&str> = arches.iter().map(|arch| arch.name()).collect();
            let linkers: Vec<&str> = linkers.iter().flatten().map(|linker| linker.name()).collect();
            if linkers.is_empty() {
                arches.join(" ")
            } else {
                format!("{}: {}", linkers.join(" "), arches.join(" "))
            }
        })
        .collect();
    parts.join("; ")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn record(
        test: &str,
        arch: Option<Arch>,
        linker: Option<Linker>,
        outcome: Outcome,
        detail: &str,
    ) -> Record {
        let spec = "resolve.md \"Weak\"".to_owned();
        Record { test: test.to_owned(), arch, linker, outcome, detail: detail.to_owned(), spec }
    }

    fn report(records: Vec<Record>) -> Report {
        Report {
            configuration: "what ran".to_owned(),
            tests: 3,
            arches: vec![Arch::Aarch64, Arch::I386],
            linkers: vec![Linker::Bfd, Linker::Mortar],
            records,
            elapsed: Duration::from_millis(1234),
            ..Report::default()
        }
    }

    // spec/harness.md "Link tests", "Exit status": 1 for a linker's
    // failure, 3 for a wrong test or a missing required tool alone, 0
    // otherwise.
    #[test]
    fn the_exit_status_of_a_run() {
        use Outcome::{Error, Fail, Killed, Oracle, Pass, Skip, Xfail, Xpass};
        let status = |outcomes: &[Outcome], missing: bool| {
            let records = outcomes
                .iter()
                .map(|outcome| record("t", Some(Arch::Aarch64), Some(Linker::Bfd), *outcome, ""))
                .collect();
            let mut report = report(records);
            if missing {
                report.missing.push("mold not found".to_owned());
            }
            report.exit_status()
        };
        assert_eq!(status(&[], false), 0);
        assert_eq!(status(&[Pass, Xfail, Skip], false), 0);
        assert_eq!(status(&[Pass, Fail], false), 1);
        assert_eq!(status(&[Xpass], false), 1);
        assert_eq!(status(&[Killed], false), 1);
        assert_eq!(status(&[Error], false), 3);
        assert_eq!(status(&[Oracle], false), 3);
        assert_eq!(status(&[Error, Fail], true), 1);
        assert_eq!(status(&[Pass], true), 3);
        // The references' records of a wrong test are that wrong test.
        let a64 = Some(Arch::Aarch64);
        let refuted = report(vec![
            record("t", a64, Some(Linker::Bfd), Fail, ""),
            record("t", a64, Some(Linker::Mold), Xpass, ""),
            record("t", a64, None, Oracle, ""),
        ]);
        assert_eq!(refuted.exit_status(), 3);
        let killed = report(vec![
            record("t", a64, Some(Linker::Bfd), Killed, ""),
            record("t", a64, None, Oracle, ""),
        ]);
        assert_eq!(killed.exit_status(), 1);
        let elsewhere = report(vec![
            record("t", a64, Some(Linker::Bfd), Fail, ""),
            record("t", Some(Arch::I386), None, Oracle, ""),
        ]);
        assert_eq!(elsewhere.exit_status(), 1);
        let mortar = report(vec![
            record("t", a64, Some(Linker::Mortar), Fail, ""),
            record("u", a64, None, Oracle, ""),
        ]);
        assert_eq!(mortar.exit_status(), 1);
        let mut broken = report(Vec::new());
        broken.errors.push("cannot build mortar".to_owned());
        assert_eq!(broken.exit_status(), 3);
    }

    // "Results file": six fields per record, escaped, `-` for an empty
    // one.
    #[test]
    fn the_results_file() {
        let report = report(vec![
            record("a.test", Some(Arch::Aarch64), Some(Linker::Bfd), Outcome::Pass, ""),
            record("a.test", Some(Arch::I386), None, Outcome::Oracle, "tab\there\nback\\slash\r"),
            Record { spec: String::new(), ..record("b.test", None, None, Outcome::Error, "x") },
        ]);
        assert_eq!(
            report.results_text(),
            "PASS\ta.test\taarch64\tbfd\t-\tresolve.md \"Weak\"\n\
             ORACLE\ta.test\ti386\t-\ttab\\there\\nback\\\\slash\\r\tresolve.md \"Weak\"\n\
             ERROR\tb.test\t-\t-\tx\t-\n"
        );
    }

    // "Summary": what ran, the notes, a line per result that is not a
    // PASS with the results of a test that read the same on one line,
    // the counts per linker and architecture, the totals.
    #[test]
    fn the_summary() {
        let a64 = Some(Arch::Aarch64);
        let i386 = Some(Arch::I386);
        let (bfd, mortar) = (Some(Linker::Bfd), Some(Linker::Mortar));
        let mut report = report(vec![
            record("a.test", a64, bfd, Outcome::Pass, ""),
            record("a.test", a64, mortar, Outcome::Xfail, "not implemented: link failed"),
            record("a.test", i386, bfd, Outcome::Pass, ""),
            record("a.test", i386, mortar, Outcome::Xfail, "not implemented: link failed"),
            record("b.test", a64, bfd, Outcome::Fail, "link (line 3): exit status 1"),
            record("b.test", a64, None, Outcome::Oracle, "bfd FAIL: link (line 3)"),
            record("b.test", i386, None, Outcome::Skip, "requires gfortran"),
            record("c.test", None, None, Outcome::Error, "line 1: no 'spec' directive"),
        ]);
        report.notes.push("x86_64 is not run".to_owned());
        report.missing.push("mold not found".to_owned());
        report.results_file = Some(PathBuf::from("/build/linktest/results.tsv"));
        report.kept = Some(PathBuf::from("/build/linktest/run-1"));
        let expected = "\
linktest: what ran
linktest: note: x86_64 is not run
linktest: error: mold not found (MORTAR_REQUIRE_TOOLS=1)
XFAIL   a.test [mortar: aarch64 i386]: not implemented: link failed
FAIL    b.test [bfd: aarch64]: link (line 3): exit status 1
ORACLE  b.test [aarch64]: bfd FAIL: link (line 3)
SKIP    b.test [i386]: requires gfortran
ERROR   c.test: line 1: no 'spec' directive
linktest:           aarch64         i386
linktest:   bfd     1 PASS, 1 FAIL  1 PASS
linktest:   mortar  1 XFAIL         1 XFAIL
linktest:   -       1 ORACLE        1 SKIP
linktest: 3 tests, 8 results: 2 PASS, 1 FAIL, 2 XFAIL, 0 XPASS, 1 SKIP, 0 KILLED, 1 ERROR, \
1 ORACLE in 1.2s
linktest: results: /build/linktest/results.tsv
linktest: kept: /build/linktest/run-1
";
        assert_eq!(report.summary(false), expected);
        // With `verbose` the passes are listed too.
        let verbose = report.summary(true);
        assert!(verbose.contains("PASS    a.test [bfd: aarch64 i386]\n"), "{verbose}");
    }

    // "Summary": linkers with the same architectures share them on a
    // line; a long detail is cut.
    #[test]
    fn who_a_line_is_for() {
        let a64 = Some(Arch::Aarch64);
        let i386 = Some(Arch::I386);
        let (bfd, lld, mold) = (Some(Linker::Bfd), Some(Linker::Lld), Some(Linker::Mold));
        assert_eq!(
            who(&[(a64, bfd), (a64, lld), (i386, bfd), (i386, lld)]),
            "bfd lld: aarch64 i386"
        );
        assert_eq!(
            who(&[(a64, bfd), (a64, mold), (i386, mold)]),
            "bfd: aarch64; mold: aarch64 i386"
        );
        assert_eq!(who(&[(a64, None), (i386, None)]), "aarch64 i386");
        assert_eq!(who(&[(None, None)]), "");
        let long = "x".repeat(400);
        let report = report(vec![record("a.test", a64, bfd, Outcome::Fail, &long)]);
        let line = report.listed(false).remove(0);
        assert!(line.ends_with(&format!("{}...", "x".repeat(300))), "{line}");
    }
}
