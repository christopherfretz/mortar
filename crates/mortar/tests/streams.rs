//! mortar's standard streams (`spec/cli.md` §3.15, WP-095): a message
//! that cannot be written is dropped and the link keeps its own verdict
//! (S-1); an output asked of a stream that cannot be written is an error
//! by name (S-2); nothing is ever left half-written (S-3). One test per
//! row of `spec/landed/cli/WP-095.md`'s table, on the built binary, with
//! each stream in turn
//!
//! - a pipe whose reader has gone before mortar starts (`EPIPE`; mortar,
//!   as every Rust program, ignores `SIGPIPE`),
//! - a closed descriptor (`2>&-`, `>&-`),
//! - `/dev/full` (`ENOSPC`):
//!
//! the exit status (never a signal, never a core), and what is on disk:
//! the output byte-equal to the same link with its streams read, or none,
//! and never a temporary name (`.<out>.mortar-tmp`, `.<out>.mortar-old`).

use std::ffi::OsString;
use std::fmt::Write as _;
use std::fs::File;
use std::os::unix::process::ExitStatusExt as _;
use std::path::Path;
use std::process::{Command, Stdio};
use std::time::Duration;

use mortar_testutil::target::Arch;
use mortar_testutil::{Outcome, Scratch, run_with, run_with_streams, tools_required};

const MORTAR: &str = env!("CARGO_BIN_EXE_mortar");
const LIMIT: Duration = Duration::from_mins(2);
/// Undefined references, and warnings: many writes, tens of KiB.
const MANY: usize = 300;

/// Which of mortar's streams a case breaks.
#[derive(Clone, Copy, Debug)]
enum Fd {
    Out,
    Err,
}

/// How it is broken.
#[derive(Clone, Copy, Debug)]
enum Sink {
    /// A pipe whose read end was closed before mortar started.
    Gone,
    /// The descriptor is closed (`>&-`, `2>&-`).
    Closed,
    /// `/dev/full`: every write fails with `ENOSPC`.
    Full,
}

const SINKS: [Sink; 3] = [Sink::Gone, Sink::Closed, Sink::Full];

struct Fixture {
    scratch: Scratch,
}

impl Fixture {
    /// `ok.o` (`_start`, a dead `.text.dead`), `u.o` (`_start` calling
    /// `u0`..`u299`, none defined) and `w.o` (`u0`..`u299`, each with a
    /// `.gnu.warning.<name>` section: one `gnu-warning` per reference).
    fn new(label: &str) -> Option<Self> {
        let Some(gcc) = Arch::Aarch64.tool("gcc") else {
            assert!(!tools_required(), "the AArch64 gcc is required");
            return None;
        };
        let scratch = Scratch::new(label);
        let stack = "\t.section .note.GNU-stack,\"\",%progbits\n";
        let ok = format!(
            "\t.text\n\t.globl _start\n_start:\n\tret\n\
             \t.section .text.dead,\"ax\",%progbits\ndead:\n\tret\n{stack}"
        );
        let mut u = String::from("\t.text\n\t.globl _start\n_start:\n");
        let mut w = String::from("\t.text\n");
        for i in 0..MANY {
            let _ = writeln!(u, "\tbl u{i}");
            let _ = write!(
                w,
                "\t.text\n\t.globl u{i}\nu{i}:\n\tret\n\
                 \t.section .gnu.warning.u{i},\"\",%progbits\n\
                 \t.string \"u{i} is deprecated: a warning of some length\"\n"
            );
        }
        u.push_str(stack);
        w.push_str(stack);
        for (name, text) in [("ok", ok), ("u", u), ("w", w)] {
            let src = scratch.write(&format!("{name}.s"), text);
            let o = run_with(
                Command::new(&gcc)
                    .arg("-c")
                    .arg(&src)
                    .arg("-o")
                    .arg(format!("{name}.o"))
                    .current_dir(scratch.path()),
                b"",
                LIMIT,
            );
            assert!(o.success(), "{name}.s: {}", o.stderr_text());
        }
        Some(Self { scratch })
    }

    fn dir(&self) -> &Path {
        self.scratch.path()
    }

    /// The link of `args` with both streams read.
    fn read(&self, args: &[&str], env: &[(&str, &str)]) -> Outcome {
        let mut c = command(self.dir(), args, env);
        run_with(&mut c, b"", LIMIT)
    }

