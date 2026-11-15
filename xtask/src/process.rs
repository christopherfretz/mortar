//! The processes the harness starts. Contract: `spec/harness.md` "Limits"
//! and "Jobserver".
//!
//! Every child of `xtask` is started here, never with a bare `Command`:
//!
//! - [`run`]: a bounded child (a linker, a compiler that builds a test's
//!   inputs, a program under test). A wall-clock limit this process
//!   enforces by killing the child's whole process group, and an
//!   address-space and a CPU-time limit the kernel enforces, which also
//!   hold if this process dies first. A child that exceeded a limit is
//!   reported as killed ([`End::Killed`]), never as an exit status.
//! - [`run_measured`]: the same child, with its peak resident memory and
//!   CPU time taken from GNU `time`.
//! - [`run_step`]: a cargo step, which is a jobserver client of its own and
//!   has no limit.
//!
//! No `unsafe` and no dependency: the limits are set by a `/bin/sh` wrapper
//! that then `exec`s the program, and the kernel's resource usage record of
//! a child, which the standard library cannot read (`wait4`), comes from a
//! program that can.
//!
//! Each bounded child holds one jobserver token for its lifetime
//! ([`crate::jobs`]), and is in the table of live children from spawn to
//! reap, so that an interrupted xtask kills it before the tokens go back
//! ([`crate::jobs::children`]).
//!
//! Ported from hod's `xtask/src/irdiff/process.rs` and
//! `xtask/src/bench/measure.rs`.

use std::fmt;
use std::io::{Read, Write};
use std::os::unix::process::{CommandExt, ExitStatusExt};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::mpsc::{Receiver, RecvTimeoutError, channel};
use std::sync::{Arc, Mutex, PoisonError};
use std::time::{Duration, Instant};

use crate::jobs::children::{self, Stop, kill_group};

pub use crate::jobs::children::run_step;

/// What a child may use.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Limits {
    /// Wall-clock time; the child's process group is killed when it passes.
    pub time: Duration,
    /// Address space, in KiB (`ulimit -v`).
    pub memory_kib: u64,
}

impl Limits {
    /// The CPU-time limit that backs the wall-clock one (`ulimit -t`), in
    /// seconds, for a child that may run on `cpus` processors at once: the
    /// time limit rounded up to a second, plus one, times `cpus + 1`. A
    /// child cannot use that much before its wall-clock limit, whatever
    /// the number of its threads, so the limit ends only a child whose
    /// harness is gone.
    pub fn cpu_seconds_on(self, cpus: u64) -> u64 {
        let wall = self.time.as_secs().saturating_add(u64::from(self.time.subsec_nanos() > 0));
        wall.saturating_add(1).saturating_mul(cpus.saturating_add(1))
    }

    /// [`Limits::cpu_seconds_on`] the processors this process may use.
    pub fn cpu_seconds(self) -> u64 {
        let cpus = std::thread::available_parallelism().map_or(1, std::num::NonZero::get);
        self.cpu_seconds_on(cpus as u64)
    }
}

/// A limit a child exceeded.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Limit {
    /// The wall-clock limit: the harness killed the child's process group.
    Time,
    /// The address-space limit: the kernel refused the child memory and
    /// the child ended saying so ([`refused_memory`]).
    Memory,
}

/// How a child ended.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum End {
    /// It exited with this status.
    Exit(i32),
    /// It was terminated by this signal.
    Signal(i32),
    /// It exceeded a limit. Not a verdict on what it was asked to do: no
    /// expectation of a failure is met by it.
    Killed(Limit),
}

impl End {
    /// Exited with status zero.
    pub fn success(self) -> bool {
        self == Self::Exit(0)
    }

    /// The limit the child exceeded, if it did.
    pub fn killed(self) -> Option<Limit> {
        match self {
            Self::Killed(limit) => Some(limit),
            Self::Exit(_) | Self::Signal(_) => None,
        }
    }
}

/// `exit status 3`, `signal 11`, `killed: time`, `killed: memory`.
impl fmt::Display for End {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Exit(code) => write!(f, "exit status {code}"),
            Self::Signal(signal) => write!(f, "signal {signal}"),
            Self::Killed(Limit::Time) => f.write_str("killed: time"),
            Self::Killed(Limit::Memory) => f.write_str("killed: memory"),
        }
    }
}

/// The resources a measured child used, as GNU `time` reports them.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Usage {
    /// Peak resident set size, KiB.
    pub rss_kib: u64,
    /// User plus system CPU time, microseconds (GNU `time` reports
    /// hundredths of a second).
    pub cpu_us: u64,
}

/// What a child did.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Output {
    pub end: End,
    pub stdout: Vec<u8>,
    pub stderr: Vec<u8>,
    /// More than [`OUTPUT_CAP`] bytes were written to one of the streams;
    /// the rest was discarded.
    pub truncated: bool,
    /// From just before the process was started, with its jobserver token
    /// held, to the return of `wait`: waiting for the pool is not in it.
    pub wall: Duration,
    /// The child's resource usage: of [`run_measured`] with a usable
    /// [`Probe`], for a child that was not killed at the time limit.
    pub usage: Option<Usage>,
}

