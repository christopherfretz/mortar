//! A path through a descriptor table is written in place or refused by
//! name, never renamed over or removed (`spec/image.md` I-58b, WP-114),
//! whatever descriptor 1 is when mortar starts.
//!
//! Every link here writes through a symbolic link of the test's own,
//! `L -> /proc/self/fd/1` (and the chain `L2 -> /dev/stdout`), in its
//! scratch directory: a linker that replaced the link would replace the
//! test's file, never the container's `/dev/stdout`. Descriptor 1 is in
//! each state of `spec/landed/image/WP-114.md`'s table; each written
//! option is given the link in turn; afterwards the link is still a
//! symbolic link to its target and nothing else is in the directory.
//! The state that replaced the link before WP-114 (a descriptor of no file
//! type: an eventfd, made by `python3`), and the closed descriptor, are
//! run 100 times each, eight links at a time.

use std::fs::File;
use std::os::unix::fs::symlink;
use std::os::unix::process::ExitStatusExt as _;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

use mortar_testutil::target::Arch;
use mortar_testutil::{Outcome, Scratch, run_with, run_with_streams, tool, tools_required};

const MORTAR: &str = env!("CARGO_BIN_EXE_mortar");
const LIMIT: Duration = Duration::from_mins(1);

/// What descriptor 1 is when mortar starts.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum State {
    /// A regular file.
    File,
    /// A regular file opened `O_APPEND`.
    Append,
    /// A regular file removed after it was opened.
    Deleted,
    /// `/dev/null`.
    Null,
    /// `/dev/full`.
    Full,
    /// A pipe whose reader reads.
    Pipe,
    /// A pipe whose reader has gone.
    Gone,
    /// One end of a socket pair.
    Socket,
    /// A directory, opened read-only.
    Directory,
    /// Closed by the shell (`>&-`); Rust's start-up opens `/dev/null`.
    Closed,
    /// An eventfd (`python3`): a descriptor of no file type.
    NoType,
}

const STATES: [State; 11] = [
    State::File,
    State::Append,
    State::Deleted,
    State::Null,
    State::Full,
    State::Pipe,
    State::Gone,
    State::Socket,
    State::Directory,
    State::Closed,
    State::NoType,
];

/// The written options, each naming `LINK`.
const OPTIONS: [&[&str]; 6] = [
    &["-Map=LINK", "-o", "x.out"],
    &["--mortar-map=LINK", "-o", "x.out"],
    &["--dependency-file=LINK", "-o", "x.out"],
    &["-o", "LINK"],
    &["--incremental", "--incremental-state=LINK", "-o", "x.out"],
    &["--incremental", "--incremental-report=LINK", "-o", "x.out"],
];

/// `ok.o`, assembled once: `_start` alone.
struct Fixture {
    scratch: Scratch,
    object: PathBuf,
    python: Option<PathBuf>,
}

impl Fixture {
    fn new(label: &str) -> Option<Self> {
        let Some(gcc) = Arch::Aarch64.tool("gcc") else {
            assert!(!tools_required(), "the AArch64 gcc is required");
            return None;
        };
        let scratch = Scratch::new(label);
        let src = scratch.write(
            "ok.s",
            "\t.text\n\t.globl _start\n_start:\n\tret\n\t.section .note.GNU-stack,\"\",%progbits\n",
        );
        let object = scratch.join("ok.o");
        let o = run_with(Command::new(&gcc).arg("-c").arg(&src).arg("-o").arg(&object), b"", LIMIT);
        assert!(o.success(), "ok.s: {}", o.stderr_text());
        let python = tool("python3");
        assert!(python.is_some() || !tools_required(), "python3 is required (an eventfd)");
        Some(Self { scratch, object, python })
    }
}

