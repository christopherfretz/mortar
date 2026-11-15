//! What a person reads (`spec/harness.md` "bench", "Report"): the table a
//! run prints, and `docs/status/bench.md`, generated from a baseline.

use std::fmt::Write as _;

use super::manifest::{Kind, Workload};
use super::record::{Link, Results, Status, Threads};
use super::stats::{Check, JUDGED};

/// Where the status page is, in the checkout.
pub const STATUS_PAGE: &str = "docs/status/bench.md";

/// Where the performance page is: mortar against the fastest linker.
pub const PERF_PAGE: &str = "docs/status/perf.md";

/// The configuration the ratios are taken against: mold at its default
/// threads, else lld at its default.
pub fn reference<'a>(results: &'a Results, workload: &str) -> Option<&'a Link> {
    ["mold", "lld"].iter().find_map(|name| {
        results.link(workload, name, Threads::Default).filter(|l| l.status == Status::Ok)
    })
}

/// `value / unit` rounded to `places` decimals (0 to 2), in integers.
fn fixed(value: u64, unit: u64, places: u32) -> String {
    let scale = 10u128.pow(places);
    let unit = u128::from(unit.max(1));
    let scaled = (u128::from(value) * scale + unit / 2) / unit;
    match places {
        0 => scaled.to_string(),
        _ => format!("{}.{:0width$}", scaled / scale, scaled % scale, width = places as usize),
    }
}

/// Milliseconds of microseconds: one decimal below 100 ms.
pub fn ms(us: u64) -> String {
    fixed(us, 1000, u32::from(us < 100_000))
}

fn mib(kib: u64) -> String {
    fixed(kib, 1024, 1)
}

fn mib_of_bytes(bytes: u64) -> String {
    fixed(bytes, 1 << 20, 1)
}

fn ratio(now: u64, base: u64) -> String {
    if base == 0 { "-".to_owned() } else { fixed(now, base, 2) }
}

/// One row's cells: linker, threads, wall ms, range ms, CPU ms, RSS MiB,
/// × time, × memory, output bytes; a link that is not `ok` has its status
/// and detail instead of figures.
fn cells(link: &Link, reference: Option<&Link>) -> Vec<String> {
    let mut row = vec![link.linker.clone(), link.threads.text().to_owned()];
    if let Some(f) = link.figures {
        let base = reference.and_then(|r| r.figures);
        row.push(ms(f.wall_us));
        row.push(format!("{}–{}", ms(f.wall_us_min), ms(f.wall_us_max)));
        row.push(f.cpu_us.map_or_else(|| "-".to_owned(), ms));
        row.push(f.rss_kib.map_or_else(|| "-".to_owned(), |r| mib(r[1])));
        row.push(base.map_or_else(|| "-".to_owned(), |b| ratio(f.wall_us, b.wall_us)));
        let memory = f.rss_kib.zip(base.and_then(|b| b.rss_kib));
        row.push(memory.map_or_else(|| "-".to_owned(), |(n, b)| ratio(n[1], b[1])));
        row.push(f.output_bytes.to_string());
    } else {
        let detail =
            if link.detail.is_empty() { String::new() } else { format!(": {}", link.detail) };
        row.push(format!("{}{detail}", link.status.text()));
    }
    row
}

/// The table a run prints on standard output.
pub fn table(results: &Results) -> String {
    let mut text = String::new();
    let head =
        ["linker", "threads", "wall ms", "range", "CPU ms", "RSS MiB", "x time", "x mem", "output"];
    for w in &results.workloads {
        let _ = writeln!(
            text,
            "\n{} ({}): {} objects, {} archives, {} shared, {} other; {} MiB in; {} symbols, {} MiB out",
            w.name,
            w.class,
            w.objects,
            w.archives,
            w.shared,
            w.others,
            mib_of_bytes(w.input_bytes),
            w.symbols,
            mib_of_bytes(w.output_bytes)
        );
        let reference = reference(results, &w.name);
        let rows: Vec<Vec<String>> = results
            .links
            .iter()
            .filter(|l| l.workload == w.name)
            .map(|l| cells(l, reference))
            .collect();
        let mut widths: Vec<usize> = head.iter().map(|h| h.len()).collect();
        // A row without figures has its status in the third cell: it does
        // not widen the columns.
        for row in rows.iter().filter(|row| row.len() == head.len()) {
            for (at, cell) in row.iter().enumerate().take(head.len() - 1) {
                widths[at] = widths[at].max(cell.chars().count());
            }
        }
        let line = |cells: &[String]| {
            let mut out = String::from("  ");
            for (at, cell) in cells.iter().enumerate() {
                if at + 1 == cells.len() {
                    out.push_str(cell);
                } else {
                    let _ = write!(out, "{cell:<width$}  ", width = widths[at]);
                }
            }
            out.trim_end().to_owned()
        };
        let _ = writeln!(text, "{}", line(&head.map(str::to_owned)));
        for row in &rows {
            let _ = writeln!(text, "{}", line(row));
        }
    }
    let rows = versus(results);
    if !rows.is_empty() {
        text.push_str(
            "\nmortar against the fastest other linker (wall) and the smallest (RSS), \
             same thread configuration:\n",
        );
        for v in &rows {
            let _ = writeln!(text, "  {}", v.cells().join("  "));
        }
    }
    text
}

