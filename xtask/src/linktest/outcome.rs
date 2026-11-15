//! What a result means (`spec/harness.md` "Link tests": "Outcomes" and
//! "The self-check"). No process is started here: these are the rules that
//! turn what the linkers did into what is reported.

use super::target::Linker;

/// The outcome of a test for one linker on one architecture, or
/// ([`Outcome::Oracle`]) of a test on one architecture as a whole.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Outcome {
    Pass,
    Fail,
    Xfail,
    Xpass,
    Skip,
    Killed,
    Error,
    Oracle,
}

impl Outcome {
    /// Every outcome, in the order of the counts.
    pub const ALL: [Self; 8] = [
        Self::Pass,
        Self::Fail,
        Self::Xfail,
        Self::Xpass,
        Self::Skip,
        Self::Killed,
        Self::Error,
        Self::Oracle,
    ];

    /// The outcome in a report.
    pub fn name(self) -> &'static str {
        match self {
            Self::Pass => "PASS",
            Self::Fail => "FAIL",
            Self::Xfail => "XFAIL",
            Self::Xpass => "XPASS",
            Self::Skip => "SKIP",
            Self::Killed => "KILLED",
            Self::Error => "ERROR",
            Self::Oracle => "ORACLE",
        }
    }

    /// Whether a run with this outcome fails.
    pub fn fails(self) -> bool {
        !matches!(self, Self::Pass | Self::Xfail | Self::Skip)
    }

    /// Whether a reference linker with this outcome did what the test
    /// says: it passed.
    fn confirms(self) -> bool {
        matches!(self, Self::Pass | Self::Xpass)
    }
}

/// What a linker did with a test, before the test's markers are applied.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Raw {
    /// Every step did what the test says.
    Pass,
    /// A step did not; the text says which and how.
    Fail(String),
    /// A step ran into its time or memory limit.
    Killed(String),
    /// The harness could not do its work.
    Error(String),
    /// A step cannot be done here, by name (`check gdb` of a program that
    /// runs under qemu): the linker's result is `SKIP`, marker or not.
    Skip(String),
}

/// The outcome and the detail of a linker that did `raw`, with the reason
/// of the `xfail` that applies to it, if one does.
///
/// A limit is not a failure and trouble of the harness is not one either:
/// `xfail` hides neither.
pub fn marked(raw: &Raw, xfail: Option<&str>) -> (Outcome, String) {
    match (raw, xfail) {
        (Raw::Pass, None) => (Outcome::Pass, String::new()),
        (Raw::Pass, Some(reason)) => {
            (Outcome::Xpass, format!("passes: remove its xfail ({reason})"))
        }
        (Raw::Fail(detail), None) => (Outcome::Fail, detail.clone()),
        (Raw::Fail(detail), Some(reason)) => (Outcome::Xfail, format!("{reason}: {detail}")),
        (Raw::Killed(detail), _) => (Outcome::Killed, detail.clone()),
        (Raw::Error(detail), _) => (Outcome::Error, detail.clone()),
        (Raw::Skip(detail), _) => (Outcome::Skip, detail.clone()),
    }
}

/// What a program printed in a run step whose expected output is the
/// references'.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Printed {
    /// The line of the run step.
    pub line: usize,
    pub stdout: Vec<u8>,
}

/// A reference linker's part in the self-check of one test on one
/// architecture.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Reference {
    pub linker: Linker,
    /// `None`: a `skip` of the test left the linker out.
    pub outcome: Option<Outcome>,
    pub detail: String,
    /// What the test's programs printed, where the test leaves that to
    /// the references.
    pub printed: Vec<Printed>,
}

/// Whether the references establish a test on one architecture.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Verdict {
    /// They do: mortar is judged by the test.
    Established,
    /// No reference linker is in the run: mortar is judged by the test's
    /// own expectations, and the run says that nothing was proven.
    Unchecked,
    /// Reference linkers are in the run, but those the test is proven
    /// with are not among them: mortar is not judged.
    Unproven,
    /// They do not, and why: the test is `ORACLE`, and mortar is not
    /// judged.
    Refuted(String),
}

