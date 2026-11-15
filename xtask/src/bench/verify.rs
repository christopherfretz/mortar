//! The verifier on what the bench times (`spec/harness.md` "bench",
//! "Verification", WP-035): before a workload's measured runs, mortar
//! links it once more, untimed, with `--mortar-map`, and `mortar_plumb`
//! judges that output. A finding fails mortar's rows of the workload; the
//! golden `output_hash` is recorded only from an output the verifier
//! judged clean, and only when the timed links wrote the same bytes.

use std::collections::BTreeMap;
use std::fmt::Write as _;
use std::path::{Path, PathBuf};

use super::measure::{LINK_LIMITS, Linker, Run, status_of};
use super::prepare::Prepared;
use super::record::{Fnv, Link, Status};
use super::run::Ran;
use crate::process;

/// How many findings a failed row shows.
pub const SHOWN: usize = 3;

/// What the verifier said of mortar's output of one workload.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Verdict {
    /// Judged, no finding: the output's hash.
    Clean { hash: String },
    /// Judged wrong: how many findings, the count per rule id (in id
    /// order), the first [`SHOWN`] findings, and where the output and the
    /// map were kept.
    Findings { count: usize, rules: Vec<(String, usize)>, first: Vec<String>, kept: PathBuf },
    /// Linked, and outside what the verifier judges (`spec/plumb.md` §1;
    /// the reasons of `linktest::engine::not_judged`).
    NotJudged { reason: &'static str },
    /// mortar refused the link: the first line it said.
    Refused(String),
    /// The link failed otherwise, was killed, or its map cannot be read.
    Failed(String),
}

impl Verdict {
    /// The value of the results header's `verify.<workload>`.
    pub fn text(&self) -> String {
        match self {
            Verdict::Clean { .. } => "clean".to_owned(),
            Verdict::Findings { count, rules, .. } => {
                format!("findings {count}: {}", rule_list(rules))
            }
            Verdict::NotJudged { reason } => format!("not judged: {reason}"),
            Verdict::Refused(detail) => format!("refused: {detail}"),
            Verdict::Failed(detail) => format!("failed: {detail}"),
        }
    }

    /// The detail of mortar's rows when the verifier finds the output
    /// wrong: the rule ids and the first findings.
    pub fn row_detail(&self) -> Option<String> {
        let Verdict::Findings { count, rules, first, kept } = self else { return None };
        Some(format!(
            "the verifier finds {count} ({}): {} (kept: {})",
            rule_list(rules),
            first.join("; "),
            kept.display()
        ))
    }
}

/// `R1 2, S5 1`.
fn rule_list(rules: &[(String, usize)]) -> String {
    rules.iter().map(|(r, n)| format!("{r} {n}")).collect::<Vec<_>>().join(", ")
}

/// Links `prepared` with `mortar` once, untimed (`@<response> -o <out>
/// --mortar-map=<map>` in the recorded directory, at mortar's default
/// threads), and has the verifier judge the output. The inputs are the
/// files the map names (relative ones from the recorded directory);
/// `--fix-cortex-a53-843419` is what the response file says of it, else
/// inferred (`spec/plumb.md` §5.10). The output and map are removed
/// afterwards unless the verifier found something.
///
/// # Errors
/// The child cannot be started.
pub fn verify(
    prepared: &Prepared,
    mortar: &Linker,
    scratch: &Path,
    sources: &Path,
) -> Result<(Verdict, Option<Ran>), String> {
    let name = &prepared.workload.name;
    let out = scratch.join(format!("{name}-mortar-verify.out"));
    let map = scratch.join(format!("{name}-mortar-verify.map"));
    let remove = || {
        let _ = std::fs::remove_file(&out);
        let _ = std::fs::remove_file(&map);
    };
    remove();
    let output = process::run(
        &mortar.path,
        |c| {
            c.arg(format!("@{}", prepared.response.display())).arg("-o").arg(&out);
            c.arg(format!("--mortar-map={}", map.display()));
            c.current_dir(&prepared.cwd).env("LC_ALL", "C");
        },
        b"",
        LINK_LIMITS,
    )?;
    let output_bytes = std::fs::metadata(&out).ok().map(|m| m.len()).filter(|n| *n > 0);
    let run = Run { output, output_bytes, output_hash: None };
    let verdict = match status_of(&run, mortar) {
        (Status::Ok, _) => judge(prepared, &out, &map),
        (Status::Refused, detail) => Verdict::Refused(detail),
        (_, detail) => Verdict::Failed(detail),
    };
    // The program runs from an output the verifier did not find wrong
    // ("Running the outputs").
    let ran = match &verdict {
        Verdict::Clean { .. } | Verdict::NotJudged { .. } => {
            match super::run::run(prepared, &out, sources, scratch) {
                Ok(Ran::Differs(how)) => {
                    Some(Ran::Differs(format!("{how} (kept: {})", out.display())))
                }
                Ok(ran) => Some(ran),
                Err(e) => {
                    remove();
                    return Err(e);
                }
            }
        }
        _ => None,
    };
    if !matches!(verdict, Verdict::Findings { .. }) && !matches!(ran, Some(Ran::Differs(_))) {
        remove();
    }
    Ok((verdict, ran))
}

