//! `--time-phases` (I-88..I-91) and the peak-memory bound (I-92).

use std::fmt::Write as _;
use std::time::{Duration, Instant};

/// The clock tick of `/proc/self/stat` (`CLK_TCK`, E15): 100 per second.
pub const CLK_TCK: u64 = 100;

/// A reading of the process's counters; `None` where a value could not be
/// read (shown `-`, I-88).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Sample {
    pub at: Instant,
    /// `utime + stime` in ticks.
    pub cpu_ticks: Option<u64>,
    /// `VmRSS` in KiB.
    pub rss_kib: Option<u64>,
    /// `VmHWM` in KiB.
    pub peak_kib: Option<u64>,
}

/// Fields 14 and 15 of a `/proc/<pid>/stat` line, summed. The second field
/// (the command) is parenthesised and may hold spaces and parentheses:
/// fields are counted after its last `)`.
#[must_use]
pub fn stat_cpu_ticks(stat: &str) -> Option<u64> {
    let rest = &stat[stat.rfind(')')? + 1..];
    let mut f = rest.split_ascii_whitespace();
    // `rest` starts at field 3.
    let utime: u64 = f.nth(11)?.parse().ok()?;
    let stime: u64 = f.next()?.parse().ok()?;
    Some(utime + stime)
}

/// The value in KiB of `field` (`VmRSS`, `VmHWM`) of a `/proc/<pid>/status`
/// text.
#[must_use]
pub fn status_kib(status: &str, field: &str) -> Option<u64> {
    status.lines().find_map(|l| {
        let v = l.strip_prefix(field)?.strip_prefix(':')?;
        v.trim().strip_suffix("kB")?.trim().parse().ok()
    })
}

impl Sample {
    /// Reads `/proc/self/stat` and `/proc/self/status` (I-91: safe std
    /// reads, made only under `--time-phases`).
    #[must_use]
    pub fn now() -> Self {
        let at = Instant::now();
        let stat = std::fs::read_to_string("/proc/self/stat").ok();
        let status = std::fs::read_to_string("/proc/self/status").ok();
        Self {
            at,
            cpu_ticks: stat.as_deref().and_then(stat_cpu_ticks),
            rss_kib: status.as_deref().and_then(|s| status_kib(s, "VmRSS")),
            peak_kib: status.as_deref().and_then(|s| status_kib(s, "VmHWM")),
        }
    }
}

fn ms(d: Duration) -> f64 {
    d.as_secs_f64() * 1000.0
}

fn mib(kib: Option<u64>, out: &mut String) {
    match kib {
        Some(k) => {
            let _ = write!(out, "{:.1} MiB", k as f64 / 1024.0);
        }
        None => out.push('-'),
    }
}

fn cpu(from: Option<u64>, to: Option<u64>, out: &mut String) {
    match (from, to) {
        (Some(a), Some(b)) => {
            let _ = write!(out, "{} ms", b.saturating_sub(a) * 1000 / CLK_TCK);
        }
        _ => out.push('-'),
    }
}

/// One phase's line (I-88): `mortar: time-phases: <ordinal> <name>: wall
/// <ms> ms, cpu <ms> ms, rss <MiB> MiB, peak <MiB> MiB`.
#[must_use]
pub fn phase_line(ordinal: u16, name: &str, start: &Sample, end: &Sample) -> String {
    let mut s = format!(
        "mortar: time-phases: {ordinal} {name}: wall {:.1} ms, cpu ",
        ms(end.at.saturating_duration_since(start.at))
    );
    cpu(start.cpu_ticks, end.cpu_ticks, &mut s);
    s.push_str(", rss ");
    mib(end.rss_kib, &mut s);
    s.push_str(", peak ");
    mib(end.peak_kib, &mut s);
    s
}

/// The total line (I-88).
#[must_use]
pub fn total_line(start: &Sample, end: &Sample) -> String {
    let mut s = format!(
        "mortar: time-phases: total: wall {:.1} ms, cpu ",
        ms(end.at.saturating_duration_since(start.at))
    );
    cpu(start.cpu_ticks, end.cpu_ticks, &mut s);
    s.push_str(", peak ");
    mib(end.peak_kib, &mut s);
    s
}

/// Records phases as they run and prints their lines (I-88, I-89).
#[derive(Debug, Clone)]
pub struct PhaseTimer {
    first: Sample,
    last: Sample,
    lines: Vec<String>,
}

impl PhaseTimer {
    #[must_use]
    pub fn start() -> Self {
        let s = Sample::now();
        Self { first: s, last: s, lines: Vec::new() }
    }

    /// The phase `ordinal name` ended now.
    pub fn phase(&mut self, ordinal: u16, name: &str) {
        let now = Sample::now();
        self.lines.push(phase_line(ordinal, name, &self.last, &now));
        self.last = now;
    }

    /// Every line, then the total.
    #[must_use]
    pub fn finish(mut self) -> Vec<String> {
        let now = Sample::now();
        self.lines.push(total_line(&self.first, &now));
        self.lines
    }
}

/// The terms of I-92, in bytes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Budget {
    /// `R_r` (resident store) or `R_s` (staged store).
    pub resident: u64,
    pub decompressed: u64,
    pub tables: u64,
    pub synth_working: u64,
    pub threads: u64,
    pub window: u64,
    /// `S`: 0 for the resident store.
    pub reloc_buffer: u64,
    pub whole_chunks: u64,
    pub events: u64,
}

impl Budget {
    /// `R + D + T + P + threads × (W + S) + X + E`.
    #[must_use]
    pub fn peak(&self) -> u64 {
        self.resident
            + self.decompressed
            + self.tables
            + self.synth_working
            + self.threads * (self.window + self.reloc_buffer)
            + self.whole_chunks
            + self.events
    }
}

/// `T`'s image terms (I-92): `sym_addr` (8 B per global symbol), the plan
/// (24 B per row), the record index (4 B per record of an unsorted
/// section, 8 B per `REC_SAMPLE` records of a sorted non-resident one),
/// the build-id leaves (`leaf` bytes per window).
#[must_use]
pub fn image_tables(symbols: u64, rows: u64, unsorted_records: u64, sampled_records: u64, windows: u64, leaf: u64) -> u64 {
    8 * symbols + 24 * rows + 4 * unsorted_records + 8 * sampled_records.div_ceil(1 << 12) + leaf * windows
}