/// The self-check: the verdict of the references that are in the run, in
/// matrix order.
pub fn self_check(references: &[Reference]) -> Verdict {
    if references.is_empty() {
        return Verdict::Unchecked;
    }
    let ran = || references.iter().filter_map(|r| Some((r, r.outcome?)));
    let wrong: Vec<String> = ran()
        .filter(|(_, outcome)| {
            !outcome.confirms() && !matches!(outcome, Outcome::Xfail | Outcome::Skip)
        })
        .map(|(reference, outcome)| {
            format!("{} {}: {}", reference.linker.name(), outcome.name(), reference.detail)
        })
        .collect();
    if !wrong.is_empty() {
        return Verdict::Refuted(wrong.join("; "));
    }
    let mut confirming = ran().filter(|(_, outcome)| outcome.confirms()).map(|(r, _)| r);
    let Some(first) = confirming.next() else {
        return Verdict::Unproven;
    };
    for other in confirming {
        let differs = first
            .printed
            .iter()
            .zip(&other.printed)
            .find(|(one, two)| one != two)
            .map(|(one, two)| (one.line, &one.stdout, &two.stdout));
        if let Some((line, one, two)) = differs {
            return Verdict::Refuted(format!(
                "the references disagree on the output of run (line {line}): {} printed {}, {} \
                 printed {}",
                first.linker.name(),
                quoted(one),
                other.linker.name(),
                quoted(two)
            ));
        }
    }
    Verdict::Established
}

/// The start of an output, for a message.
pub fn quoted(output: &[u8]) -> String {
    const SHOWN: usize = 120;
    let text = String::from_utf8_lossy(output);
    let mut shown: String = text.chars().take(SHOWN).collect();
    if text.chars().count() > SHOWN {
        shown.push_str("...");
    }
    format!("{shown:?}")
}

#[cfg(test)]
mod tests {
    use super::*;

    // spec/harness.md "Link tests", "Outcomes": what a linker did and the
    // marker that applies to it give its outcome. A limit and trouble of
    // the harness are never hidden by `xfail`.
    #[test]
    fn a_marker_turns_a_result_into_an_outcome() {
        let fail = Raw::Fail("link (line 4): exit status 1".to_owned());
        let killed = Raw::Killed("run (line 5): killed: time".to_owned());
        let error = Raw::Error("cannot run 'nm'".to_owned());
        let xfail = Some("not implemented (M1)");
        let rows = [
            (&Raw::Pass, None, Outcome::Pass, ""),
            (&Raw::Pass, xfail, Outcome::Xpass, "passes: remove its xfail (not implemented (M1))"),
            (&fail, None, Outcome::Fail, "link (line 4): exit status 1"),
            (&fail, xfail, Outcome::Xfail, "not implemented (M1): link (line 4): exit status 1"),
            (&killed, None, Outcome::Killed, "run (line 5): killed: time"),
            (&killed, xfail, Outcome::Killed, "run (line 5): killed: time"),
            (&error, None, Outcome::Error, "cannot run 'nm'"),
            (&error, xfail, Outcome::Error, "cannot run 'nm'"),
        ];
        for (raw, marker, outcome, detail) in rows {
            assert_eq!(marked(raw, marker), (outcome, detail.to_owned()), "{raw:?} {marker:?}");
        }
    }

    // "Exit status": which outcomes fail a run.
    #[test]
    fn which_outcomes_fail_a_run() {
        let failing: Vec<&str> =
            Outcome::ALL.into_iter().filter(|o| o.fails()).map(Outcome::name).collect();
        assert_eq!(failing, ["FAIL", "XPASS", "KILLED", "ERROR", "ORACLE"]);
        let names: Vec<&str> = Outcome::ALL.into_iter().map(Outcome::name).collect();
        assert_eq!(names, ["PASS", "FAIL", "XFAIL", "XPASS", "SKIP", "KILLED", "ERROR", "ORACLE"]);
    }

    fn reference(linker: Linker, outcome: Option<Outcome>) -> Reference {
        Reference { linker, outcome, detail: "what happened".to_owned(), printed: Vec::new() }
    }

    fn printing(linker: Linker, text: &str) -> Reference {
        let printed = vec![Printed { line: 9, stdout: text.as_bytes().to_vec() }];
        Reference { printed, ..reference(linker, Some(Outcome::Pass)) }
    }