/// mortar's link of one workload at one thread configuration, against
/// the fastest and the smallest of the other linkers' `ok` links in the
/// same configuration (GNU ld's one thread counts as its default).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Versus<'a> {
    pub mortar: &'a Link,
    pub fastest: &'a Link,
    pub smallest: Option<&'a Link>,
}

impl Versus<'_> {
    fn figures(link: &Link) -> super::record::Figures {
        link.figures.unwrap_or_else(|| unreachable!("versus() keeps links with figures"))
    }

    /// mortar's median wall time over the fastest other's, ×1000.
    pub fn wall_permille(&self) -> u64 {
        let (m, f) = (Self::figures(self.mortar).wall_us, Self::figures(self.fastest).wall_us);
        u64::try_from(u128::from(m) * 1000 / u128::from(f.max(1))).unwrap_or(u64::MAX)
    }

    /// mortar's median RSS over the smallest other's, ×1000.
    pub fn rss_permille(&self) -> Option<u64> {
        let m = Self::figures(self.mortar).rss_kib?[1];
        let s = Self::figures(self.smallest?).rss_kib?[1];
        Some(u64::try_from(u128::from(m) * 1000 / u128::from(s.max(1))).unwrap_or(u64::MAX))
    }

    /// workload, threads, mortar wall (range), the fastest other's wall
    /// (linker), × wall, mortar RSS, the smallest other's RSS (linker),
    /// × RSS.
    pub fn cells(&self) -> Vec<String> {
        let (m, f) = (Self::figures(self.mortar), Self::figures(self.fastest));
        let rss =
            |link: &Link| Self::figures(link).rss_kib.map_or_else(|| "-".to_owned(), |r| mib(r[1]));
        let named = |link: &Link| format!("{} {}", link.linker, link.threads.text());
        vec![
            self.mortar.workload.clone(),
            self.mortar.threads.text().to_owned(),
            format!("{} ({}–{})", ms(m.wall_us), ms(m.wall_us_min), ms(m.wall_us_max)),
            m.cpu_us.map_or_else(|| "-".to_owned(), ms),
            format!("{} ({})", ms(f.wall_us), named(self.fastest)),
            ratio(m.wall_us, f.wall_us),
            rss(self.mortar),
            self.smallest.map_or_else(|| "-".to_owned(), |s| format!("{} ({})", rss(s), named(s))),
            self.rss_permille().map_or_else(|| "-".to_owned(), |p| fixed(p, 1000, 2)),
        ]
    }
}

/// Every `ok` link of mortar against the other linkers, in the run's
/// order.
pub fn versus(results: &Results) -> Vec<Versus<'_>> {
    let mut rows = Vec::new();
    for mortar in results.links.iter().filter(|l| l.linker == JUDGED && l.figures.is_some()) {
        let others: Vec<&Link> = results
            .links
            .iter()
            .filter(|l| l.workload == mortar.workload && l.linker != JUDGED && l.figures.is_some())
            .filter(|l| l.threads == mortar.threads || l.linker == "bfd")
            .collect();
        let wall = |l: &&Link| l.figures.map_or(u64::MAX, |f| f.wall_us);
        let Some(fastest) = others.iter().copied().min_by_key(wall) else { continue };
        let smallest = others
            .iter()
            .copied()
            .filter(|l| l.figures.is_some_and(|f| f.rss_kib.is_some()))
            .min_by_key(|l| l.figures.and_then(|f| f.rss_kib).map_or(u64::MAX, |r| r[1]));
        rows.push(Versus { mortar, fastest, smallest });
    }
    rows
}