impl Output {
    /// The first non-empty line of standard error, for a diagnostic.
    pub fn first_error_line(&self) -> String {
        let text = String::from_utf8_lossy(&self.stderr);
        text.lines().map(str::trim).find(|line| !line.is_empty()).unwrap_or("").to_owned()
    }
}

/// The most that is kept of each output stream.
pub const OUTPUT_CAP: usize = 16 << 20;

/// How long the output of a child that has ended is waited for. A
/// descendant that left the process group and holds the pipe open cannot
/// hold the harness longer than this.
const DRAIN_GRACE: Duration = Duration::from_secs(2);

/// How long a killed process group is waited for before it is killed again.
const KILL_AGAIN: Duration = Duration::from_secs(1);

/// The exit status and message of the wrapper when it cannot set a limit.
const SETUP_FAILED: i32 = 125;
const SETUP_MESSAGE: &str = "xtask: cannot set the resource limits";

/// `$1`: the address-space limit in KiB. `$2`: the CPU limit in seconds.
/// The rest: the program and its arguments. Core files are switched off:
/// a test program may be expected to die by a signal.
const WRAPPER: &str = "ulimit -c 0 && ulimit -v \"$1\" && ulimit -t \"$2\" || \
                       { echo 'xtask: cannot set the resource limits' >&2; exit 125; }\n\
                       shift 2\n\
                       exec \"$@\"";

/// Where GNU time is installed.
pub const GNU_TIME: &str = "/usr/bin/time";

/// What GNU time is asked to write: the child's peak resident set size in
/// KiB, and its user and system CPU seconds (two decimals).
const TIME_FORMAT: &str = "%M %U %S";

/// How a child's resource usage is obtained.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Probe {
    /// GNU time, if it is usable; without it only wall time is measured.
    pub time: Option<PathBuf>,
}

impl Probe {
    /// Uses `time` if it accepts `-f <format> -o <file>` and writes three
    /// numbers for a child that does nothing. `scratch` is a writable
    /// directory.
    pub fn detect(time: &Path, scratch: &Path) -> Self {
        let candidate = Self { time: Some(time.to_path_buf()) };
        let limits = Limits { time: Duration::from_secs(20), memory_kib: 1 << 20 };
        let usable = time.is_file()
            && run_measured(
                Path::new("/bin/sh"),
                |c| {
                    c.args(["-c", ":"]);
                },
                b"",
                limits,
                &candidate,
                &scratch.join("probe.usage"),
            )
            .is_ok_and(|output| output.end.success() && output.usage.is_some());
        if usable { candidate } else { Self { time: None } }
    }

    /// How the usage is obtained, in words: `gnu-time (<path> -f '%M %U
    /// %S')`, or `none`.
    pub fn describe(&self) -> String {
        match &self.time {
            Some(time) => format!("gnu-time ({} -f '{TIME_FORMAT}')", time.display()),
            None => "none".to_owned(),
        }
    }
}

/// Runs `program` under `limits` with `stdin` as its input, capturing its
/// output. `configure` adds the arguments, environment and working
/// directory to the command (its `arg` calls append the program's
/// arguments).
///
/// An error is a failure of the harness or the machine, not of the
/// program: the program is missing, the process cannot be started, or a
/// limit cannot be set.
///
/// The child holds one jobserver token from before it starts until it has
/// ended and its output is read; the time limit starts once the token is
/// held. When xtask is being interrupted (SIGINT, SIGTERM) this does not
/// return: no child is started any more, the result of a child the
/// interrupt killed is nobody's to report, and the signal thread ends the
/// process.
pub fn run(
    program: &Path,
    configure: impl FnOnce(&mut Command),
    stdin: &[u8],
    limits: Limits,
) -> Result<Output, String> {
    let result = run_from(program, configure, stdin, limits, None);
    // The token and the table entry are released by now.
    children::park_if_interrupted();
    result
}

/// [`run`] for measurement: the child's peak resident memory and CPU time
/// come from GNU `time` (`probe`), which writes its report to
/// `usage_file`, a scratch path that is removed. The limits are set
/// outside GNU `time`, which then starts the program with them inherited,
/// so its report is the program's alone.
///
/// A measured child is one process: it does not get `MAKEFLAGS` or
/// `CARGO_MAKEFLAGS`, so that it cannot find a jobserver and decide to use
/// more than its share.
pub fn run_measured(
    program: &Path,
    configure: impl FnOnce(&mut Command),
    stdin: &[u8],
    limits: Limits,
    probe: &Probe,
    usage_file: &Path,
) -> Result<Output, String> {
    let result = run_from(program, configure, stdin, limits, Some((probe, usage_file)));
    children::park_if_interrupted();
    result
}

