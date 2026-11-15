//! The per-test result file of `cargo xtask mason libcxx` (WP-034,
//! `tests/mason/README.md` "Result file"), the comparison of two modes,
//! the link-time summary and the mortar baseline.
//!
//! One line per test and mode, tab-separated, after a `#` header:
//!
//! ```text
//! # test  mode  outcome  detail  link-ms  links  link-rss-kib  plumb  first-key
//! ```
//!
//! `outcome`, `detail` and `first-key` are kiln's runner's (`PASS`,
//! `FAIL`, `UNSUPPORTED`, ...); `link-ms` is the wall time of the test's
//! link commands (the lane's `ld` shims, `timed.sh`), summed, with three
//! decimals; `links` their number; `link-rss-kib` the largest peak RSS of
//! one of them; `plumb` the verifier's verdict on a mortar mode's output
//! (`clean`, `N findings: <first>`, ...). An empty field is `-`.

use std::collections::BTreeMap;
use std::fmt::Write as _;
use std::path::Path;

/// One test in one mode.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Rec {
    pub test: String,
    pub mode: String,
    pub outcome: String,
    pub detail: String,
    /// Microseconds of link wall time, summed over the test's links.
    pub link_us: Option<u64>,
    pub links: u32,
    pub rss_kib: Option<u64>,
    pub plumb: String,
    pub key: String,
}

pub const HEADER: &str =
    "# test\tmode\toutcome\tdetail\tlink-ms\tlinks\tlink-rss-kib\tplumb\tfirst-key";

fn field(s: &str) -> String {
    let s = s.trim();
    if s.is_empty() { "-".to_owned() } else { s.replace(['\t', '\n'], " ") }
}

/// The file's text.
#[allow(clippy::cast_precision_loss)] // microseconds: exact far beyond any link
pub fn render(recs: &[Rec]) -> String {
    let mut s = String::from(HEADER);
    s.push('\n');
    for r in recs {
        let ms = r.link_us.map_or_else(|| "-".to_owned(), |us| format!("{:.3}", us as f64 / 1e3));
        let rss = r.rss_kib.map_or_else(|| "-".to_owned(), |k| k.to_string());
        let _ = writeln!(
            s,
            "{}\t{}\t{}\t{}\t{ms}\t{}\t{rss}\t{}\t{}",
            field(&r.test),
            field(&r.mode),
            field(&r.outcome),
            field(&r.detail),
            r.links,
            field(&r.plumb),
            field(&r.key)
        );
    }
    s
}

/// Reads a result file.
pub fn parse(text: &str) -> Result<Vec<Rec>, String> {
    let mut out = Vec::new();
    for (n, l) in text.lines().enumerate() {
        if l.starts_with('#') || l.trim().is_empty() {
            continue;
        }
        let f: Vec<&str> = l.split('\t').collect();
        if f.len() != 9 {
            return Err(format!("line {}: {} fields, not 9", n + 1, f.len()));
        }
        let opt = |s: &str| (s != "-").then(|| s.to_owned());
        let ms = opt(f[4])
            .map(|s| s.parse::<f64>().map_err(|e| format!("line {}: link-ms: {e}", n + 1)))
            .transpose()?;
        out.push(Rec {
            test: f[0].into(),
            mode: f[1].into(),
            outcome: f[2].into(),
            detail: f[3].into(),
            link_us: ms.map(|m| (m * 1e3).round() as u64),
            links: f[5].parse().map_err(|e| format!("line {}: links: {e}", n + 1))?,
            rss_kib: opt(f[6])
                .map(|s| s.parse().map_err(|e| format!("line {}: rss: {e}", n + 1)))
                .transpose()?,
            plumb: f[7].into(),
            key: f[8].into(),
        });
    }
    Ok(out)
}

