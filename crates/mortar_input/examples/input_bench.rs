//! The input phase on a bench workload (`spec/input.md` §11.8, §12):
//! per thread count, the wall time of the walk (stat, read, sniff, parse),
//! the peak RSS, the machine's load, and the system calls of the store.
//!
//! `cargo run --release -p mortar_input --example input_bench -- DIR
//! [THREADS...]`: `DIR` is a workload (`DIR/response.txt`, one argument
//! per line, run in `DIR/build`); thread counts default to 1 2 4 8. Each
//! thread count runs in its own process, so its peak RSS is its own; each
//! process runs the walk `RUNS` times (default 5) with a fresh store and
//! reports the median and minimum.

use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{Duration, Instant};

use mortar_core::par::Pool;
use mortar_input::store::{Store, StorePolicy};
use mortar_input::walk::{Options, Timing, WalkEnv, walk};

fn status(key: &str) -> String {
    std::fs::read_to_string("/proc/self/status")
        .unwrap_or_default()
        .lines()
        .find(|l| l.starts_with(key))
        .map(|l| l[key.len()..].trim().to_owned())
        .unwrap_or_default()
}

fn loadavg() -> String {
    std::fs::read_to_string("/proc/loadavg")
        .unwrap_or_default()
        .split_whitespace()
        .take(3)
        .collect::<Vec<_>>()
        .join(" ")
}

fn ms(d: Duration) -> f64 {
    d.as_secs_f64() * 1e3
}

fn child(dir: &Path, threads: usize, runs: usize) {
    let text = std::fs::read_to_string(dir.join("response.txt")).expect("response.txt");
    let args: Vec<OsString> = text.lines().filter(|l| !l.is_empty()).map(OsString::from).collect();
    let plan = mortar_cli::parse(&args).plan;
    let env = WalkEnv { cwd: dir.join("build") };
    let pool = Pool::new(threads);
    let load_before = loadavg();
    let mut times = Vec::new();
    let mut timings: Vec<Timing> = Vec::new();
    let mut last = None;
    for _ in 0..runs {
        let store = Store::new(StorePolicy::Resident);
        let t = Instant::now();
        let out = walk(&plan, &env, &store, &pool, Options::default());
        times.push(t.elapsed());
        timings.push(out.timing);
        last = Some((
            out.files.len(),
            out.archives.len(),
            out.link.opened.len(),
            out.problems.len(),
            out.waves,
            store.stats(),
        ));
    }
    let mut order: Vec<usize> = (0..runs).collect();
    order.sort_by_key(|&i| times[i]);
    let median = order[runs / 2];
    let tm = timings[median];
    let (files, archives, opened, problems, waves, st) = last.expect("one run");
    println!(
        "threads {threads}: median {:.1} ms (min {:.1}); walk {:.1}, stat {:.1}, canon {:.1}, read {:.1}, facts {:.1}, hash {:.1} ms; \
         VmHWM {}; load {load_before} -> {}; \
         rows {files}, archives {archives}, opened {opened}, problems {problems}, waves {waves}; \
         files read {}, bytes {} ({} KiB); stat {}, open {}, fstat {}, read {}, close {}, peak open {}",
        ms(times[median]),
        ms(times[order[0]]),
        ms(tm.walk),
        ms(tm.stat),
        ms(tm.canon),
        ms(tm.read),
        ms(tm.facts),
        ms(tm.hash),
        status("VmHWM:"),
        loadavg(),
        st.files,
        st.bytes,
        st.bytes >> 10,
        st.stats,
        st.opens,
        st.fstats,
        st.reads,
        st.closes,
        st.peak_open,
    );
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let runs: usize = std::env::var("RUNS").ok().and_then(|r| r.parse().ok()).unwrap_or(5);
    if args.first().map(String::as_str) == Some("--child") {
        child(Path::new(&args[1]), args[2].parse().expect("threads"), runs);
        return;
    }
    let Some(dir) = args.first().map(PathBuf::from) else {
        eprintln!("usage: input_bench DIR [THREADS...]");
        std::process::exit(2);
    };
    let threads: Vec<usize> = if args.len() > 1 {
        args[1..].iter().map(|t| t.parse().expect("threads")).collect()
    } else {
        vec![1, 2, 4, 8]
    };
    let exe = std::env::current_exe().expect("the example's path");
    println!("workload {}; {runs} runs per thread count", dir.display());
    for t in threads {
        let status = Command::new(&exe)
            .arg("--child")
            .arg(&dir)
            .arg(t.to_string())
            .status()
            .expect("the child");
        assert!(status.success(), "the {t}-thread child failed");
    }
}