/// [`run`] and [`run_measured`], without the wait for the end of an
/// interrupted xtask.
fn run_from(
    program: &Path,
    configure: impl FnOnce(&mut Command),
    stdin: &[u8],
    limits: Limits,
    measure: Option<(&Probe, &Path)>,
) -> Result<Output, String> {
    if !program.is_file() {
        return Err(format!("cannot run '{}': no such file", program.display()));
    }
    let mut command = Command::new("/bin/sh");
    command.arg("-c").arg(WRAPPER).arg("sh");
    command.arg(limits.memory_kib.to_string()).arg(limits.cpu_seconds().to_string());
    let usage_file = measure.map(|(_, file)| file);
    if let Some((Probe { time: Some(time) }, file)) = measure {
        command.arg(time).args(["-f", TIME_FORMAT, "-o"]).arg(file).arg("--");
    }
    command.arg(program);
    configure(&mut command);
    if measure.is_some() {
        command.env_remove("MAKEFLAGS").env_remove("CARGO_MAKEFLAGS");
    }
    command.stdin(if stdin.is_empty() { Stdio::null() } else { Stdio::piped() });
    command.stdout(Stdio::piped()).stderr(Stdio::piped());
    // Its own process group, so that the time limit reaches everything the
    // child started (the wrapper, GNU time, a compiler driver's linker, a
    // script's commands).
    command.process_group(0);
    if let Some(file) = usage_file {
        let _ = std::fs::remove_file(file);
    }
    let cannot = |e: std::io::Error| format!("cannot run '{}': {e}", program.display());

    // One jobserver slot for the child's lifetime (released on every return
    // path, kill or panic by `Drop`, after the table entry below); the time
    // limit and the clock start once it is held.
    let _slot = crate::jobs::global().acquire();
    let spawning = children::begin_spawn().map_err(cannot)?;
    let start = Instant::now();
    let child = command.spawn();
    // Deregistered when this returns, after the child is reaped.
    let _live = spawning.spawned(child.as_ref().map(Child::id).ok(), Stop::Kill);
    let mut child = child.map_err(cannot)?;
    let group = child.id();

    if let Some(mut pipe) = child.stdin.take() {
        let input = stdin.to_vec();
        // A child that does not read its input closes the pipe when it
        // ends; that is not an error of ours.
        std::thread::spawn(move || {
            let _ = pipe.write_all(&input);
        });
    }
    let stdout = Capture::start(child.stdout.take());
    let stderr = Capture::start(child.stderr.take());

    // A thread blocked in `wait` reads the clock the moment the child is
    // gone; polling would add its interval to every time.
    let (ended, end) = channel();
    std::thread::spawn(move || {
        let status = child.wait();
        let _ = ended.send((status, Instant::now()));
    });
    let gone = || format!("cannot wait for '{}'", program.display());
    let mut timed_out = false;
    let mut next = end.recv_timeout(limits.time.saturating_sub(start.elapsed()));
    let (status, finished) = loop {
        match next {
            Ok(ended) => break ended,
            Err(RecvTimeoutError::Disconnected) => return Err(gone()),
            Err(RecvTimeoutError::Timeout) => {
                // The leader is not reaped until the waiting thread
                // returns, so its process group id is still its own. Again
                // while the group lives: the kill is a process of its own,
                // and one that could not be started killed nothing.
                timed_out = true;
                kill_group(group);
                next = end.recv_timeout(KILL_AGAIN);
            }
        }
    };
    let drained = Instant::now() + DRAIN_GRACE;
    if !(stdout.wait(drained) && stderr.wait(drained)) {
        // Something still holds the pipes: what is left of the group goes.
        kill_group(group);
    }
    let (stdout, out_truncated) = stdout.take();
    let (stderr, err_truncated) = stderr.take();
    let report = usage_file.map(|file| {
        let text = std::fs::read_to_string(file).unwrap_or_default();
        let _ = std::fs::remove_file(file);
        parse_usage(&text)
    });
    let wall = finished.duration_since(start);
    let truncated = out_truncated || err_truncated;
    if timed_out {
        let end = End::Killed(Limit::Time);
        return Ok(Output { end, stdout, stderr, truncated, wall, usage: None });
    }

    let status = status.map_err(|e| format!("{}: {e}", gone()))?;
    let mut end = match (status.code(), status.signal()) {
        (Some(code), _) => End::Exit(code),
        (None, Some(signal)) => End::Signal(signal),
        (None, None) => return Err(format!("'{}' ended without a status", program.display())),
    };
    if end == End::Exit(SETUP_FAILED) && stderr.starts_with(SETUP_MESSAGE.as_bytes()) {
        return Err(format!(
            "cannot limit '{}' to {} KiB of address space and {} s of CPU time",
            program.display(),
            limits.memory_kib,
            limits.cpu_seconds()
        ));
    }
    // GNU time exits with the signal's number when the program was
    // terminated by one, and says so in its report.
    if let Some(signal) = report.and_then(|report| report.signal) {
        end = End::Signal(signal);
    }
    if !end.success() && refused_memory(&stderr) {
        end = End::Killed(Limit::Memory);
    }
    let usage = report.and_then(|report| report.usage);
    Ok(Output { end, stdout, stderr, truncated, wall, usage })
}