/// The verdicts of a check, one line each, for standard output.
pub fn check_lines(check: &Check) -> String {
    let mut text = String::new();
    if let Some(why) = &check.common.advisory {
        let _ = writeln!(text, "advisory: the baseline does not bind this run: {why}");
    }
    if let Some(why) = &check.common.unsteady {
        let _ = writeln!(text, "unsteady: {why}");
    }
    for j in &check.judged {
        let _ = writeln!(
            text,
            "{} {} {}: {}",
            j.workload,
            j.now.linker,
            j.threads.text(),
            j.verdict.text()
        );
    }
    if check.judged.is_empty() {
        text.push_str("nothing to judge: the run has no mortar link\n");
    }
    text
}

fn md_row(text: &mut String, cells: &[String]) {
    let cells: Vec<String> = cells.iter().map(|c| c.replace('|', "\\|")).collect();
    let _ = writeln!(text, "| {} |", cells.join(" | "));
}

/// `docs/status/bench.md` of a baseline (`results`, at `path` in the
/// checkout) and the manifest's planned workloads.
pub fn status_page(results: &Results, path: &str, workloads: &[Workload]) -> String {
    let get = |key: &str| results.get(key).unwrap_or("?");
    let mut text = String::from("# Bench\n\n");
    let _ = writeln!(
        text,
        "Generated by `cargo xtask bench` (`--update-baseline` or `--report`) from \
         `{path}`; do not edit by hand. The contract, the workloads and how to read the \
         figures: `spec/harness.md` \"bench\".\n"
    );
    let _ = writeln!(
        text,
        "- Machine `{}` ({} processors); recorded {} at commit `{}`.",
        get("machine"),
        get("cores"),
        get("date"),
        get("commit")
    );
    let _ = writeln!(
        text,
        "- {} measured runs per link after a warm-up, one link at a time; figures are the \
         median run, with the fastest and slowest.",
        get("runs")
    );
    let calibration = |key: &str| results.number(key).map_or_else(|| "?".to_owned(), ms);
    let _ = writeln!(
        text,
        "- Calibration: {}: {} ms before the run, {} ms after; noisy: {}. Load average \
         (1, 5, 15 min) before: {}; after: {}.",
        get("calibration"),
        calibration("calibration_before_us"),
        calibration("calibration_after_us"),
        get("noisy"),
        get("load_before"),
        get("load_after")
    );
    let _ = writeln!(text, "- Memory: {}.", get("probe"));
    let _ = writeln!(text, "- Why this baseline: {}\n", get("reason"));
    text.push_str("Linkers:\n\n");
    for name in results.linkers() {
        let version = results.get(&format!("version.{name}")).unwrap_or("?");
        let _ = writeln!(text, "- `{name}`: {version}");
    }
    text.push_str(&workload_section(results, workloads));
    text.push_str(&figures_section(results));
    text.push_str(&planned_section(workloads));
    text
}

fn workload_section(results: &Results, workloads: &[Workload]) -> String {
    let mut text = String::new();
    text.push_str("\n## Workloads\n\n");
    md_row(
        &mut text,
        &[
            "workload",
            "class",
            "objects",
            "archives",
            "shared",
            "other",
            "input MiB",
            "symbols out",
            "MiB out (GNU ld)",
            "what",
        ]
        .map(str::to_owned),
    );
    text.push_str("|---|---|--:|--:|--:|--:|--:|--:|--:|---|\n");
    for w in &results.workloads {
        let about = workloads.iter().find(|m| m.name == w.name).map_or("", |m| m.about.as_str());
        md_row(
            &mut text,
            &[
                format!("`{}`", w.name),
                w.class.clone(),
                w.objects.to_string(),
                w.archives.to_string(),
                w.shared.to_string(),
                w.others.to_string(),
                mib_of_bytes(w.input_bytes),
                w.symbols.to_string(),
                mib_of_bytes(w.output_bytes),
                about.to_owned(),
            ],
        );
    }
    text
}

