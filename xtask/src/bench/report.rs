//! What a person reads (`spec/harness.md` "bench", "Report"): the table a
//! run prints, and `docs/status/bench.md`, generated from a baseline.

use std::fmt::Write as _;

use super::manifest::{Kind, Workload};
use super::record::{Link, Results, Status, Threads};
use super::stats::Check;

/// Where the status page is, in the checkout.
pub const STATUS_PAGE: &str = "docs/status/bench.md";

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

/// One row's cells: linker, threads, wall ms, range ms, RSS MiB, × time,
/// × memory, output bytes; a link that is not `ok` has its status and
/// detail instead of figures.
fn cells(link: &Link, reference: Option<&Link>) -> Vec<String> {
    let mut row = vec![link.linker.clone(), link.threads.text().to_owned()];
    if let Some(f) = link.figures {
        let base = reference.and_then(|r| r.figures);
        row.push(ms(f.wall_us));
        row.push(format!("{}–{}", ms(f.wall_us_min), ms(f.wall_us_max)));
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
    let head = ["linker", "threads", "wall ms", "range", "RSS MiB", "x time", "x mem", "output"];
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
    text
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
         slowest run), peak resident memory in MiB (median), and both as a ratio to mold at its \
         default number of threads on the same workload (lld where mold did not link). A link \
         that is not `ok` has no figures. Only mortar's rows are judged against the baseline \
         (5% time, 2% memory); the reference linkers' are information.\n",
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
                "RSS MiB",
                "× time",
                "× memory",
                "output bytes",
            ]
            .map(str::to_owned),
        );
        text.push_str("|---|---|--:|--:|--:|--:|--:|--:|\n");
        let reference = reference(results, &w.name);
        for link in results.links.iter().filter(|l| l.workload == w.name) {
            let mut row = cells(link, reference);
            row.resize(8, String::new());
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
            text.contains("  lld     default  6.0      5.9–6.6  8.8      1.00    1.00   70432"),
            "{text}"
        );
        assert!(
            text.contains("  bfd     1        9.0      8.8–9.9  4.9      1.50    0.56   70432"),
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