/// The marks of a refused allocation in a child's standard error
/// ([`refused_memory`]): what the tools the harness runs write when the
/// kernel refuses them memory.
const REFUSED_MEMORY: [&str; 7] = [
    // The dynamic loader, for a program or a library that cannot be mapped.
    "failed to map segment from shared object",
    // `strerror(ENOMEM)`: a failed `mmap`, `fork` or `exec` as any C
    // program reports it.
    "Cannot allocate memory",
    // BFD ("final link failed: memory exhausted"), gnulib's `xalloc_die`,
    // GCC's "virtual memory exhausted".
    "memory exhausted",
    // libiberty's `xmalloc` (GNU ld, GNU as, GCC): "out of memory
    // allocating N bytes after a total of M bytes".
    "out of memory allocating",
    // LLVM's allocation failure handler (lld, clang).
    "LLVM ERROR: out of memory",
    // An uncaught C++ allocation failure, as libstdc++ and as libc++abi
    // name it before they abort. (The bare name of the exception is not a
    // mark: a linker prints it in a diagnostic about its symbols.)
    "throwing an instance of 'std::bad_alloc'",
    "uncaught exception of type std::bad_alloc",
];

/// Did a child that wrote `stderr` say that it was refused memory?
///
/// The kernel enforces the address-space limit by refusing the allocation
/// that would exceed it, and does not record that it did: what is left to
/// see is what the child wrote before it gave up. This recognizes a line
/// with one of the [`REFUSED_MEMORY`] marks, one that ends Rust's `memory
/// allocation of N bytes failed`, or one that starts with Python's
/// `MemoryError`. A child that was refused memory and ended without
/// such a line (a null pointer it did not check: a segmentation fault) is
/// reported by its exit status or signal.
pub fn refused_memory(stderr: &[u8]) -> bool {
    let text = String::from_utf8_lossy(stderr);
    text.lines().any(|line| {
        REFUSED_MEMORY.iter().any(|mark| line.contains(mark))
            || line.starts_with("MemoryError")
            || line
                .split_once("memory allocation of ")
                .is_some_and(|(_, rest)| rest.trim_end().ends_with(" bytes failed"))
    })
}

/// What GNU time's report says.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
struct Report {
    usage: Option<Usage>,
    signal: Option<i32>,
}

/// Reads GNU time's report: an optional line `Command exited with
/// non-zero status N` or `Command terminated by signal N`, then the
/// formatted line, `<KiB> <user seconds> <system seconds>`.
fn parse_usage(text: &str) -> Report {
    let mut report = Report::default();
    for line in text.lines() {
        if let Some(signal) = line.strip_prefix("Command terminated by signal ") {
            report.signal = signal.trim().parse().ok();
        }
    }
    let last = text.lines().rev().find(|line| !line.trim().is_empty()).unwrap_or("");
    let fields: Vec<&str> = last.split_whitespace().collect();
    if let [rss, user, system] = fields[..] {
        let cpu_us = seconds_to_us(user).zip(seconds_to_us(system)).map(|(u, s)| u + s);
        report.usage =
            rss.parse().ok().zip(cpu_us).map(|(rss_kib, cpu_us)| Usage { rss_kib, cpu_us });
    }
    report
}

/// `12.34` seconds as microseconds; at most six decimals.
fn seconds_to_us(text: &str) -> Option<u64> {
    let (whole, fraction) = text.split_once('.').unwrap_or((text, ""));
    let digits = |s: &str| s.bytes().all(|b| b.is_ascii_digit());
    if whole.is_empty() || !digits(whole) || !digits(fraction) || fraction.len() > 6 {
        return None;
    }
    let scale = 10u64.pow(6 - fraction.len() as u32);
    let fraction = if fraction.is_empty() { 0 } else { fraction.parse::<u64>().ok()? * scale };
    whole.parse::<u64>().ok()?.checked_mul(1_000_000)?.checked_add(fraction)
}

/// One output stream being read on its own thread.
struct Capture {
    kept: Arc<Mutex<(Vec<u8>, bool)>>,
    done: Receiver<()>,
}

impl Capture {
    fn start(pipe: Option<impl Read + Send + 'static>) -> Self {
        let kept = Arc::new(Mutex::new((Vec::new(), false)));
        let (finished, done) = channel();
        if let Some(mut pipe) = pipe {
            let kept = Arc::clone(&kept);
            std::thread::spawn(move || {
                let mut chunk = vec![0; 1 << 16];
                loop {
                    match pipe.read(&mut chunk) {
                        Ok(0) | Err(_) => break,
                        Ok(n) => {
                            let mut kept = kept.lock().unwrap_or_else(PoisonError::into_inner);
                            let room = OUTPUT_CAP - kept.0.len();
                            kept.0.extend_from_slice(&chunk[..n.min(room)]);
                            kept.1 |= n > room;
                        }
                    }
                }
                let _ = finished.send(());
            });
        }
        Self { kept, done }
    }

    /// Whether the stream ended before `deadline`.
    fn wait(&self, deadline: Instant) -> bool {
        // A reader that is gone has dropped its sender: the stream ended.
        !matches!(
            self.done.recv_timeout(deadline.saturating_duration_since(Instant::now())),
            Err(RecvTimeoutError::Timeout)
        )
    }