/// The verifier on a link that wrote `out` and `map`.
fn judge(prepared: &Prepared, out: &Path, map: &Path) -> Verdict {
    let bytes = match std::fs::read(out) {
        Ok(b) => b,
        Err(e) => return Verdict::Failed(format!("{}: {e}", out.display())),
    };
    if let Some(reason) = crate::linktest::engine::not_judged(&bytes) {
        return Verdict::NotJudged { reason };
    }
    let mut h = Fnv::default();
    h.update(&bytes);
    drop(bytes);
    let fix = std::fs::read(&prepared.response)
        .ok()
        .and_then(|t| crate::plumb::response_fix_843419(&String::from_utf8_lossy(&t)));
    // The verifier runs as a child (`xtask plumb`), not in this process:
    // what it allocates must not stay in the process that then starts the
    // measured links. Measured in-process, mortar's medians on
    // hello-c-static and hello-cxx-static rose by 5-10% (WP-035).
    let exe = match std::env::current_exe() {
        Ok(e) => e,
        Err(e) => return Verdict::Failed(format!("cannot find xtask itself: {e}")),
    };
    let ran = process::run(
        &exe,
        |c| {
            c.arg("plumb").arg(out).arg("--map").arg(map).args(["--map-format", "mortar"]);
            match fix {
                Some(true) => c.arg("--fix-cortex-a53-843419"),
                Some(false) => c.arg("--no-fix-cortex-a53-843419"),
                None => c,
            };
            c.current_dir(&prepared.cwd);
        },
        b"",
        VERIFY_LIMITS,
    );
    match ran {
        Err(e) => Verdict::Failed(format!("the verifier: {e}")),
        Ok(o) if o.end.success() => Verdict::Clean { hash: h.text() },
        Ok(o) if o.end == process::End::Exit(1) => {
            read_findings(&String::from_utf8_lossy(&o.stdout), out.to_path_buf())
        }
        Ok(o) => Verdict::Failed(format!("the verifier: {}: {}", o.end, o.first_error_line())),
    }
}

/// The limits of the verifier's process.
const VERIFY_LIMITS: process::Limits =
    process::Limits { time: std::time::Duration::from_mins(10), memory_kib: 16 << 20 };

/// The findings `xtask plumb` printed: one `... verifier rule <id>:
/// <message>` line each (`spec/plumb.md` §7.3), then the summary with
/// `findings: <n>`.
fn read_findings(stdout: &str, kept: PathBuf) -> Verdict {
    let mut rules: BTreeMap<String, usize> = BTreeMap::new();
    let mut first = Vec::new();
    let mut lines = 0;
    for l in stdout.lines() {
        let Some((_, f)) = l.split_once("verifier rule ") else { continue };
        let Some((rule, message)) = f.split_once(": ") else { continue };
        lines += 1;
        *rules.entry(rule.to_owned()).or_default() += 1;
        if first.len() < SHOWN {
            first.push(format!("{rule} {message}"));
        }
    }
    let count = stdout
        .lines()
        .rev()
        .find_map(|l| l.strip_prefix("findings: ")?.trim().parse().ok())
        .unwrap_or(lines);
    Verdict::Findings { count, rules: rules.into_iter().collect(), first, kept }
}

