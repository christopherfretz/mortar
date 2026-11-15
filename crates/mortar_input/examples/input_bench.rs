//! The input phase on a bench workload (`spec/input.md` §11.8, §12), under
//! both stores (§11.1): per store and thread count, the wall and CPU time
//! of the walk (stat, read, sniff, parse), then of a stand-in for the
//! image phase's reads (every bulk section's bytes through `copy_to` into
//! 1 MiB buffers), with the peak RSS, the machine's load, and the store's
//! reads and descriptors.
//!
//! `cargo run --release -p mortar_input --example input_bench -- DIR
//! [THREADS...]`: `DIR` is a workload (`DIR/response.txt`, one argument
//! per line, run in `DIR/build`); thread counts default to 1 2 4 8;
//! `STORES=resident,staged` (the default) chooses the stores. Each store
//! and thread count runs in its own process, so its peak RSS is its own;
//! each process runs `RUNS` times (default 5) with a fresh store and
//! reports medians, then runs the whole of `load` (the walk plus interning
//! into `Inputs`) `RUNS` times and reports its median.
//!
//! The bulk sections: every section with file bytes but the symbol and
//! string tables, relocations, groups and `SHT_SYMTAB_SHNDX`, of every
//! relocatable object the walk loaded: what the image phase copies.
#![allow(clippy::cast_precision_loss, reason = "megabytes for a table")]

use std::cell::RefCell;
use std::ffi::OsString;
use std::ops::Range;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{Duration, Instant};

use mortar_core::inputs::Reader;
use mortar_core::par::Pool;
use mortar_diag::{Collector, Policy};
use mortar_elf::consts::{
    SHT_GROUP, SHT_NOBITS, SHT_REL, SHT_RELA, SHT_STRTAB, SHT_SYMTAB, SHT_SYMTAB_SHNDX,
};
use mortar_input::store::{Blob, Hooks, Store, StoreConfig, StorePolicy, StoreStats};
use mortar_input::walk::{Options, Timing, WalkEnv, walk};

const WINDOW: u64 = 1 << 20;

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

/// The process's user plus system CPU time (`/proc/self/stat`, clock
/// ticks of 10 ms).
fn cpu() -> Duration {
    let stat = std::fs::read_to_string("/proc/self/stat").unwrap_or_default();
    let after = stat.rsplit_once(')').map_or("", |(_, rest)| rest);
    let fields: Vec<u64> = after.split_whitespace().filter_map(|f| f.parse().ok()).collect();
    // After the name: state (not a number), then ppid.. ; utime and stime
    // are fields 14 and 15 of the whole line, 11 and 12 of the numbers.
    let ticks = fields.get(10).copied().unwrap_or(0) + fields.get(11).copied().unwrap_or(0);
    Duration::from_millis(ticks * 10)
}

fn ms(d: Duration) -> f64 {
    d.as_secs_f64() * 1e3
}

fn median(mut v: Vec<Duration>) -> Duration {
    v.sort();
    v[v.len() / 2]
}

/// The bulk sections of the walk's objects, in 1 MiB windows.
fn bulk_windows(out: &mortar_input::walk::Loaded<'_>) -> Vec<(Blob, Range<u64>)> {
    let mut windows = Vec::new();
    for f in &out.files {
        let Reader::Object(object) = f.reader else { continue };
        for s in object.sections() {
            let skip = matches!(
                s.sh_type(),
                SHT_NOBITS
                    | SHT_SYMTAB
                    | SHT_STRTAB
                    | SHT_REL
                    | SHT_RELA
                    | SHT_GROUP
                    | SHT_SYMTAB_SHNDX
            );
            let Ok(Some(r)) = s.file_range() else { continue };
            if skip || r.is_empty() {
                continue;
            }
            let (lo, hi) = (f.base + r.start as u64, f.base + r.end as u64);
            let mut at = lo;
            while at < hi {
                let end = (at + WINDOW).min(hi);
                windows.push((f.blob, at..end));
                at = end;
            }
        }
    }
    windows
}

thread_local! {
    static BUFFER: RefCell<Vec<u8>> = RefCell::new(vec![0u8; WINDOW as usize]);
}

/// Copies every window through `copy_to`, from every worker; the xor of
/// the first byte of each, so that nothing is optimized away.
fn bulk_copy(store: &Store, pool: &Pool, windows: &[(Blob, Range<u64>)]) -> u8 {
    let firsts = pool.map(windows.len(), |i| {
        let (b, r) = &windows[i];
        BUFFER.with(|buf| {
            let mut buf = buf.borrow_mut();
            let out = &mut buf[..(r.end - r.start) as usize];
            store.copy_to(*b, r.clone(), out).expect("the workload does not change");
            out[0]
        })
    });
    firsts.into_iter().fold(0, |a, b| a ^ b)
}

struct Run {
    walk: Duration,
    walk_cpu: Duration,
    copy: Duration,
    copy_cpu: Duration,
    timing: Timing,
    stats: StoreStats,
    bulk: u64,
    rows: usize,
}