    /// What was read so far, and whether anything was discarded.
    fn take(&self) -> (Vec<u8>, bool) {
        let mut kept = self.kept.lock().unwrap_or_else(PoisonError::into_inner);
        (std::mem::take(&mut kept.0), kept.1)
    }
}

// spec/harness.md "Limits". Ported from hod's tests of the same names
// (`irdiff::process`, `bench::measure`).
#[cfg(test)]
mod tests {
    use super::*;
    use mortar_testutil::Scratch;

    const ROOMY: Limits = Limits { time: Duration::from_secs(20), memory_kib: 1 << 20 };

    fn sh(script: &str, stdin: &[u8], limits: Limits) -> Output {
        run_from(
            Path::new("/bin/sh"),
            |c| {
                c.arg("-c").arg(script);
            },
            stdin,
            limits,
            None,
        )
        .unwrap()
    }

    fn measured(script: &str, probe: &Probe, scratch: &Scratch, limits: Limits) -> Output {
        run_measured(
            Path::new("/bin/sh"),
            |c| {
                c.arg("-c").arg(script);
            },
            b"",
            limits,
            probe,
            &scratch.join("usage"),
        )
        .unwrap()
    }

    // "Limits", "What a child gets": output, input, status.
    #[test]
    fn captures_output_and_status() {
        let output = sh("cat; echo err >&2; exit 3", b"input", ROOMY);
        assert_eq!(output.end, End::Exit(3));
        assert_eq!(output.stdout, b"input");
        assert_eq!(output.stderr, b"err\n");
        assert!(!output.truncated && output.usage.is_none());
        assert_eq!(output.first_error_line(), "err");
        assert!(!output.end.success() && output.end.killed().is_none());
        assert!(sh("exit 0", b"", ROOMY).end.success());

        // Arguments, environment and directory come from `configure`.
        let scratch = Scratch::new("process");
        let output = run(
            Path::new("/bin/sh"),
            |c| {
                c.args(["-c", "echo \"$0 $1 $V\"; pwd", "zero", "one"]);
                c.env("V", "value").current_dir(scratch.path());
            },
            b"",
            ROOMY,
        )
        .unwrap();
        let dir = scratch.path().canonicalize().unwrap();
        assert_eq!(output.stdout, format!("zero one value\n{}\n", dir.display()).as_bytes());
    }

    // "Limits", "How a child ended": a signal is told from an exit status.
    #[test]
    fn reports_a_signal() {
        assert_eq!(sh("kill -SEGV $$", b"", ROOMY).end, End::Signal(11));
        assert_eq!(sh("exit 139", b"", ROOMY).end, End::Exit(139));
        let texts = [
            (End::Exit(3), "exit status 3"),
            (End::Signal(11), "signal 11"),
            (End::Killed(Limit::Time), "killed: time"),
            (End::Killed(Limit::Memory), "killed: memory"),
        ];
        for (end, text) in texts {
            assert_eq!(end.to_string(), text);
        }
    }

    // "Limits", "How a child ended": a child that sleeps past its time
    // limit is killed: time, with its whole process group, and what it
    // wrote before is kept.
    #[test]
    fn time_limit_kills_the_group() {
        let scratch = Scratch::new("process-timeout");
        let marker = scratch.join("grandchild-survived");
        let script = format!(
            "echo started; (sleep 3; echo late; touch '{}') & sleep 60; echo never",
            marker.display()
        );
        let limits = Limits { time: Duration::from_millis(400), ..ROOMY };
        let output = sh(&script, b"", limits);
        assert_eq!(output.end, End::Killed(Limit::Time));
        assert_eq!(output.end.killed(), Some(Limit::Time));
        assert_eq!(output.stdout, b"started\n");
        assert!(
            output.wall >= limits.time && output.wall < Duration::from_secs(3),
            "{:?}",
            output.wall
        );
        // The background child of the script died with it.
        std::thread::sleep(Duration::from_millis(3200).saturating_sub(output.wall));
        assert!(!marker.exists(), "a grandchild outlived the time limit");
    }

    // "Limits", "What a child gets": a child that never reads its input,
    // and one that leaves a descendant holding its output, do not hold the
    // harness.
    #[test]
    fn never_hangs_on_pipes() {
        let big = vec![b'x'; 1 << 20];
        assert_eq!(sh("exit 0", &big, ROOMY).end, End::Exit(0));

        let started = Instant::now();
        let output = sh("echo first; (sleep 30 &) ; exit 4", b"", ROOMY);
        assert_eq!(output.end, End::Exit(4));
        assert_eq!(output.stdout, b"first\n");
        // The time is the child's, not the wait for its output.
        assert!(output.wall < Duration::from_secs(15), "{:?}", output.wall);
        // A hang guard: the wait for a token of the machine's pool is in it.
        assert!(started.elapsed() < Duration::from_mins(10), "{:?}", started.elapsed());
    }