/// The records of one mode's run directory (`<out>/<mode>/`): kiln's
/// `results.tsv` (selection order: line `i` is the test of scratch
/// directory `t<i>`), the shims' `times.tsv`, mortar's `lane.log`.
pub fn collect(mode: &str, dir: &Path) -> Result<Vec<Rec>, String> {
    let read = |p: &Path| std::fs::read_to_string(p).map_err(|e| format!("{}: {e}", p.display()));
    let results = read(&dir.join("results.tsv"))?;
    let index =
        |cwd: &str| -> Option<usize> { cwd.rsplit('/').next()?.strip_prefix('t')?.parse().ok() };
    let mut times: BTreeMap<usize, (u64, u32, Option<u64>)> = BTreeMap::new();
    for l in read(&dir.join("times.tsv")).unwrap_or_default().lines() {
        let f: Vec<&str> = l.split('\t').collect();
        let (Some(i), Some(us)) = (f.first().and_then(|c| index(c)), f.get(3)) else { continue };
        let Ok(us) = us.parse::<u64>() else { continue };
        let rss = f.get(4).and_then(|r| r.parse::<u64>().ok());
        let e = times.entry(i).or_insert((0, 0, None));
        e.0 += us;
        e.1 += 1;
        e.2 = e.2.max(rss);
    }
    let mut plumbs: BTreeMap<usize, String> = BTreeMap::new();
    for l in read(&dir.join("lane.log")).unwrap_or_default().lines() {
        let f: Vec<&str> = l.split('\t').collect();
        if f.len() < 5 || f[1] != "mortar" {
            continue;
        }
        if let Some(i) = index(f[0]) {
            let p = plumbs.entry(i).or_default();
            // A test's worst verdict: a finding or a failure over `clean`.
            if p.is_empty() || p == "-" || p == "plumb: clean" {
                f[4].trim().clone_into(p);
            }
        }
    }
    let mut out = Vec::new();
    let lines = results.lines().filter(|l| !l.starts_with('#') && !l.trim().is_empty());
    for (i, l) in lines.enumerate() {
        let f: Vec<&str> = l.split('\t').collect();
        if f.len() < 4 {
            return Err(format!("{}: a line of {} fields", dir.display(), f.len()));
        }
        let t = times.get(&i);
        out.push(Rec {
            test: f[1].into(),
            mode: mode.into(),
            outcome: f[0].into(),
            detail: f[2].into(),
            link_us: t.map(|t| t.0),
            links: t.map_or(0, |t| t.1),
            rss_kib: t.and_then(|t| t.2),
            plumb: plumbs.get(&i).cloned().unwrap_or_else(|| "-".into()),
            key: f[3].into(),
        });
    }
    Ok(out)
}

/// The records of `mode` in `recs` (any mode when `None` and the file
/// holds one).
pub fn select<'a>(
    recs: &'a [Rec],
    mode: Option<&str>,
) -> Result<BTreeMap<&'a str, &'a Rec>, String> {
    let modes: std::collections::BTreeSet<&str> = recs.iter().map(|r| r.mode.as_str()).collect();
    let mode = match mode {
        Some(m) if modes.contains(m) => m,
        Some(m) => return Err(format!("no mode '{m}' (modes: {modes:?})")),
        None if modes.len() == 1 => modes.into_iter().next().unwrap_or_default(),
        None => return Err(format!("several modes ({modes:?}): name one as <file>:<mode>")),
    };
    Ok(recs.iter().filter(|r| r.mode == mode).map(|r| (r.test.as_str(), r)).collect())
}

/// The class of an outcome in the comparison.
fn class(outcome: &str) -> &'static str {
    match outcome {
        "PASS" => "pass",
        "UNSUPPORTED" => "unsupported",
        "XFAIL" => "xfail",
        _ => "fail",
    }
}

/// Why `b` differs from the reference `a` (a mortar mode against bfd).
pub fn cause(a: &Rec, b: &Rec) -> String {
    let refused = b.detail == "link" && b.key.contains("mortar: error:");
    if refused {
        return format!("refusal: {}", super::libcxx::refusal(&b.key));
    }
    if matches!(b.outcome.as_str(), "TIMEOUT" | "KILLED")
        || matches!(a.outcome.as_str(), "TIMEOUT" | "KILLED")
    {
        return format!("timeout/killed ({} / {})", a.outcome, b.outcome);
    }
    if a.outcome == "PASS" && b.detail == "run" {
        return "WRONG? linked, then failed to run (re-run to rule out a flake)".into();
    }
    format!("{} {} / {} {}", a.outcome, a.detail, b.outcome, b.detail)
}