    /// The link of `args` with `fd` broken as `sink` (the other read).
    fn broken(&self, fd: Fd, sink: Sink, args: &[&str], env: &[(&str, &str)]) -> Outcome {
        let stream = match sink {
            Sink::Gone => {
                let (reader, writer) = std::io::pipe().expect("a pipe");
                drop(reader);
                Some(Stdio::from(writer))
            }
            Sink::Full => Some(Stdio::from(
                File::options().write(true).open("/dev/full").expect("/dev/full opens"),
            )),
            Sink::Closed => None,
        };
        let mut c = match sink {
            // The shell closes the descriptor and becomes mortar.
            Sink::Closed => {
                let close = match fd {
                    Fd::Out => ">&-",
                    Fd::Err => "2>&-",
                };
                let mut c = Command::new("sh");
                c.arg("-c").arg(format!("exec \"$0\" \"$@\" {close}")).arg(MORTAR).args(args);
                c.current_dir(self.dir()).env_remove("MORTAR_SCHED_SEED");
                for (k, v) in env {
                    c.env(k, v);
                }
                c
            }
            _ => command(self.dir(), args, env),
        };
        let (out, err) = match fd {
            Fd::Out => (stream.or_else(|| Some(Stdio::null())), None),
            Fd::Err => (None, stream.or_else(|| Some(Stdio::null()))),
        };
        run_with_streams(&mut c, out, err, LIMIT)
    }

    /// The names in the directory that start with `out` or `.out`.
    fn left(&self, out: &str) -> Vec<String> {
        let dot = format!(".{out}");
        let mut names: Vec<String> = std::fs::read_dir(self.dir())
            .expect("the scratch lists")
            .flatten()
            .map(|e| e.file_name().to_string_lossy().into_owned())
            .filter(|n| n.starts_with(out) || n.starts_with(&dot))
            .collect();
        names.sort();
        names
    }
}

fn command(dir: &Path, args: &[&str], env: &[(&str, &str)]) -> Command {
    let mut c = Command::new(MORTAR);
    c.args(args.iter().map(OsString::from)).current_dir(dir).env_remove("MORTAR_SCHED_SEED");
    for (k, v) in env {
        c.env(k, v);
    }
    c
}

/// The process exited with `code`: no signal, no core, not timed out.
fn exited(o: &Outcome, code: i32, what: &str) {
    let status = o.status.unwrap_or_else(|| panic!("{what}: timed out"));
    assert_eq!(status.signal(), None, "{what}: killed by a signal ({status:?})");
    assert!(!status.core_dumped(), "{what}: dumped core");
    assert_eq!(status.code(), Some(code), "{what}: {status:?}\n{}", o.stderr_text());
}

/// A link that succeeds whatever happens to `fd`: the output is the
/// one the same link writes with its streams read, alone on disk
/// (with `extra`, the incremental state).
fn succeeds(f: &Fixture, fd: Fd, args: &[&str], env: &[(&str, &str)], extra: &[&str]) {
    let with = |out: &str| -> Vec<String> {
        args.iter().map(|a| if *a == "OUT" { out.to_owned() } else { (*a).to_owned() }).collect()
    };
    let reference = with("ref.out");
    let reference: Vec<&str> = reference.iter().map(String::as_str).collect();
    let r = f.read(&reference, env);
    exited(&r, 0, &format!("{args:?}, streams read"));
    let expected = std::fs::read(f.dir().join("ref.out")).expect("the reference output");
    for (n, sink) in SINKS.into_iter().enumerate() {
        let out = format!("s{n}.out");
        let line = with(&out);
        let line: Vec<&str> = line.iter().map(String::as_str).collect();
        let what = format!("{args:?} {env:?}, {fd:?} {sink:?}");
        let o = f.broken(fd, sink, &line, env);
        exited(&o, 0, &what);
        let written = std::fs::read(f.dir().join(&out)).unwrap_or_default();
        assert!(written == expected, "{what}: the output differs from the reference");
        let mut want = vec![out.clone()];
        want.extend(extra.iter().map(|e| format!("{out}{e}")));
        want.sort();
        assert_eq!(f.left(&out), want, "{what}");
        for name in want {
            std::fs::remove_file(f.dir().join(name)).expect("the output is removed");
        }
    }
    for name in f.left("ref.out") {
        std::fs::remove_file(f.dir().join(name)).expect("the reference is removed");
    }
}

/// A link that fails whatever happens to `fd`: exit 1, nothing on disk.
fn fails(f: &Fixture, fd: Fd, args: &[&str], env: &[(&str, &str)]) {
    for (n, sink) in SINKS.into_iter().enumerate() {
        let out = format!("f{n}.out");
        let line: Vec<&str> =
            args.iter().map(|a| if *a == "OUT" { out.as_str() } else { a }).collect();
        let what = format!("{args:?} {env:?}, {fd:?} {sink:?}");
        let o = f.broken(fd, sink, &line, env);
        exited(&o, 1, &what);
        assert_eq!(f.left(&out), Vec::<String>::new(), "{what}");
    }
}

// spec: cli.md §3.15 (S-1): 300 errors nobody reads still fail the link,
// with status 1 and no output.
#[test]
fn errors_to_a_broken_stderr() {
    let Some(f) = Fixture::new("streams-errors") else { return };
    fails(&f, Fd::Err, &["-o", "OUT", "u.o"], &[]);
}