/// Applies `verdict` and `ran` to the workload's measured `links`: on a
/// finding, or a program that runs differently from GNU ld's, mortar's
/// rows are `failed` with what was found and lose their figures.
/// The golden hash to record: the verified output's, when the timed
/// warm-up's (`timed`) is the same; a different one fails mortar's rows
/// too (the bytes timed are not the bytes verified).
pub fn apply(
    verdict: &Verdict,
    ran: Option<&Ran>,
    links: &mut [Link],
    timed: Option<&str>,
) -> Option<String> {
    let fail = |links: &mut [Link], detail: &str| {
        for l in links.iter_mut().filter(|l| l.linker == super::stats::JUDGED) {
            l.status = Status::Failed;
            l.figures = None;
            l.runs = 0;
            detail.clone_into(&mut l.detail);
        }
    };
    if let Some(detail) = verdict.row_detail().or_else(|| ran.and_then(Ran::row_detail)) {
        fail(links, &detail);
        return None;
    }
    let Verdict::Clean { hash } = verdict else { return None };
    match timed {
        Some(t) if t == hash => Some(hash.clone()),
        Some(t) => {
            fail(links, &format!("the timed output ({t}) is not the verified output ({hash})"));
            None
        }
        None => None,
    }
}

/// The summary a person reads after the table, from the results header
/// (`verify`, `verify.<workload>`): which workloads were verified, which
/// mortar refused (by name), which the verifier cannot judge yet, and the
/// findings. `None` when mortar was not measured.
pub fn summary(results: &super::record::Results) -> Option<String> {
    if results.get("verify") == Some("off") {
        return Some(
            "\nverifier: OFF (--no-verify): mortar's outputs were not verified, no output hash recorded\n"
                .to_owned(),
        );
    }
    let verdicts: Vec<(&str, &str)> = results
        .header
        .iter()
        .filter_map(|(k, v)| Some((k.strip_prefix("verify.")?, v.as_str())))
        .collect();
    if verdicts.is_empty() {
        return None;
    }
    let mut groups: BTreeMap<u8, (&str, Vec<String>)> = BTreeMap::new();
    for (w, v) in &verdicts {
        let (order, title, item) = if *v == "clean" {
            (0, "verified clean", (*w).to_owned())
        } else if v.starts_with("findings") {
            (1, "FINDINGS", format!("{w} ({v})"))
        } else if let Some(r) = v.strip_prefix("not judged: ") {
            (2, "linked, not judged by the verifier yet", format!("{w} ({r})"))
        } else if let Some(r) = v.strip_prefix("refused: ") {
            (3, "refused by mortar", format!("{w} (`{r}`)"))
        } else {
            (4, "FAILED to link for the verifier", format!("{w} ({v})"))
        };
        groups.entry(order).or_insert((title, Vec::new())).1.push(item);
    }
    let mut s = format!("\nverifier on mortar's outputs ({} workloads):\n", verdicts.len());
    for (title, items) in groups.values() {
        let _ = writeln!(s, "  {title} {}: {}", items.len(), items.join(", "));
    }
    let runs: Vec<(&str, &str)> = results
        .header
        .iter()
        .filter_map(|(k, v)| Some((k.strip_prefix("run.")?, v.as_str())))
        .collect();
    if !runs.is_empty() {
        let mut groups: BTreeMap<u8, (&str, Vec<String>)> = BTreeMap::new();
        for (w, v) in &runs {
            let (order, title, item) = if v.starts_with("same") {
                (0, "the same as GNU ld's", (*w).to_owned())
            } else if let Some(how) = v.strip_prefix("differs: ") {
                (1, "DIFFERENT from GNU ld's", format!("{w} ({how})"))
            } else {
                (2, "not run", format!("{w} ({})", v.strip_prefix("not run: ").unwrap_or(v)))
            };
            groups.entry(order).or_insert((title, Vec::new())).1.push(item);
        }
        let _ =
            writeln!(s, "mortar's programs run once against GNU ld's ({} workloads):", runs.len());
        for (title, items) in groups.values() {
            let _ = writeln!(s, "  {title} {}: {}", items.len(), items.join(", "));
        }
    }
    Some(s)
}