/// One link: `option` naming `link` (`L` or `L2`) in a directory of its
/// own, with descriptor 1 in `state`. `None` when the state needs a tool
/// that is missing.
fn link(
    fixture: &Fixture,
    run: usize,
    state: State,
    option: &[&str],
    chain: bool,
) -> Option<Outcome> {
    let dir = fixture.scratch.join(format!("r{run}"));
    std::fs::create_dir(&dir).expect("the run's directory");
    std::fs::copy(&fixture.object, dir.join("ok.o")).expect("ok.o copied");
    let (name, target) = if chain { ("L2", "/dev/stdout") } else { ("L", "/proc/self/fd/1") };
    symlink(target, dir.join(name)).expect("the scratch link");
    let mut args: Vec<String> = option.iter().map(|a| a.replace("LINK", name)).collect::<Vec<_>>();
    args.push("ok.o".to_owned());
    let mut keep = None;
    let mut command = match state {
        State::Closed => {
            let mut c = Command::new("sh");
            c.arg("-c").arg("exec \"$0\" \"$@\" >&-").arg(MORTAR).args(&args);
            c
        }
        State::NoType => {
            let python = fixture.python.as_ref()?;
            let mut c = Command::new(python);
            c.arg("-I")
                .arg("-c")
                .arg(
                    "import os, sys\n\
                     try:\n    fd = os.eventfd(0)\n\
                     except AttributeError:\n    import select; e = select.epoll(); fd = e.fileno()\n\
                     os.dup2(fd, 1)\nos.execv(sys.argv[1], sys.argv[1:])\n",
                )
                .arg(MORTAR)
                .args(&args);
            c
        }
        _ => {
            let mut c = Command::new(MORTAR);
            c.args(&args);
            c
        }
    };
    command.current_dir(&dir).env_remove("MORTAR_SCHED_SEED");
    let out = match state {
        State::File => Some(Stdio::from(File::create(dir.join("fd1")).expect("fd1"))),
        State::Append => Some(Stdio::from(
            File::options().create(true).append(true).open(dir.join("fd1")).expect("fd1"),
        )),
        State::Deleted => {
            let file = File::create(dir.join("fd1")).expect("fd1");
            std::fs::remove_file(dir.join("fd1")).expect("fd1 removed");
            Some(Stdio::from(file))
        }
        State::Null | State::Closed | State::NoType => Some(Stdio::null()),
        State::Full => {
            Some(Stdio::from(File::options().write(true).open("/dev/full").expect("/dev/full")))
        }
        State::Pipe => None,
        State::Gone => {
            let (reader, writer) = std::io::pipe().expect("a pipe");
            drop(reader);
            Some(Stdio::from(writer))
        }
        State::Socket => {
            let (ours, theirs) = std::os::unix::net::UnixStream::pair().expect("a socket pair");
            keep = Some(theirs);
            Some(Stdio::from(std::os::fd::OwnedFd::from(ours)))
        }
        State::Directory => Some(Stdio::from(File::open(&dir).expect("the directory opens"))),
    };
    let outcome = run_with_streams(&mut command, out, None, LIMIT);
    drop(keep);
    let what = format!("{state:?} {args:?}");
    let status = outcome.status.unwrap_or_else(|| panic!("{what}: timed out (a hang)"));
    assert_eq!(status.signal(), None, "{what}: killed by a signal");
    assert!(matches!(status.code(), Some(0 | 1)), "{what}: {status:?}\n{}", outcome.stderr_text());
    // I-58b: the link is the same link, and nothing was created beside it.
    let to = std::fs::read_link(dir.join(name));
    assert!(
        to.as_deref().is_ok_and(|t| t == Path::new(target)),
        "{what}: the scratch link {name} -> {target} was replaced or removed ({to:?})\n{}",
        outcome.stderr_text()
    );
    let mut left: Vec<String> = std::fs::read_dir(&dir)
        .expect("the run's directory lists")
        .flatten()
        .map(|e| e.file_name().to_string_lossy().into_owned())
        .filter(|n| n.starts_with('.'))
        .collect();
    left.sort();
    assert_eq!(left, Vec::<String>::new(), "{what}: litter beside the link");
    std::fs::remove_dir_all(&dir).expect("the run's directory is removed");
    Some(outcome)
}

/// Runs `jobs` eight at a time (each holds a jobserver token while its
/// link runs).
fn parallel(
    jobs: &[(State, &[&str], bool)],
    f: &Fixture,
    check: impl Fn(State, &[&str], &Outcome) + Sync,
) {
    let next = AtomicUsize::new(0);
    std::thread::scope(|s| {
        for _ in 0..8 {
            s.spawn(|| {
                loop {
                    let i = next.fetch_add(1, Ordering::Relaxed);
                    let Some(&(state, option, chain)) = jobs.get(i) else { break };
                    if let Some(o) = link(f, i, state, option, chain) {
                        check(state, option, &o);
                    }
                }
            });
        }
    });
}

/// What a state must give, beyond the link kept.
fn expected(state: State, option: &[&str], o: &Outcome) {
    let what = format!("{state:?} {option:?}");
    let err = o.stderr_text();
    let state_file = option.iter().any(|a| a.starts_with("--incremental-state"));
    let report = option.iter().any(|a| a.starts_with("--incremental-report"));
    if state_file {
        // IN-17, I-58b: the state through a descriptor is refused by name
        // before anything is read (it was replaced, or read until a hang).
        assert_eq!(o.status.and_then(|s| s.code()), Some(1), "{what}: {err}");
        assert!(err.contains("error: cannot write output file "), "{what}: {err}");
        return;
    }
    match state {
        // The state that replaced the link before WP-114: refused by name.
        State::NoType => {
            assert_eq!(o.status.and_then(|s| s.code()), Some(1), "{what}: {err}");
            assert!(err.contains("of no file type"), "{what}: {err}");
        }
        // S-4: a closed descriptor is /dev/null.
        State::Closed | State::Null => {
            assert!(o.success(), "{what}: {err}");
        }
        State::File | State::Append if !report => {
            // Written in place through the alias (I-58a).
            assert!(o.success(), "{what}: {err}");
        }
        _ => {}
    }
}

// spec: image.md I-58b (every descriptor state × every written option:
// written in place or refused by name; the link through the descriptor
// table is never renamed over or removed)
#[test]
fn a_link_through_a_descriptor_table_is_never_replaced() {
    let Some(f) = Fixture::new("descriptors-all") else { return };
    let mut jobs = Vec::new();
    for state in STATES {
        for option in OPTIONS {
            for chain in [false, true] {
                jobs.push((state, option, chain));
            }
        }
    }
    parallel(&jobs, &f, expected);
}

// spec: image.md I-58b (WP-114's reproduction: descriptor 1 of no file
// type replaced the link with the map every time; and a closed one), 100
// times each, in parallel
#[test]
fn the_state_that_replaced_the_link_is_refused_every_time() {
    let Some(f) = Fixture::new("descriptors-repeat") else { return };
    let map: &[&str] = OPTIONS[0];
    let output: &[&str] = OPTIONS[3];
    let mut jobs = Vec::new();
    for i in 0..100 {
        jobs.push((State::NoType, map, i % 2 == 1));
        jobs.push((State::NoType, output, i % 2 == 1));
        jobs.push((State::Closed, map, i % 2 == 1));
    }
    parallel(&jobs, &f, expected);
}