/// The comparison of `b` against the reference `a`: counts by class and
/// the DIFFERENT tests.
pub fn compare(
    an: &str,
    a: &BTreeMap<&str, &Rec>,
    bn: &str,
    b: &BTreeMap<&str, &Rec>,
) -> (String, usize) {
    let mut s = String::new();
    let mut same: BTreeMap<&str, usize> = BTreeMap::new();
    let mut different = Vec::new();
    let mut only = 0;
    for (t, ra) in a {
        let Some(rb) = b.get(t) else {
            only += 1;
            continue;
        };
        if ra.outcome == rb.outcome {
            *same.entry(class(&ra.outcome)).or_insert(0) += 1;
        } else {
            different.push((*t, *ra, *rb));
        }
    }
    let only_b = b.keys().filter(|t| !a.contains_key(*t)).count();
    let ref_pass = a.values().filter(|r| r.outcome == "PASS").count();
    let b_pass_of = a
        .iter()
        .filter(|(t, r)| r.outcome == "PASS" && b.get(*t).is_some_and(|rb| rb.outcome == "PASS"))
        .count();
    let _ = writeln!(
        s,
        "{bn} passes {b_pass_of} of the {ref_pass} tests {an} passes; {} DIFFERENT",
        different.len()
    );
    let both = |k: &str| same.get(k).copied().unwrap_or(0);
    let _ = writeln!(
        s,
        "both pass {}, both fail {}, both xfail {}, both unsupported {}, DIFFERENT {}{}",
        both("pass"),
        both("fail"),
        both("xfail"),
        both("unsupported"),
        different.len(),
        if only + only_b > 0 {
            format!(" (only in {an}: {only}, only in {bn}: {only_b})")
        } else {
            String::new()
        }
    );
    if !different.is_empty() {
        let _ = writeln!(s, "\n| test | {an} | {bn} | cause |\n|---|---|---|---|");
        for (t, ra, rb) in &different {
            let _ = writeln!(
                s,
                "| {t} | {} {} | {} {} | {} |",
                ra.outcome,
                ra.detail,
                rb.outcome,
                rb.detail,
                cause(ra, rb).replace('|', "\\|")
            );
        }
    }
    (s, different.len())
}

/// The link-time summary of one mode: links, sum and median of the
/// per-test link wall time, the 90th percentile, and the peak RSS.
#[allow(clippy::cast_precision_loss)] // microseconds and counts: exact far beyond any run
pub fn times(name: &str, m: &BTreeMap<&str, &Rec>) -> String {
    let mut ms: Vec<u64> = m.values().filter_map(|r| r.link_us).collect();
    let mut rss: Vec<u64> = m.values().filter_map(|r| r.rss_kib).collect();
    ms.sort_unstable();
    rss.sort_unstable();
    let links: u32 = m.values().map(|r| r.links).sum();
    let pick = |v: &[u64], q: f64| {
        v.get(((v.len() as f64 - 1.0) * q).round() as usize).copied().unwrap_or(0)
    };
    format!(
        "| {name} | {} | {links} | {:.1} | {:.1} | {:.1} | {:.1} | {:.1} | {:.1} |",
        ms.len(),
        ms.iter().sum::<u64>() as f64 / 1e6,
        pick(&ms, 0.5) as f64 / 1e3,
        pick(&ms, 0.9) as f64 / 1e3,
        pick(&ms, 1.0) as f64 / 1e3,
        pick(&rss, 0.5) as f64 / 1024.0,
        pick(&rss, 1.0) as f64 / 1024.0,
    )
}

pub const TIMES_HEADER: &str = "| mode | tests linked | links | link wall s (sum) | median ms | p90 ms | max ms | median peak RSS MiB | max peak RSS MiB |\n|---|---|---|---|---|---|---|---|---|";