/// The workloads whose mortar output the verifier found wrong, or whose
/// program ran differently from GNU ld's, in this run.
pub fn found(results: &super::record::Results) -> Vec<String> {
    results
        .header
        .iter()
        .filter_map(|(k, v)| {
            if let Some(w) = k.strip_prefix("verify.") {
                return v.starts_with("findings").then(|| format!("{w}: {v}"));
            }
            let w = k.strip_prefix("run.")?;
            v.starts_with("differs").then(|| format!("{w}: runs {v}"))
        })
        .collect()
}

// spec/harness.md "bench", "Verification".
#[cfg(test)]
mod tests {
    use super::*;
    use crate::bench::record::{Figures, Results, Threads};

    fn link(linker: &str, threads: Threads) -> Link {
        Link {
            status: Status::Ok,
            workload: "w".to_owned(),
            linker: linker.to_owned(),
            threads,
            runs: 5,
            figures: Some(Figures {
                wall_us_min: 1,
                wall_us: 2,
                wall_us_max: 3,
                cpu_us: None,
                rss_kib: None,
                output_bytes: 9,
            }),
            detail: String::new(),
        }
    }

    fn links() -> Vec<Link> {
        vec![
            link("lld", Threads::Default),
            link("mortar", Threads::Default),
            link("mortar", Threads::One),
        ]
    }

    fn findings() -> Verdict {
        Verdict::Findings {
            count: 3,
            rules: vec![("C1".to_owned(), 1), ("R1".to_owned(), 2)],
            first: vec!["R1 a field at 0x10".to_owned(), "R1 b".to_owned()],
            kept: PathBuf::from("/s/w-mortar-verify.out"),
        }
    }

    // "Verification": a finding fails mortar's rows (never a reference's),
    // with the rule ids and the first findings, and no hash is recorded.
    #[test]
    fn a_finding_fails_mortars_rows() {
        let mut l = links();
        assert_eq!(apply(&findings(), None, &mut l, Some("fnv1a64:1")), None);
        assert_eq!(l[0].status, Status::Ok);
        for m in &l[1..] {
            assert_eq!((m.status, m.runs, m.figures), (Status::Failed, 0, None));
            assert_eq!(
                m.detail,
                "the verifier finds 3 (C1 1, R1 2): R1 a field at 0x10; R1 b \
                 (kept: /s/w-mortar-verify.out)"
            );
        }
        assert_eq!(findings().text(), "findings 3: C1 1, R1 2");
    }

    // "Verification": the golden hash only from a verified output, and
    // only when the timed output is the same bytes.
    #[test]
    fn the_golden_hash_is_the_verified_outputs() {
        let clean = Verdict::Clean { hash: "fnv1a64:1".to_owned() };
        let mut l = links();
        assert_eq!(apply(&clean, None, &mut l, Some("fnv1a64:1")), Some("fnv1a64:1".to_owned()));
        assert!(l.iter().all(|l| l.status == Status::Ok));
        assert_eq!(apply(&clean, None, &mut l, None), None);
        let mut l = links();
        assert_eq!(apply(&clean, None, &mut l, Some("fnv1a64:2")), None);
        assert_eq!(l[1].status, Status::Failed);
        assert!(l[1].detail.contains("is not the verified output"));
        // "Running the outputs": a program that runs differently fails
        // mortar's rows as a finding does; the same run changes nothing.
        let mut l = links();
        let differs =
            Ran::Differs("standard output differs from byte 0 (1 bytes, GNU ld's 2)".into());
        assert_eq!(apply(&clean, Some(&differs), &mut l, Some("fnv1a64:1")), None);
        assert_eq!(
            (l[0].status, l[1].status, l[2].status),
            (Status::Ok, Status::Failed, Status::Failed)
        );
        assert!(
            l[2].detail.starts_with("mortar's output runs differently from GNU ld's: standard")
        );
        let mut l = links();
        let same = Ran::Same { end: process::End::Exit(0), stdout: 3 };
        assert!(apply(&clean, Some(&same), &mut l, Some("fnv1a64:1")).is_some());
        assert!(l.iter().all(|l| l.status == Status::Ok));
        for v in [
            Verdict::NotJudged { reason: "pie-or-shared" },
            Verdict::Refused("mortar: error: x".to_owned()),
            Verdict::Failed("killed: time".to_owned()),
        ] {
            let mut l = links();
            assert_eq!(apply(&v, None, &mut l, Some("fnv1a64:1")), None, "{v:?}");
            assert!(l.iter().all(|l| l.status == Status::Ok));
        }
    }

