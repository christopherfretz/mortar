//! Measuring links (`spec/harness.md` "bench", "Linkers", "Replay",
//! "Measurement", "Calibration"): the linkers and their configurations,
//! one measured child at a time, rounds after a warm-up.

use std::path::{Path, PathBuf};
use std::time::Duration;

use super::prepare::Prepared;
use super::record::{Figures, Fnv, Link, Status, Threads};
use super::stats::median;
use crate::process::{self, End, Limits, Output, Probe};

/// The limits of a measured link: ten minutes, 16 GiB of address space
/// (lld at its default number of threads cannot start them under 1 GiB).
pub const LINK_LIMITS: Limits = Limits { time: Duration::from_mins(10), memory_kib: 16 << 20 };

/// The linkers the bench knows, in the order of the table.
pub const LINKERS: [&str; 4] = ["bfd", "lld", "mold", "mortar"];

/// A linker as measured.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Linker {
    /// One of [`LINKERS`].
    pub name: &'static str,
    pub path: PathBuf,
    /// The first line of its `--version`.
    pub version: String,
}

impl Linker {
    /// The configurations it is measured in: GNU ld has one thread and no
    /// option for it; the others at their default and at one.
    pub fn configs(&self) -> &'static [Threads] {
        if self.name == "bfd" { &[Threads::One] } else { &[Threads::Default, Threads::One] }
    }

    /// The arguments after the response file and `-o`: one thread where
    /// asked; mold links in the process measured (`--no-fork`: by default
    /// its parent returns while a child it does not wait for holds the
    /// memory, which GNU `time` would not count).
    pub fn flags(&self, threads: Threads) -> Vec<&'static str> {
        let mut flags = Vec::new();
        if self.name == "mold" {
            flags.push("--no-fork");
        }
        if threads == Threads::One && self.name != "bfd" {
            flags.push("--threads=1");
        }
        flags
    }

    /// The judged linker: only its refusal is `refused`.
    pub fn judged(&self) -> bool {
        self.name == super::stats::JUDGED
    }
}

/// One run of a link.
#[derive(Clone, Debug)]
pub struct Run {
    pub output: Output,
    /// The size of the file it wrote, if it left a non-empty one.
    pub output_bytes: Option<u64>,
    /// The hash of that file, when asked for (the judged linker's warm-up
    /// runs: the golden output check).
    pub output_hash: Option<String>,
}

/// Runs `linker` once on `prepared`: `<linker> @<response> -o <out>
/// <flags>` in the directory the driver ran the linker in (the last `-o`
/// wins in all of them). The output is removed before and after; with
/// `hash`, it is hashed first.
pub fn run_once(
    prepared: &Prepared,
    linker: &Linker,
    threads: Threads,
    probe: &Probe,
    scratch: &Path,
    hash: bool,
) -> Result<Run, String> {
    let out =
        scratch.join(format!("{}-{}-{}.out", prepared.workload.name, linker.name, threads.text()));
    let usage = scratch.join(format!("{}-{}.usage", linker.name, threads.text()));
    let _ = std::fs::remove_file(&out);
    let output = process::run_measured(
        &linker.path,
        |c| {
            c.arg(format!("@{}", prepared.response.display())).arg("-o").arg(&out);
            c.args(linker.flags(threads)).current_dir(&prepared.cwd).env("LC_ALL", "C");
        },
        b"",
        LINK_LIMITS,
        probe,
        &usage,
    )?;
    let output_bytes = std::fs::metadata(&out).ok().map(|m| m.len()).filter(|n| *n > 0);
    let output_hash = if hash && output_bytes.is_some() {
        std::fs::read(&out).ok().map(|bytes| {
            let mut h = Fnv::default();
            h.update(&bytes);
            h.text()
        })
    } else {
        None
    };
    let _ = std::fs::remove_file(&out);
    Ok(Run { output, output_bytes, output_hash })
}

/// What one run says about a link: `ok` (status 0 and a non-empty
/// output), `refused` (the judged linker only: a non-zero status, a
/// diagnostic, no output), `killed` (by a limit), else `failed`. With the
/// detail for a link that is not `ok`.
pub fn status_of(run: &Run, linker: &Linker) -> (Status, String) {
    let end = run.output.end;
    let first = run.output.first_error_line();
    if let End::Killed(_) = end {
        return (Status::Killed, end.to_string());
    }
    if end.success() {
        return match run.output_bytes {
            Some(_) => (Status::Ok, String::new()),
            None => (Status::Failed, "exit status 0 and no output".to_owned()),
        };
    }
    let refused = linker.judged()
        && matches!(end, End::Exit(_))
        && run.output_bytes.is_none()
        && !first.is_empty();
    if refused {
        return (Status::Refused, first);
    }
    let detail = if first.is_empty() { end.to_string() } else { format!("{end}: {first}") };
    (Status::Failed, detail)
}