fn child(dir: &Path, policy: StorePolicy, threads: usize, runs: usize) {
    let text = std::fs::read_to_string(dir.join("response.txt")).expect("response.txt");
    let args: Vec<OsString> = text.lines().filter(|l| !l.is_empty()).map(OsString::from).collect();
    let plan = mortar_cli::parse(&args).plan;
    let env = WalkEnv { cwd: dir.join("build") };
    let pool = Pool::new(threads);
    let load_before = loadavg();
    let mut all: Vec<Run> = Vec::new();
    let mut check = 0u8;
    for _ in 0..runs {
        let store = Store::with_config(policy, config(), Hooks::default());
        let (t, c) = (Instant::now(), cpu());
        let out = walk(&plan, &env, &store, &pool, Options::default());
        let (walk_time, walk_cpu) = (t.elapsed(), cpu().saturating_sub(c));
        assert!(out.problems.iter().all(|p| !p.is_error()), "the workload loads");
        let windows = bulk_windows(&out);
        let bulk = windows.iter().map(|(_, r)| r.end - r.start).sum();
        let (t, c) = (Instant::now(), cpu());
        check ^= bulk_copy(&store, &pool, &windows);
        let (copy, copy_cpu) = (t.elapsed(), cpu().saturating_sub(c));
        all.push(Run {
            walk: walk_time,
            walk_cpu,
            copy,
            copy_cpu,
            timing: out.timing,
            stats: store.stats(),
            bulk,
            rows: out.files.len(),
        });
    }
    let hwm = status("VmHWM:");
    let load_after = loadavg();
    let walk_med = median(all.iter().map(|r| r.walk).collect());
    let total_med = median(all.iter().map(|r| r.walk + r.copy).collect());
    let at = all.iter().position(|r| r.walk == walk_med).unwrap_or(0);
    let r = &all[at];
    let st = r.stats;
    println!(
        "| {policy:?} | {threads} | {:.1} | {:.1} | {:.1} | {:.1} | {:.1} | {} | {:.1} | {} | {} | {:.1} | {} | {} | {:.1} | {} | {load_before} -> {load_after} |",
        ms(walk_med),
        ms(median(all.iter().map(|r| r.walk_cpu).collect())),
        ms(median(all.iter().map(|r| r.copy).collect())),
        ms(median(all.iter().map(|r| r.copy_cpu).collect())),
        ms(total_med),
        hwm,
        st.bytes as f64 / 1e6,
        st.reads,
        st.opens,
        st.resident as f64 / 1e6,
        st.peak_open,
        st.open_now,
        st.copy_bytes as f64 / 1e6,
        st.copy_reads,
    );
    println!(
        "  walk steps (median run): walk {:.1}, stat {:.1}, canon {:.1}, read {:.1}, facts {:.1}, hash {:.1} ms; rows {}; bulk {:.1} MB; check {check}",
        ms(r.timing.walk),
        ms(r.timing.stat),
        ms(r.timing.canon),
        ms(r.timing.read),
        ms(r.timing.facts),
        ms(r.timing.hash),
        r.rows,
        r.bulk as f64 / 1e6,
    );
    let env = mortar_input::LoadEnv { cwd: env.cwd, pool };
    let mut loads = Vec::new();
    let mut keys = (0, 0, 0);
    for _ in 0..runs {
        let store = Store::with_config(policy, config(), Hooks::default());
        let diag = Collector::new(Policy::default());
        let t = Instant::now();
        let inputs = mortar_input::load(&plan, &env, &store, &diag).expect("the workload loads");
        loads.push(t.elapsed());
        keys = (inputs.files.len(), inputs.symbols.len(), inputs.comdats.len());
    }
    println!(
        "  load median {:.1} ms; files {}, symbol keys {}, comdat keys {}; VmHWM {}; load {}",
        ms(median(loads)),
        keys.0,
        keys.1,
        keys.2,
        status("VmHWM:"),
        loadavg(),
    );
}

/// The staged store's first reads, from `HEAD_KIB` and `TAIL_KIB` (the
/// measurement that chose the defaults).
fn config() -> StoreConfig {
    let kib = |name: &str| std::env::var(name).ok().and_then(|v| v.parse::<u64>().ok());
    let d = StoreConfig::default();
    StoreConfig {
        head: kib("HEAD_KIB").map_or(d.head, |k| k << 10),
        tail: kib("TAIL_KIB").map_or(d.tail, |k| k << 10),
        ..d
    }
}

fn policy(name: &str) -> StorePolicy {
    match name {
        "resident" => StorePolicy::Resident,
        "staged" => StorePolicy::Staged,
        other => panic!("unknown store {other}"),
    }
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let runs: usize = std::env::var("RUNS").ok().and_then(|r| r.parse().ok()).unwrap_or(5);
    if args.first().map(String::as_str) == Some("--child") {
        let threads = args[3].parse().expect("threads");
        child(Path::new(&args[1]), policy(&args[2]), threads, runs);
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
    let stores = std::env::var("STORES").unwrap_or_else(|_| "resident,staged".to_owned());
    let exe = std::env::current_exe().expect("the example's path");
    let c = config();
    println!(
        "workload {}; {runs} runs per row; times are medians (ms); head {} KiB, tail {} KiB",
        dir.display(),
        c.head >> 10,
        c.tail >> 10
    );
    println!(
        "| store | threads | walk | walk CPU | copy | copy CPU | walk+copy | VmHWM | read MB | read calls | opens | resident MB | peak open | open after | copy_to disk MB | copy_to reads | load avg |"
    );
    println!("|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|");
    for store in stores.split(',') {
        for &t in &threads {
            let status = Command::new(&exe)
                .arg("--child")
                .arg(&dir)
                .arg(store)
                .arg(t.to_string())
                .status()
                .expect("the child");
            assert!(status.success(), "the {store} {t}-thread child failed");
        }
    }
}