    // "The self-check": the outcomes of the references in the run give
    // the verdict. Each row: bfd, lld, mold (`-` is not in the run, `s`
    // is left out by a `skip`), and what the verdict is.
    #[test]
    fn the_references_establish_a_test_or_do_not() {
        use Outcome::{Error, Fail, Killed, Pass, Xfail, Xpass};
        #[derive(Clone, Copy)]
        enum Cell {
            Absent,
            Skipped,
            Is(Outcome),
        }
        use Cell::{Absent, Is, Skipped};
        let established = "established";
        let rows: [([Cell; 3], &str); 17] = [
            // All agree.
            ([Is(Pass), Is(Pass), Is(Pass)], established),
            // A reference may differ when the test says so, as long as
            // one confirms the test.
            ([Is(Pass), Is(Xfail), Is(Pass)], established),
            ([Is(Xfail), Is(Xfail), Is(Pass)], established),
            ([Is(Pass), Skipped, Skipped], established),
            // A stale marker fails the run by itself; the linker still
            // confirms the test.
            ([Is(Xpass), Is(Pass), Is(Pass)], established),
            ([Is(Xpass), Is(Xfail), Skipped], established),
            // A reference that fails without a marker refutes the test.
            ([Is(Fail), Is(Pass), Is(Pass)], "bfd FAIL: what happened"),
            ([Is(Pass), Is(Pass), Is(Fail)], "mold FAIL: what happened"),
            (
                [Is(Fail), Is(Fail), Is(Fail)],
                "bfd FAIL: what happened; lld FAIL: what happened; mold FAIL: what happened",
            ),
            // A limit or trouble of the harness is not a pass, with or
            // without a marker: nothing is established.
            ([Is(Pass), Is(Killed), Is(Pass)], "lld KILLED: what happened"),
            ([Is(Error), Is(Pass), Is(Pass)], "bfd ERROR: what happened"),
            ([Is(Xfail), Is(Fail), Is(Pass)], "lld FAIL: what happened"),
            // Part of the references: those in the run decide.
            ([Is(Pass), Absent, Absent], established),
            ([Absent, Is(Fail), Absent], "lld FAIL: what happened"),
            // The references in the run do not include one the test is
            // proven with.
            ([Is(Xfail), Absent, Absent], "unproven"),
            ([Skipped, Is(Xfail), Absent], "unproven"),
            // No reference in the run.
            ([Absent, Absent, Absent], "unchecked"),
        ];
        for (cells, want) in rows {
            let references: Vec<Reference> = Linker::REFERENCES
                .into_iter()
                .zip(cells)
                .filter_map(|(linker, cell)| match cell {
                    Absent => None,
                    Skipped => Some(reference(linker, None)),
                    Is(outcome) => Some(reference(linker, Some(outcome))),
                })
                .collect();
            let verdict = match self_check(&references) {
                Verdict::Established => established.to_owned(),
                Verdict::Unchecked => "unchecked".to_owned(),
                Verdict::Unproven => "unproven".to_owned(),
                Verdict::Refuted(why) => why,
            };
            let shown: Vec<_> = references.iter().map(|r| (r.linker, r.outcome)).collect();
            assert_eq!(verdict, want, "{shown:?}");
        }
    }

    // "The self-check": the references that pass must print the same
    // where the test leaves the output to them; one that is expected to
    // fail is not asked.
    #[test]
    fn references_that_disagree_on_output_refute_a_test() {
        let same = [
            printing(Linker::Bfd, "a\n"),
            printing(Linker::Lld, "a\n"),
            printing(Linker::Mold, "a\n"),
        ];
        assert_eq!(self_check(&same), Verdict::Established);

        let differ = [
            printing(Linker::Bfd, "a\n"),
            printing(Linker::Lld, "a\n"),
            printing(Linker::Mold, "b\n"),
        ];
        assert_eq!(
            self_check(&differ),
            Verdict::Refuted(
                "the references disagree on the output of run (line 9): bfd printed \"a\\n\", \
                 mold printed \"b\\n\""
                    .to_owned()
            )
        );

        let mut excused = printing(Linker::Mold, "b\n");
        excused.outcome = Some(Outcome::Xfail);
        let two = [printing(Linker::Bfd, "a\n"), printing(Linker::Lld, "a\n"), excused];
        assert_eq!(self_check(&two), Verdict::Established);
        assert_eq!(quoted(&[b'x'; 200]).len(), 120 + 3 + 2);
    }

    // "Checks": a check that cannot run on the architecture (a debugger
    // under qemu) is SKIP by name, and neither refutes nor proves a test.
    #[test]
    fn a_skipped_check_does_not_refute() {
        let skip = Raw::Skip("check (line 9): gdb runs only programs of the host here".to_owned());
        let (outcome, detail) = marked(&skip, Some("M2: WP-053"));
        assert_eq!(outcome, Outcome::Skip);
        assert!(detail.starts_with("check (line 9)"), "{detail}");
        let reference = |linker, outcome| Reference {
            linker,
            outcome: Some(outcome),
            detail: String::new(),
            printed: Vec::new(),
        };
        let skipped = [
            reference(Linker::Bfd, Outcome::Skip),
            reference(Linker::Lld, Outcome::Skip),
            reference(Linker::Mold, Outcome::Skip),
        ];
        assert_eq!(self_check(&skipped), Verdict::Unproven);
        let one = [reference(Linker::Bfd, Outcome::Pass), reference(Linker::Lld, Outcome::Skip)];
        assert_eq!(self_check(&one), Verdict::Established);
    }
}