fn figures_section(results: &Results) -> String {
    let mut text = String::new();
    text.push_str(
        "\n## Time and memory\n\nWall time in milliseconds (median, and the fastest to the \
         slowest run), CPU time (user and system, median) in milliseconds, peak resident memory in MiB (median), and both as a ratio to mold at its \
         default number of threads on the same workload (lld where mold did not link). A link \
         that is not `ok` has no figures. Only mortar's rows are judged against the baseline \
         (35% time, 4% memory); the reference linkers' are information.\n",
    );
    for w in &results.workloads {
        let _ = writeln!(text, "\n### `{}`\n", w.name);
        md_row(
            &mut text,
            &[
                "linker",
                "threads",
                "wall ms",
                "range ms",
                "CPU ms",
                "RSS MiB",
                "× time",
                "× memory",
                "output bytes",
            ]
            .map(str::to_owned),
        );
        text.push_str("|---|---|--:|--:|--:|--:|--:|--:|--:|\n");
        let reference = reference(results, &w.name);
        for link in results.links.iter().filter(|l| l.workload == w.name) {
            let mut row = cells(link, reference);
            row.resize(9, String::new());
            md_row(&mut text, &row);
        }
    }
    text
}

fn planned_section(workloads: &[Workload]) -> String {
    let mut text = String::new();
    let planned: Vec<&Workload> =
        workloads.iter().filter(|w| matches!(w.kind, Kind::Planned { .. })).collect();
    if !planned.is_empty() {
        text.push_str(
            "\n## Planned workloads\n\nSpecified in `bench/workloads/manifest`, not built yet.\n\n",
        );
        md_row(&mut text, &["workload", "class", "needs", "what"].map(str::to_owned));
        text.push_str("|---|---|---|---|\n");
        for w in planned {
            let Kind::Planned { needs } = &w.kind else { continue };
            md_row(
                &mut text,
                &[format!("`{}`", w.name), w.class.clone(), needs.clone(), w.about.clone()],
            );
        }
    }
    text
}

/// The perf page's table of mortar against the others and the rows
/// furthest behind.
fn versus_section(results: &Results) -> String {
    let mut text = String::new();
    let rows = versus(results);
    text.push_str(
        "\n## mortar against the fastest other linker\n\n\
         The other linkers' links in the same configuration (`1`: one thread; `default`: \
         their default, GNU ld's one thread included). × is mortar's over the other's.\n\n",
    );
    md_row(
        &mut text,
        &[
            "workload",
            "threads",
            "mortar wall ms (range)",
            "mortar CPU ms",
            "fastest other wall ms",
            "× wall",
            "mortar RSS MiB",
            "smallest other RSS MiB",
            "× RSS",
        ]
        .map(str::to_owned),
    );
    text.push_str("|---|---|--:|--:|--:|--:|--:|--:|--:|\n");
    for v in &rows {
        md_row(&mut text, &v.cells());
    }
    let worst = |key: &dyn Fn(&Versus<'_>) -> u64, what: &str| {
        let mut sorted: Vec<&Versus<'_>> = rows.iter().collect();
        sorted.sort_by_key(|v| std::cmp::Reverse(key(v)));
        let mut t = format!("\n## The three rows furthest behind ({what})\n\n");
        for v in sorted.iter().take(3) {
            let c = v.cells();
            let _ = writeln!(
                t,
                "- `{}` {}: × wall {}, × RSS {} (mortar {} ms, {} MiB; fastest {}; smallest {}).",
                c[0], c[1], c[5], c[8], c[2], c[6], c[4], c[7]
            );
        }
        t
    };
    if !rows.is_empty() {
        text.push_str(&worst(&|v| v.wall_permille(), "wall time"));
        text.push_str(&worst(&|v| v.rss_permille().unwrap_or(0), "RSS"));
    }
    text
}