/// The figures of measured runs: wall time and peak memory, fastest,
/// middle and slowest; the middle CPU time.
pub fn figures(runs: &[Run]) -> Option<Figures> {
    let walls: Vec<u64> = runs.iter().map(|r| r.output.wall.as_micros() as u64).collect();
    let three =
        |values: &[u64]| Some([*values.iter().min()?, median(values)?, *values.iter().max()?]);
    let [wall_us_min, wall_us, wall_us_max] = three(&walls)?;
    let usage: Vec<_> = runs.iter().filter_map(|r| r.output.usage).collect();
    let complete = usage.len() == runs.len();
    let rss_kib =
        complete.then(|| three(&usage.iter().map(|u| u.rss_kib).collect::<Vec<_>>())).flatten();
    let cpu_us =
        complete.then(|| median(&usage.iter().map(|u| u.cpu_us).collect::<Vec<_>>())).flatten();
    let output_bytes = runs.last()?.output_bytes?;
    Some(Figures { wall_us_min, wall_us, wall_us_max, cpu_us, rss_kib, output_bytes })
}

/// Measures every configuration of `linkers` on `prepared`: a warm-up
/// round (discarded; it decides whether a configuration links), then
/// `runs` rounds, each running every configuration that linked once, in
/// the same order. A configuration whose measured run does not link is
/// `failed` (or `killed`) with no figures.
///
/// The golden output check (WP-032a): the judged linker's warm-up outputs
/// are hashed; outputs that differ between its thread configurations
/// fail both (mortar's output does not depend on the thread count), and
/// the hash is returned, for the record's `output_hash.<workload>` (a
/// commit that only makes mortar faster keeps it).
pub fn measure(
    prepared: &Prepared,
    linkers: &[Linker],
    runs: u32,
    probe: &Probe,
    scratch: &Path,
) -> Result<(Vec<Link>, Option<String>), String> {
    let name = &prepared.workload.name;
    let mut links = Vec::new();
    let mut measured: Vec<(usize, Vec<Run>)> = Vec::new();
    let mut golden: Vec<(usize, String)> = Vec::new();
    for linker in linkers {
        for &threads in linker.configs() {
            let warm = run_once(prepared, linker, threads, probe, scratch, linker.judged())?;
            let (status, detail) = status_of(&warm, linker);
            if status == Status::Ok
                && let Some(h) = &warm.output_hash
            {
                golden.push((links.len(), h.clone()));
            }
            if status == Status::Ok {
                measured.push((links.len(), Vec::new()));
            }
            links.push(Link {
                status,
                workload: name.clone(),
                linker: linker.name.to_owned(),
                threads,
                runs: 0,
                figures: None,
                detail,
            });
        }
    }
    if golden.windows(2).any(|w| w[0].1 != w[1].1) {
        let all: Vec<&str> = golden.iter().map(|(_, h)| h.as_str()).collect();
        for (at, _) in &golden {
            links[*at].status = Status::Failed;
            links[*at].detail =
                format!("the output differs between thread configurations: {}", all.join(" "));
        }
    }
    let output_hash = golden.first().map(|(_, h)| h.clone());
    let configs: Vec<(&Linker, Threads)> =
        linkers.iter().flat_map(|l| l.configs().iter().map(move |t| (l, *t))).collect();
    for round in 0..runs {
        for (at, samples) in &mut measured {
            if links[*at].status != Status::Ok {
                continue;
            }
            let (linker, threads) = configs[*at];
            let run = run_once(prepared, linker, threads, probe, scratch, false)?;
            let (status, detail) = status_of(&run, linker);
            if status == Status::Ok {
                samples.push(run);
            } else {
                links[*at].status =
                    if status == Status::Killed { Status::Killed } else { Status::Failed };
                links[*at].detail = format!("run {}: {detail}", round + 1);
            }
        }
    }
    for (at, samples) in measured {
        if links[at].status == Status::Ok {
            links[at].runs = samples.len() as u32;
            links[at].figures = figures(&samples);
        }
    }
    Ok((links, output_hash))
}

/// The calibration link: `linker` at its default threads on the
/// calibration workload, once to warm up, then `runs` times; the middle
/// wall time in microseconds.
pub fn calibrate(
    prepared: &Prepared,
    linker: &Linker,
    runs: u32,
    probe: &Probe,
    scratch: &Path,
) -> Result<u64, String> {
    let mut walls = Vec::new();
    for at in 0..=runs {
        let run = run_once(prepared, linker, Threads::Default, probe, scratch, false)?;
        let (status, detail) = status_of(&run, linker);
        if status != Status::Ok {
            return Err(format!(
                "the calibration link ({} on {}) did not link: {detail}",
                linker.name, prepared.workload.name
            ));
        }
        if at > 0 {
            walls.push(run.output.wall.as_micros() as u64);
        }
    }
    median(&walls).ok_or_else(|| "no calibration run".to_owned())
}