    // "Limits", "What a child gets": the kernel's limits are in force in
    // the child, and core files are off. The CPU limit is above what any
    // number of threads can use within the wall-clock limit.
    #[test]
    fn limits_are_set_in_the_child() {
        let limits = Limits { time: Duration::from_millis(2500), memory_kib: 123_456 };
        assert_eq!(limits.cpu_seconds_on(1), 8);
        assert_eq!(limits.cpu_seconds_on(12), 52);
        assert_eq!(Limits { time: Duration::from_secs(10), memory_kib: 1 }.cpu_seconds_on(3), 44);
        assert_eq!(
            Limits { time: Duration::MAX, memory_kib: 1 }.cpu_seconds_on(u64::MAX),
            u64::MAX
        );
        let cpu = limits.cpu_seconds();
        assert!(cpu >= 8 && cpu.is_multiple_of(4), "{cpu}");
        let output = sh("ulimit -v; ulimit -t; ulimit -c", b"", limits);
        assert_eq!(output.stdout, format!("123456\n{cpu}\n0\n").as_bytes());
        // They are inherited by what the child starts.
        let output = sh("sh -c 'ulimit -v'", b"", limits);
        assert_eq!(output.stdout, b"123456\n");
    }

    // "Limits", "How a child ended": the address-space limit fires, and a
    // child that says so is killed: memory, not an ordinary failure. `dd`
    // asks for a 256 MiB buffer and is refused.
    #[test]
    fn memory_limit_fires() {
        let script = "dd if=/dev/zero of=/dev/null bs=268435456 count=1 2>&1; echo \"status $?\"";
        let within = sh(script, b"", ROOMY);
        assert!(within.stdout.ends_with(b"status 0\n"), "{within:?}");
        let limits = Limits { memory_kib: 64 << 10, ..ROOMY };
        let refused = sh(script, b"", limits);
        assert!(refused.stdout.ends_with(b"status 1\n"), "{refused:?}");
        // The script went on and exited with status 0: it was not killed.
        assert_eq!(refused.end, End::Exit(0));

        let dd = "exec dd if=/dev/zero of=/dev/null bs=268435456 count=1";
        assert_eq!(sh(dd, b"", ROOMY).end, End::Exit(0));
        let killed = sh(dd, b"", limits);
        assert_eq!(killed.end, End::Killed(Limit::Memory), "{killed:?}");
        assert_eq!(killed.end.to_string(), "killed: memory");
        // A limit below what the program needs to start at all: the
        // dynamic loader says so, or the program dies before it can.
        let tiny = Limits { memory_kib: 1 << 10, ..ROOMY };
        let killed = sh("exit 0", b"", tiny);
        assert!(matches!(killed.end, End::Killed(Limit::Memory) | End::Signal(_)), "{killed:?}");
    }

    // "Limits", "How a child ended": the marks of a refused allocation,
    // each as its tool writes it; a child that succeeded is never killed,
    // whatever it wrote.
    #[test]
    fn refused_memory_is_recognized_by_what_the_child_wrote() {
        for said in [
            "sh: error while loading shared libraries: libc.so.6: failed to map segment from \
             shared object",
            "ld.bfd: can not create hash table: memory exhausted",
            "/usr/bin/ld: final link failed: memory exhausted",
            "dd: memory exhausted by input buffer of size 268435456 bytes (256 MiB)",
            "cc1plus: out of memory allocating 65536 bytes after a total of 1171456 bytes",
            "LLVM ERROR: out of memory",
            "terminate called after throwing an instance of 'std::bad_alloc'",
            "libc++abi: terminating due to uncaught exception of type std::bad_alloc",
            "memory allocation of 4294967296 bytes failed",
            "mold: fatal: mmap failed: Cannot allocate memory",
            "MemoryError",
        ] {
            assert!(refused_memory(format!("warning: w\n{said}\n").as_bytes()), "{said}");
        }
        for said in [
            "",
            "error: undefined symbol: main",
            "ld: region `ROM' overflowed by 16 bytes",
            "error: cannot allocate memory in static TLS block",
            "ld: a.o: undefined reference to `typeinfo for std::bad_alloc'",
            "ld.lld: error: undefined symbol: MemoryError::what() const",
            "note: the memory allocation of the section failed",
            "Segmentation fault",
        ] {
            assert!(!refused_memory(said.as_bytes()), "{said}");
        }
        let output = sh("echo 'LLVM ERROR: out of memory' >&2; exit 0", b"", ROOMY);
        assert_eq!(output.end, End::Exit(0));
        let output = sh("echo 'LLVM ERROR: out of memory' >&2; kill -ABRT $$", b"", ROOMY);
        assert_eq!(output.end, End::Killed(Limit::Memory));
        // A crash without a word is a crash.
        assert_eq!(sh("kill -ABRT $$", b"", ROOMY).end, End::Signal(6));
    }

    // "Limits", "What a child gets": output beyond the cap is discarded,
    // not stored.
    #[test]
    fn output_is_capped() {
        let output = sh("dd if=/dev/zero bs=1048576 count=20 2>/dev/null", b"", ROOMY);
        assert_eq!(output.end, End::Exit(0));
        assert_eq!(output.stdout.len(), OUTPUT_CAP);
        assert!(output.truncated);
    }