    // "Verification": what `xtask plumb` prints, read back.
    #[test]
    fn findings_are_read_from_xtask_plumb() {
        let out = "mortar-plumb: error: /s/o: verifier rule R1: a field: expected 0x1, found 0x2 at 0x10\n\
                   mortar-plumb: error: /s/o: verifier rule C1: an unfixed site at 0x20\n\
                   mortar-plumb: error: /s/o: verifier rule R1: b at 0x30\n\
                   mortar-plumb: error: /s/o: verifier rule R1: c\n\
                   relocation records in placed sections: 9\nfindings: 4\n";
        assert_eq!(
            read_findings(out, PathBuf::from("/s/o")),
            Verdict::Findings {
                count: 4,
                rules: vec![("C1".to_owned(), 1), ("R1".to_owned(), 3)],
                first: vec![
                    "R1 a field: expected 0x1, found 0x2 at 0x10".to_owned(),
                    "C1 an unfixed site at 0x20".to_owned(),
                    "R1 b at 0x30".to_owned(),
                ],
                kept: PathBuf::from("/s/o"),
            }
        );
    }

    // "Verification": the summary names every workload under its verdict.
    #[test]
    fn the_summary_names_every_workload() {
        let mut r = Results::default();
        assert_eq!(summary(&r), None);
        r.set("verify", "on");
        r.set("verify.a", Verdict::Clean { hash: String::new() }.text());
        r.set(
            "verify.b",
            Verdict::Refused("mortar: error: unsupported option: -pie".into()).text(),
        );
        r.set("verify.c", Verdict::NotJudged { reason: "pie-or-shared" }.text());
        r.set("verify.d", findings().text());
        r.set("verify.e", Verdict::Clean { hash: String::new() }.text());
        assert_eq!(
            summary(&r).unwrap(),
            "\nverifier on mortar's outputs (5 workloads):\n  \
             verified clean 2: a, e\n  \
             FINDINGS 1: d (findings 3: C1 1, R1 2)\n  \
             linked, not judged by the verifier yet 1: c (pie-or-shared)\n  \
             refused by mortar 1: b (`mortar: error: unsupported option: -pie`)\n"
        );
        assert_eq!(found(&r), ["d: findings 3: C1 1, R1 2"]);
        r.set("run.a", Ran::Same { end: process::End::Exit(0), stdout: 13 }.text());
        r.set(
            "run.e",
            Ran::Differs("exit status 1 where GNU ld's ended with exit status 0".into()).text(),
        );
        r.set("run.c", Ran::NotRun("no `run` in the manifest".into()).text());
        let text = summary(&r).unwrap();
        assert!(
            text.ends_with(
                "mortar's programs run once against GNU ld's (3 workloads):\n  \
                 the same as GNU ld's 1: a\n  \
                 DIFFERENT from GNU ld's 1: e (exit status 1 where GNU ld's ended with exit status 0)\n  \
                 not run 1: c (no `run` in the manifest)\n"
            ),
            "{text}"
        );
        assert_eq!(
            found(&r),
            [
                "d: findings 3: C1 1, R1 2",
                "e: runs differs: exit status 1 where GNU ld's ended with exit status 0"
            ]
        );
        r.set("verify", "off");
        assert!(summary(&r).unwrap().contains("OFF (--no-verify)"));
    }
}