// spec/harness.md "bench", "Linkers", "Measurement".
#[cfg(test)]
mod tests {
    use super::*;
    use crate::process::{Limit, Usage};

    fn linker(name: &'static str) -> Linker {
        Linker { name, path: PathBuf::from("/bin/false"), version: String::new() }
    }

    fn run(end: End, stderr: &str, wall_ms: u64, rss: Option<u64>, output: Option<u64>) -> Run {
        Run {
            output: Output {
                end,
                stdout: Vec::new(),
                stderr: stderr.as_bytes().to_vec(),
                truncated: false,
                wall: Duration::from_millis(wall_ms),
                usage: rss.map(|rss_kib| Usage { rss_kib, cpu_us: wall_ms * 1500 }),
            },
            output_bytes: output,
            output_hash: None,
        }
    }

    // "Linkers": the configurations and the arguments of each.
    #[test]
    fn linker_configurations() {
        assert_eq!(linker("bfd").configs(), [Threads::One]);
        assert!(linker("bfd").flags(Threads::One).is_empty());
        assert_eq!(linker("lld").configs(), [Threads::Default, Threads::One]);
        assert!(linker("lld").flags(Threads::Default).is_empty());
        assert_eq!(linker("lld").flags(Threads::One), ["--threads=1"]);
        assert_eq!(linker("mold").flags(Threads::Default), ["--no-fork"]);
        assert_eq!(linker("mold").flags(Threads::One), ["--no-fork", "--threads=1"]);
        assert_eq!(linker("mortar").flags(Threads::One), ["--threads=1"]);
        assert!(linker("mortar").judged() && !linker("lld").judged());
    }

    // "Measurement": how a run ends decides the row's status; only mortar
    // refuses.
    #[test]
    fn statuses() {
        let status = |r: &Run, l: &str| {
            let l = linker(match l {
                "mortar" => "mortar",
                _ => "lld",
            });
            status_of(r, &l)
        };
        let refusal = "mortar: error: linking is not implemented yet\n";
        assert_eq!(
            status(&run(End::Exit(0), "", 5, None, Some(10)), "lld"),
            (Status::Ok, String::new())
        );
        assert_eq!(status(&run(End::Exit(1), refusal, 5, None, None), "mortar").0, Status::Refused);
        assert_eq!(
            status(&run(End::Exit(1), refusal, 5, None, None), "mortar").1,
            "mortar: error: linking is not implemented yet"
        );
        // A reference linker's non-zero status is a failure, as is mortar's
        // that left an output, said nothing, or died by a signal.
        assert_eq!(
            status(&run(End::Exit(1), "ld.lld: error: x\n", 5, None, None), "lld"),
            (Status::Failed, "exit status 1: ld.lld: error: x".to_owned())
        );
        assert_eq!(
            status(&run(End::Exit(1), refusal, 5, None, Some(3)), "mortar").0,
            Status::Failed
        );
        assert_eq!(
            status(&run(End::Exit(1), "", 5, None, None), "mortar"),
            (Status::Failed, "exit status 1".to_owned())
        );
        assert_eq!(
            status(&run(End::Signal(11), refusal, 5, None, None), "mortar").0,
            Status::Failed
        );
        assert_eq!(
            status(&run(End::Exit(0), "", 5, None, None), "lld"),
            (Status::Failed, "exit status 0 and no output".to_owned())
        );
        // Killed by a limit is its own state, for every linker.
        for l in ["lld", "mortar"] {
            assert_eq!(
                status(&run(End::Killed(Limit::Memory), "", 5, None, None), l),
                (Status::Killed, "killed: memory".to_owned())
            );
            assert_eq!(
                status(&run(End::Killed(Limit::Time), "", 5, None, Some(4)), l).0,
                Status::Killed
            );
        }
    }

    // "Measurement": fastest, middle and slowest of the runs.
    #[test]
    fn figures_of_runs() {
        let runs: Vec<Run> = [(12, 900), (10, 1000), (30, 1100), (11, 1000), (13, 950)]
            .iter()
            .map(|&(ms, rss)| run(End::Exit(0), "", ms, Some(rss), Some(77)))
            .collect();
        let f = figures(&runs).unwrap();
        assert_eq!((f.wall_us_min, f.wall_us, f.wall_us_max), (10_000, 12_000, 30_000));
        assert_eq!(f.rss_kib, Some([900, 1000, 1100]));
        assert_eq!((f.cpu_us, f.output_bytes), (Some(18_000), 77));
        // Without the probe of one run there are no memory figures.
        let mut partial = runs.clone();
        partial[2].output.usage = None;
        assert_eq!(figures(&partial).unwrap().rss_kib, None);
        assert_eq!(figures(&[]), None);
    }
}