    // "Limits", "Errors of the harness": what is the harness's failure,
    // not the child's.
    #[test]
    fn harness_errors() {
        let missing = run(Path::new("/nonexistent/tool"), |_| {}, b"", ROOMY).unwrap_err();
        assert!(missing.contains("cannot run '/nonexistent/tool'"), "{missing}");

        // A limit that cannot be set (not a number) is reported as such.
        let limits = Limits { memory_kib: u64::MAX, ..ROOMY };
        match run(
            Path::new("/bin/sh"),
            |c| {
                c.args(["-c", "exit 0"]);
            },
            b"",
            limits,
        ) {
            // Some shells accept any number as "unlimited enough".
            Ok(output) => assert_eq!(output.end, End::Exit(0)),
            Err(message) => assert!(message.contains("cannot limit"), "{message}"),
        }
        // A program that exits with the wrapper's status is not mistaken
        // for a setup failure.
        assert_eq!(sh("echo oops >&2; exit 125", b"", ROOMY).end, End::Exit(125));
    }

    /// A stand-in for GNU time: runs the command, then writes what the
    /// environment says to the `-o` file, as GNU time would.
    const FAKE_TIME: &str = r#"PATH=/usr/bin:/bin
out=
while [ $# -gt 0 ]; do
  case "$1" in
    -f) shift ;;
    -o) out=$2; shift ;;
    --) shift; break ;;
  esac
  shift
done
"$@"
status=$?
if [ $status -gt 128 ]; then
  echo "Command terminated by signal $((status - 128))" > "$out"; status=$((status - 128))
elif [ $status -ne 0 ]; then
  echo "Command exited with non-zero status $status" > "$out"
else
  : > "$out"