/// `docs/status/perf.md` of a run (`results`, the baseline at `path`):
/// mortar against the fastest and the smallest other linker on every
/// workload it links, the rows where it is furthest behind, the
/// workloads it does not link, and every linker's figures. Numbers only.
pub fn perf_page(results: &Results, path: &str) -> String {
    let get = |key: &str| results.get(key).unwrap_or("?");
    let mut text = String::from("# Performance\n\n");
    let _ = writeln!(
        text,
        "Generated by `cargo xtask bench` (`--update-baseline` or `--report`) from `{path}`; \
         do not edit by hand. How it is measured: `spec/harness.md` \"bench\".\n"
    );
    let calibration = |key: &str| results.number(key).map_or_else(|| "?".to_owned(), ms);
    let _ = writeln!(
        text,
        "- Measured {} at commit `{}` on `{}` ({} processors).\n\
         - Load average (1, 5, 15 min) before the run: {}; after: {}. Calibration (lld, \
         default threads): {} ms before, {} ms after; noisy: {}.\n\
         - {} measured runs per link after a warm-up, the linkers interleaved run by run; \
         wall and CPU (user + system) are medians in ms, the range is the fastest to the \
         slowest run; RSS is the median peak in MiB.\n\
         - Default threads: lld, mold and mortar each start one thread per processor they \
         may run on ({}); GNU ld has one.\n\
         - Why this baseline: {}\n\
         - How the gate uses it: the time margin (35%) is widened by the calibration only \
         when a run's machine is slower than at the baseline, never narrowed. Against a \
         baseline recorded under load (a calibration well above the host's quiet one), a \
         run on a quieter machine is REGRESSED (time) only when it is slower than the \
         baseline's loaded time by 35%, so the gate catches gross slowdowns, links lost \
         (LOST) and memory growth (4%, judged whatever the load), not small time changes; \
         a smaller change is shown by comparing mortar's time to the calibration's, or \
         CPU time, across runs.",
        get("date"),
        get("commit"),
        get("machine"),
        get("cores"),
        get("load_before"),
        get("load_after"),
        calibration("calibration_before_us"),
        calibration("calibration_after_us"),
        get("noisy"),
        get("runs"),
        get("cores"),
        get("reason"),
    );
    text.push_str(&versus_section(results));
    let not_linked: Vec<String> = results
        .workloads
        .iter()
        .filter_map(|w| {
            let mortar: Vec<&Link> = results
                .links
                .iter()
                .filter(|l| l.workload == w.name && l.linker == JUDGED)
                .collect();
            let first = mortar.iter().find(|l| l.status != Status::Ok)?;
            Some(format!("- `{}`: {}: {}", w.name, first.status.text(), first.detail))
        })
        .collect();
    if !not_linked.is_empty() {
        text.push_str("\n## Workloads mortar does not link\n\n");
        for line in &not_linked {
            let _ = writeln!(text, "{line}");
        }
    }
    let runs: Vec<String> = results
        .header
        .iter()
        .filter_map(|(k, v)| Some(format!("- `{}`: {v}", k.strip_prefix("run.")?)))
        .collect();
    if !runs.is_empty() {
        text.push_str(
            "\n## mortar's programs run against GNU ld's\n\nOnce per workload, untimed, \
             from the output the verifier judged: exit status and standard output.\n\n",
        );
        for line in &runs {
            let _ = writeln!(text, "{line}");
        }
    }
    text.push_str("\n## Every link\n\n");
    md_row(
        &mut text,
        &["workload", "linker", "threads", "wall ms", "range ms", "CPU ms", "RSS MiB"]
            .map(str::to_owned),
    );
    text.push_str("|---|---|---|--:|--:|--:|--:|\n");
    for w in &results.workloads {
        for link in results.links.iter().filter(|l| l.workload == w.name) {
            let mut row = cells(link, None);
            row.truncate(6);
            row.insert(0, format!("`{}`", w.name));
            row.resize(7, String::new());
            md_row(&mut text, &row);
        }
    }
    text
}

// spec/harness.md "bench", "Report".
#[cfg(test)]
mod tests {
    use super::super::manifest;
    use super::super::record::tests::sample;
    use super::*;

    // "Report": rows with figures have ratios to the reference; the others
    // say what happened instead.
    #[test]
    fn the_table() {
        let results = sample();
        let text = table(&results);
        assert!(
            text.contains(
                "\nhello-c (small): 4 objects, 2 archives, 3 shared, 1 other; 2.2 MiB in"
            ),
            "{text}"
        );
        // lld default is the reference without mold: its ratio is 1.
        assert!(
            text.contains(
                "  lld     default  6.0      5.9–6.6  12.0    8.8      1.00    1.00   70432"
            ),
            "{text}"
        );
        assert!(
            text.contains(
                "  bfd     1        9.0      8.8–9.9  18.0    4.9      1.50    0.56   70432"
            ),
            "{text}"
        );
        assert!(
            text.contains(
                "  mortar  default  refused: mortar: error: linking is not implemented yet"
            ),
            "{text}"
        );
        assert!(text.contains("  lld     default  failed: lld: signal 11"), "{text}");
    }