// spec: cli.md §3.15 (S-1): 300 warnings nobody reads: the link succeeds.
#[test]
fn warnings_to_a_broken_stderr() {
    let Some(f) = Fixture::new("streams-warnings") else { return };
    let r = f.read(&["-o", "x.out", "u.o", "w.o"], &[]);
    let warnings = r.stderr_text().matches("warning: ").count();
    assert_eq!(warnings, MANY, "one warning per reference:\n{}", r.stderr_text());
    succeeds(&f, Fd::Err, &["-o", "OUT", "u.o", "w.o"], &[], &[]);
}

// spec: cli.md §3.15 (S-1, S-3): `MORTAR_STEPS` (WP-061's SIGABRT): a
// successful link, a failed one and an incremental one.
#[test]
fn steps_to_a_broken_stderr() {
    let Some(f) = Fixture::new("streams-steps") else { return };
    let steps = [("MORTAR_STEPS", "1")];
    succeeds(&f, Fd::Err, &["-o", "OUT", "u.o", "w.o"], &steps, &[]);
    fails(&f, Fd::Err, &["-o", "OUT", "u.o"], &steps);
    succeeds(
        &f,
        Fd::Err,
        &["--incremental", "--incremental-report", "-o", "OUT", "u.o", "w.o"],
        &steps,
        &[".mortar-state"],
    );
}

// spec: cli.md §3.15 (S-1): `--time-phases`, `--print-gc-sections`.
#[test]
fn listings_to_a_broken_stderr() {
    let Some(f) = Fixture::new("streams-listings") else { return };
    let r = f.read(&["--gc-sections", "--print-gc-sections", "-o", "x.out", "ok.o"], &[]);
    assert!(r.stderr_text().contains(".text.dead"), "{}", r.stderr_text());
    succeeds(&f, Fd::Err, &["--gc-sections", "--print-gc-sections", "-o", "OUT", "ok.o"], &[], &[]);
    succeeds(&f, Fd::Err, &["--time-phases", "-o", "OUT", "ok.o"], &[], &[]);
}

// spec: cli.md §3.15 (S-1): the answers of §5 are dropped, status 0.
#[test]
fn answers_to_a_broken_stdout() {
    let Some(f) = Fixture::new("streams-answers") else { return };
    for args in [&["--version"][..], &["--help"], &["-V"], &["-v"]] {
        let r = f.read(args, &[]);
        exited(&r, 0, &format!("{args:?}"));
        assert!(!r.stdout.is_empty(), "{args:?} answers");
        for sink in SINKS {
            exited(&f.broken(Fd::Out, sink, args, &[]), 0, &format!("{args:?} {sink:?}"));
        }
    }
}

// spec: cli.md §3.15 (S-2): `-M`'s map is an output: one that cannot be
// written fails the link by name and leaves no output. A closed
// descriptor is /dev/null (S-4): the link succeeds.
#[test]
fn print_map_to_a_broken_stdout() {
    let Some(f) = Fixture::new("streams-print-map") else { return };
    for (n, sink) in [Sink::Gone, Sink::Full].into_iter().enumerate() {
        let out = format!("m{n}.out");
        let o = f.broken(Fd::Out, sink, &["-M", "-o", &out, "ok.o"], &[]);
        exited(&o, 1, &format!("-M {sink:?}"));
        assert!(
            o.stderr_text().contains("error: cannot write output file standard output (-M): "),
            "{sink:?}: {}",
            o.stderr_text()
        );
        assert_eq!(f.left(&out), Vec::<String>::new(), "-M {sink:?}");
    }
    let o = f.broken(Fd::Out, Sink::Closed, &["-M", "-o", "m2.out", "ok.o"], &[]);
    exited(&o, 0, "-M, stdout closed");
    assert_eq!(f.left("m2.out"), ["m2.out"]);
}

// spec: cli.md §3.15 (S-2), image.md I-79a: a file written whole to
// `/dev/stdout` (WP-087) that cannot be written fails the link by name.
#[test]
fn written_files_to_a_broken_stdout() {
    let Some(f) = Fixture::new("streams-dev-stdout") else { return };
    for option in ["-Map=/dev/stdout", "--mortar-map=/dev/stdout", "--dependency-file=/dev/stdout"]
    {
        for (n, sink) in [Sink::Gone, Sink::Full].into_iter().enumerate() {
            let out = format!("d{n}.out");
            let o = f.broken(Fd::Out, sink, &[option, "-o", &out, "ok.o"], &[]);
            exited(&o, 1, &format!("{option} {sink:?}"));
            assert!(
                o.stderr_text().contains("error: cannot write output file /dev/stdout: "),
                "{option} {sink:?}: {}",
                o.stderr_text()
            );
            assert_eq!(f.left(&out), Vec::<String>::new(), "{option} {sink:?}");
        }
        let o = f.broken(Fd::Out, Sink::Closed, &[option, "-o", "d2.out", "ok.o"], &[]);
        exited(&o, 0, &format!("{option}, stdout closed"));
        assert_eq!(f.left("d2.out"), ["d2.out"]);
        let _ = std::fs::remove_file(f.dir().join("d2.out"));
    }
}