fi
echo "${FAKE_RSS:-1234} ${FAKE_USER:-0.25} ${FAKE_SYS:-0.05}" >> "$out"
exit $status"#;

    // "Limits", "Measured children": GNU time's report is the peak memory
    // and the CPU seconds, after an optional status line.
    #[test]
    fn usage_report_parsing() {
        let usage = |rss_kib, cpu_us| Some(Usage { rss_kib, cpu_us });
        let report = parse_usage("67128 0.41 0.07\n");
        assert_eq!(report, Report { usage: usage(67128, 480_000), signal: None });
        let failed = parse_usage("Command exited with non-zero status 3\n700 0.00 0.00\n");
        assert_eq!(failed, Report { usage: usage(700, 0), signal: None });
        let killed = parse_usage("Command terminated by signal 11\n700 1.50 2\n\n");
        assert_eq!(killed, Report { usage: usage(700, 3_500_000), signal: Some(11) });
        assert_eq!(parse_usage(""), Report::default());
        assert_eq!(parse_usage("cannot run x\n"), Report::default());
        assert_eq!(parse_usage("12 0.1\n"), Report::default());
        assert_eq!(parse_usage("12 a 0.1\n").usage, None);
        assert_eq!(seconds_to_us("0.01"), Some(10_000));
        assert_eq!(seconds_to_us("12"), Some(12_000_000));
        assert_eq!(seconds_to_us("1.000001"), Some(1_000_001));
        for bad in ["", ".5", "1.0000001", "-1", "1e3"] {
            assert_eq!(seconds_to_us(bad), None, "{bad}");
        }
    }

    // "Limits", "Measured children": wall time, the exit status, both
    // output streams, and the usage figures of the probe; the child's
    // environment and directory.
    #[test]
    fn a_measured_run() {
        let scratch = Scratch::new("measure");
        let probe = Probe { time: Some(scratch.write_script("time", FAKE_TIME)) };
        let script = "sleep 0.2; echo out; echo diagnostic >&2; exit 3";
        let output = measured(script, &probe, &scratch, ROOMY);
        assert_eq!(output.end, End::Exit(3));
        assert!(
            output.wall >= Duration::from_millis(200) && output.wall < Duration::from_secs(10),
            "{output:?}"
        );
        assert_eq!(output.usage, Some(Usage { rss_kib: 1234, cpu_us: 300_000 }));
        assert_eq!(
            (output.stdout.as_slice(), output.first_error_line().as_str()),
            (&b"out\n"[..], "diagnostic")
        );
        assert!(!scratch.join("usage").exists(), "the usage file is removed");

        let output = run_measured(
            Path::new("/bin/sh"),
            |c| {
                c.args([
                    "-c",
                    "cat; echo \"$V ${MAKEFLAGS:-none} ${CARGO_MAKEFLAGS:-none}\"; pwd; ulimit -v",
                ]);
                c.env("V", "value")
                    .env("MAKEFLAGS", "--jobserver-auth=fifo:/x")
                    .env("CARGO_MAKEFLAGS", "-j");
                c.env("FAKE_RSS", "99").current_dir(scratch.path());
            },
            b"input\n",
            Limits { memory_kib: 654_321, ..ROOMY },
            &probe,
            &scratch.join("usage"),
        )
        .unwrap();
        let dir = scratch.path().canonicalize().unwrap();
        // The jobserver is not handed to a measured child; the limits are.
        assert_eq!(
            String::from_utf8_lossy(&output.stdout),
            format!("input\nvalue none none\n{}\n654321\n", dir.display())
        );
        assert_eq!(output.usage.map(|usage| usage.rss_kib), Some(99));

        // Without a probe: wall time only.
        let bare = measured("exit 0", &Probe { time: None }, &scratch, ROOMY);
        assert_eq!((bare.end, bare.usage), (End::Exit(0), None));
        assert!(bare.wall > Duration::ZERO);
    }

    // "Limits", "Measured children": a signal is told from an exit
    // status, also through the probe; a child at the time limit is killed
    // with its group and has no usage figures.
    #[test]
    fn signals_and_the_time_limit_of_a_measured_run() {
        let scratch = Scratch::new("measure-end");
        let probe = Probe { time: Some(scratch.write_script("time", FAKE_TIME)) };
        assert_eq!(measured("kill -SEGV $$", &probe, &scratch, ROOMY).end, End::Signal(11));
        assert_eq!(measured("exit 11", &probe, &scratch, ROOMY).end, End::Exit(11));
        let bare = Probe { time: None };
        assert_eq!(measured("kill -SEGV $$", &bare, &scratch, ROOMY).end, End::Signal(11));

        let marker = scratch.join("survived");
        let script = format!("(sleep 2; touch '{}') & sleep 60", marker.display());
        let limits = Limits { time: Duration::from_millis(300), ..ROOMY };
        let output = measured(&script, &probe, &scratch, limits);
        assert_eq!((output.end, output.usage), (End::Killed(Limit::Time), None));
        assert!(output.wall < Duration::from_secs(2), "{:?}", output.wall);
        std::thread::sleep(Duration::from_millis(2200).saturating_sub(output.wall));
        assert!(!marker.exists(), "a grandchild outlived the time limit");
    }

    // "Limits", "Measured children": the probe is used only if it works.
    #[test]
    fn probe_detection() {
        let scratch = Scratch::new("measure-probe");
        let none = Probe { time: None };
        let good = scratch.write_script("time", FAKE_TIME);
        assert_eq!(Probe::detect(&good, scratch.path()), Probe { time: Some(good.clone()) });
        assert!(Probe::detect(&good, scratch.path()).describe().starts_with("gnu-time ("));
        let silent = scratch.write_script(
            "silent-time",
            "while [ \"$1\" != -- ]; do shift; done; shift; exec \"$@\"",
        );
        assert_eq!(Probe::detect(&silent, scratch.path()), none);
        assert_eq!(Probe::detect(&scratch.join("absent"), scratch.path()), none);
        assert_eq!(none.describe(), "none");
        let missing = run_measured(
            Path::new("/nonexistent/tool"),
            |_| {},
            b"",
            ROOMY,
            &none,
            &scratch.join("u"),
        );
        assert!(missing.unwrap_err().contains("cannot run '/nonexistent/tool'"));
    }

    // "Limits", "Measured children": with the real GNU time, a child that
    // allocates and touches 64 MiB more than another is measured within 1%
    // of that, and its CPU time is its own.
    #[test]
    fn real_probe_measures_a_known_allocation() {
        const KNOWN_KIB: u64 = 64 << 10;
        let scratch = Scratch::new("measure-real");
        let probe = Probe::detect(Path::new(GNU_TIME), scratch.path());
        if probe.time.is_none() {
            assert!(
                !mortar_testutil::tools_required(),
                "GNU time is required ({GNU_TIME}; MORTAR_REQUIRE_TOOLS=1)"
            );
            eprintln!("note: {GNU_TIME} is not GNU time; skipping the memory probe's self-check");
            return;
        }
        let dd = |block_kib: u64| {
            let script =
                format!("exec dd if=/dev/zero of=/dev/null count=1 bs={}", block_kib * 1024);
            let output = measured(&script, &probe, &scratch, ROOMY);
            assert_eq!(output.end, End::Exit(0), "{output:?}");
            output.usage.expect("the probe reports").rss_kib
        };
        let (small, large) = (dd(4), dd(4 + KNOWN_KIB));
        let error = large.abs_diff(small).abs_diff(KNOWN_KIB);
        assert!(
            large > small && error <= KNOWN_KIB / 100,
            "measured {small} and {large} KiB for a 64 MiB difference"
        );

        // CPU time is the child's: a busy loop of a quarter second or so.
        let busy = "i=0; while [ $i -lt 200000 ]; do i=$((i+1)); done";
        let busy = measured(busy, &probe, &scratch, ROOMY);
        assert_eq!(busy.end, End::Exit(0));
        let (cpu, wall) =
            (busy.usage.unwrap().cpu_us, u64::try_from(busy.wall.as_micros()).unwrap());
        assert!(cpu >= 10_000 && cpu <= wall + 20_000, "cpu {cpu} us, wall {wall} us");
    }
}