    // "Report": mortar against the fastest and the smallest other linker in
    // its configuration (GNU ld's one thread counts at both), the rows
    // furthest behind, and what mortar does not link.
    #[test]
    fn mortar_against_the_fastest() {
        use super::super::record::tests::ok;
        let mut results = sample();
        results.links.push(ok("hello-c", "mold", Threads::One, 4_000, 6_000));
        results.links.push(ok("hello-c", "mortar", Threads::One, 7_000, 4_000));
        results.links.push(ok("cxx-medium-g", "mortar", Threads::One, 2_000_000, 800_000));
        results.links.push(ok("cxx-medium-g", "mortar", Threads::Default, 1_000_000, 1_000_000));
        results.set("run.hello-c", "same: exit status 0, 13 bytes of standard output");
        let rows = versus(&results);
        assert_eq!(rows.len(), 3);
        assert_eq!(
            (rows[0].fastest.linker.as_str(), rows[0].smallest.map(|l| l.linker.as_str())),
            ("mold", Some("bfd"))
        );
        assert_eq!((rows[0].wall_permille(), rows[0].rss_permille()), (1750, Some(800)));
        assert_eq!(
            rows[0].cells(),
            [
                "hello-c",
                "1",
                "7.0 (6.9–7.7)",
                "14.0",
                "4.0 (mold 1)",
                "1.75",
                "3.9",
                "4.9 (bfd 1)",
                "0.80"
            ]
        );
        // Only GNU ld linked cxx-medium-g: it is the reference at both.
        assert_eq!((rows[2].fastest.linker.as_str(), rows[2].wall_permille()), ("bfd", 250));
        let page = perf_page(&results, "bench/baselines/m.tsv");
        assert!(page.starts_with("# Performance\n\nGenerated by `cargo xtask bench`"), "{page}");
        assert!(page.contains("| hello-c | 1 | 7.0 (6.9–7.7) | 14.0 | 4.0 (mold 1) | 1.75 | 3.9 | 4.9 (bfd 1) | 0.80 |"), "{page}");
        let behind = page.split("## The three rows furthest behind (wall time)").nth(1).unwrap();
        assert!(behind.trim_start().starts_with("- `hello-c` 1: × wall 1.75"), "{page}");
        assert!(page.contains("## Workloads mortar does not link\n\n- `hello-c`: refused: mortar: error: linking is not implemented yet\n"), "{page}");
        assert!(
            page.contains("- `hello-c`: same: exit status 0, 13 bytes of standard output"),
            "{page}"
        );
        assert!(
            page.contains("| `cxx-medium-g` | lld | default | failed: lld: signal 11 |  |  |  |"),
            "{page}"
        );
        assert!(page.contains("| `hello-c` | bfd | 1 | 9.0 | 8.8–9.9 | 18.0 | 4.9 |"), "{page}");
        assert!(table(&results).contains("\nmortar against the fastest other linker"));
    }

    // "Report": the status page names its source and its machine, has a
    // table per workload, and lists the planned workloads.
    #[test]
    fn the_status_page() {
        let mut results = sample();
        results.set("reason", "first numbers");
        let planned = manifest::parse(
            "[libcxx-test]\nclass = small\nkind = planned\nneeds = kiln | hod\nabout = One test.\n",
        )
        .unwrap();
        let page = status_page(&results, "bench/baselines/m.tsv", &planned);
        assert!(page.starts_with("# Bench\n\nGenerated by `cargo xtask bench`"), "{page}");
        assert!(page.contains("from `bench/baselines/m.tsv`"));
        assert!(page.contains("- Machine `aarch64-implementer-0x61-part-0x000-24c`"));
        assert!(page.contains("- Why this baseline: first numbers"));
        assert!(page.contains("- `bfd`: GNU ld (GNU Binutils for Debian) 2.44"));
        assert!(
            page.contains("| `cxx-medium-g` | medium | 4 | 2 | 3 | 1 | 2.2 | 1234 | 0.1 |  |"),
            "{page}"
        );
        assert!(page.contains("\n### `hello-c`\n"));
        assert!(
            page.contains(
                "| mortar | default | refused: mortar: error: linking is not implemented yet |  |"
            ),
            "{page}"
        );
        assert!(page.contains("| `libcxx-test` | small | kiln \\| hod | One test. |"), "{page}");
    }
}
