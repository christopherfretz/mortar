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
pub fn verify(prepared: &Prepared, mortar: &Linker, scratch: &Path) -> Result<Verdict, String> {
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
    if !matches!(verdict, Verdict::Findings { .. }) {
        remove();
    }
    Ok(verdict)
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
    let verified = crate::plumb::verify_link_with(
        out,
        map,
        mortar_plumb::Format::Mortar,
        &prepared.cwd,
        None,
        fix,
    );
    let report = match verified {
        Ok((_, report, _)) => report,
        Err(e) => return Verdict::Failed(format!("the verifier cannot read the link: {e}")),
    };
    if report.clean() {
        return Verdict::Clean { hash: h.text() };
    }
    let mut rules: BTreeMap<String, usize> = BTreeMap::new();
    for f in &report.findings {
        *rules.entry(f.rule.id()).or_default() += 1;
    }
    let first = report
        .findings
        .iter()
        .take(SHOWN)
        .map(|f| {
            let at = f.addr.map(|a| format!(" at {a:#x}")).unwrap_or_default();
            format!("{} {}{at}", f.rule.id(), f.message)
        })
        .collect();
    Verdict::Findings {
        count: report.findings.len(),
        rules: rules.into_iter().collect(),
        first,
        kept: out.to_path_buf(),
    }
}

/// Applies `verdict` to the workload's measured `links`: on a finding,
/// mortar's rows are `failed` with the findings and lose their figures.
/// The golden hash to record: the verified output's, when the timed
/// warm-up's (`timed`) is the same; a different one fails mortar's rows
/// too (the bytes timed are not the bytes verified).
pub fn apply(verdict: &Verdict, links: &mut [Link], timed: Option<&str>) -> Option<String> {
    let fail = |links: &mut [Link], detail: &str| {
        for l in links.iter_mut().filter(|l| l.linker == super::stats::JUDGED) {
            l.status = Status::Failed;
            l.figures = None;
            l.runs = 0;
            detail.clone_into(&mut l.detail);
        }
    };
    if let Some(detail) = verdict.row_detail() {
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
    Some(s)
}

/// Whether the verifier found a mortar output wrong in this run.
pub fn found(results: &super::record::Results) -> Vec<String> {
    results
        .header
        .iter()
        .filter_map(|(k, v)| {
            let w = k.strip_prefix("verify.")?;
            v.starts_with("findings").then(|| format!("{w}: {v}"))
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
        assert_eq!(apply(&findings(), &mut l, Some("fnv1a64:1")), None);
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
        assert_eq!(apply(&clean, &mut l, Some("fnv1a64:1")), Some("fnv1a64:1".to_owned()));
        assert!(l.iter().all(|l| l.status == Status::Ok));
        assert_eq!(apply(&clean, &mut l, None), None);
        let mut l = links();
        assert_eq!(apply(&clean, &mut l, Some("fnv1a64:2")), None);
        assert_eq!(l[1].status, Status::Failed);
        assert!(l[1].detail.contains("is not the verified output"));
        for v in [
            Verdict::NotJudged { reason: "pie-or-shared" },
            Verdict::Refused("mortar: error: x".to_owned()),
            Verdict::Failed("killed: time".to_owned()),
        ] {
            let mut l = links();
            assert_eq!(apply(&v, &mut l, Some("fnv1a64:1")), None, "{v:?}");
            assert!(l.iter().all(|l| l.status == Status::Ok));
        }
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
        r.set("verify", "off");
        assert!(summary(&r).unwrap().contains("OFF (--no-verify)"));
    }
}