/// The baseline of a mortar mode: the tests that do not pass, are not
/// unsupported and are not expected to fail, with the reason.
pub fn baseline(m: &BTreeMap<&str, &Rec>) -> String {
    let mut s = String::from(
        "# The libc++ tests mortar mode does not pass (FAIL, TIMEOUT, XPASS, ...) with the reason\n\
         # (tests/mason/README.md \"Baseline\"). test<TAB>outcome<TAB>detail<TAB>reason\n",
    );
    for (t, r) in m {
        if class(&r.outcome) == "fail" {
            let _ = writeln!(s, "{t}\t{}\t{}\t{}", r.outcome, field(&r.detail), field(&r.key));
        }
    }
    s
}

/// Against a baseline: the tests that do not pass now and are not in it
/// (regressed), and the tests in it that pass now (fixed).
pub fn against_baseline(m: &BTreeMap<&str, &Rec>, base: &str) -> (Vec<String>, Vec<String>) {
    let listed: BTreeMap<&str, &str> = base
        .lines()
        .filter(|l| !l.starts_with('#') && !l.trim().is_empty())
        .filter_map(|l| l.split_once('\t'))
        .collect();
    let mut regressed = Vec::new();
    let mut fixed = Vec::new();
    for (t, r) in m {
        let bad = class(&r.outcome) == "fail";
        match (bad, listed.contains_key(t)) {
            (true, false) => regressed.push(format!("{t}: {} {} {}", r.outcome, r.detail, r.key)),
            (false, true) => fixed.push(format!("{t}: {}", r.outcome)),
            _ => {}
        }
    }
    (regressed, fixed)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rec(test: &str, mode: &str, outcome: &str, detail: &str, key: &str) -> Rec {
        Rec {
            test: test.into(),
            mode: mode.into(),
            outcome: outcome.into(),
            detail: detail.into(),
            link_us: Some(12_345),
            links: 1,
            rss_kib: Some(2048),
            plumb: "plumb: clean".into(),
            key: key.into(),
        }
    }

    #[test]
    fn result_file_round_trips() {
        let mut r = rec("std/a.pass.cpp", "mortar", "PASS", "", "");
        let text = render(std::slice::from_ref(&r));
        assert!(text.starts_with(HEADER));
        assert!(text.contains("std/a.pass.cpp\tmortar\tPASS\t-\t12.345\t1\t2048\tplumb: clean\t-"));
        let back = parse(&text).unwrap();
        r.detail = "-".into();
        r.key = "-".into();
        assert_eq!(back, [r]);
        assert!(parse("a\tb\n").is_err());
    }

    #[test]
    fn compare_counts_and_causes() {
        let a = [
            rec("t1", "bfd", "PASS", "-", "-"),
            rec("t2", "bfd", "PASS", "-", "-"),
            rec("t3", "bfd", "UNSUPPORTED", "x", "-"),
            rec("t4", "bfd", "PASS", "-", "-"),
        ];
        let b = [
            rec("t1", "mortar", "PASS", "-", "-"),
            rec("t2", "mortar", "FAIL", "link", "link: mortar: error: unsupported feature: x"),
            rec("t3", "mortar", "UNSUPPORTED", "x", "-"),
            rec("t4", "mortar", "FAIL", "run", "run: assertion failed"),
        ];
        let (am, bm) = (select(&a, None).unwrap(), select(&b, None).unwrap());
        let (text, n) = compare("bfd", &am, "mortar", &bm);
        assert_eq!(n, 2);
        assert!(
            text.starts_with("mortar passes 1 of the 3 tests bfd passes; 2 DIFFERENT"),
            "{text}"
        );
        assert!(text.contains("refusal: unsupported feature: x"), "{text}");
        assert!(text.contains("WRONG?"), "{text}");
        let base = baseline(&bm);
        assert_eq!(base.lines().filter(|l| !l.starts_with('#')).count(), 2);
        let (reg, fixed) = against_baseline(&am, &base);
        assert!(reg.is_empty());
        assert_eq!(fixed.len(), 2);
    }
}
