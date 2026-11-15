//! `xtask` as a jobserver client (`spec/harness.md` "Jobserver").
//!
//! The client a process uses is the one it inherits through `MAKEFLAGS`,
//! read once. So each test here runs this test binary again as a child
//! process, with a `MAKEFLAGS` that names a pool of the test's own (never
//! the machine's), and lets that child run the `inner_children` test
//! below, which is ignored in a normal run and starts its children through
//! `xtask::process::run`, as a command of `xtask` does.
//!
//! The children are a script that records how many of its kind are alive:
//! it makes `live.<pid>` in a directory, counts the `live.*` there, and
//! appends the count to `peaks`.
#![cfg(target_os = "linux")]

use std::os::unix::process::ExitStatusExt as _;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{Duration, Instant};

use mortar_testutil::jobs::{LocalDir, PrivatePool};
use mortar_testutil::{Outcome, Scratch, run_with};
use xtask::process::{self, End, Limits};

/// What the inner test reads from the environment.
const SCRIPT: &str = "MORTAR_XTASK_TEST_SCRIPT";
const THREADS: &str = "MORTAR_XTASK_TEST_THREADS";
const ROUNDS: &str = "MORTAR_XTASK_TEST_ROUNDS";

/// The limit of children that live until the test lets them go, or kills
/// them. Not a verdict: a hang guard, far above any wait under load.
const HELD_LIMIT: Duration = Duration::from_mins(10);

/// Writes the child script. It stays alive for a tenth of a second, and
/// while `<marks>/hold` exists until `<marks>/release` does.
fn write_child(scratch: &Scratch, marks: &Path) -> PathBuf {
    let body = format!(
        "PATH=/usr/bin:/bin\n\
         marks='{}'\n\
         mkdir \"$marks/live.$$\" || exit 1\n\
         ls \"$marks\" | grep -c '^live\\.' >> \"$marks/peaks\"\n\
         if [ -e \"$marks/hold\" ]; then\n\
         \x20 while [ ! -e \"$marks/release\" ]; do sleep 0.05; done\n\
         fi\n\
         sleep 0.1\n\
         rmdir \"$marks/live.$$\"",
        marks.display()
    );
    scratch.write_script("child", &body)
}

/// This test binary, about to run the ignored test `inner_children` on
/// `threads` threads, each starting the script `rounds` times.
fn inner(script: &Path, threads: usize, rounds: usize) -> Command {
    let mut command = Command::new(std::env::current_exe().unwrap());
    command.args(["--exact", "inner_children", "--ignored", "--nocapture", "--test-threads=1"]);
    command.env(SCRIPT, script).env(THREADS, threads.to_string()).env(ROUNDS, rounds.to_string());
    command.env_remove("MAKEFLAGS");
    command
}

/// The most children that were alive at once, and how many ran.
fn peaks(marks: &Path) -> (usize, usize) {
    let text = std::fs::read_to_string(marks.join("peaks")).unwrap_or_default();
    let counts: Vec<usize> = text.lines().map(|line| line.trim().parse().unwrap()).collect();
    (counts.iter().copied().max().unwrap_or(0), counts.len())
}

/// The pids of the children that made a marker and did not remove it.
fn live(marks: &Path) -> Vec<u32> {
    std::fs::read_dir(marks)
        .unwrap()
        .filter_map(|entry| entry.ok()?.file_name().to_str()?.strip_prefix("live.")?.parse().ok())
        .collect()
}

fn text(outcome: &Outcome) -> String {
    format!("{}\n{}", outcome.stdout_text(), outcome.stderr_text())
}

fn ran_one_test(outcome: &Outcome) {
    assert!(outcome.success(), "the inner test failed:\n{}", text(outcome));
    let out = outcome.stdout_text();
    assert!(out.contains("test result: ok. 1 passed"), "the inner test did not run:\n{out}");
}

// "Jobserver", "The clients" and "The implicit token": a process whose
// threads each start children through `xtask::process::run` has, on a pool
// of 2 tokens, at most 3 children alive, and leaves the pool as it found
// it.
#[test]
fn process_run_is_bounded_by_the_pool() {
    const POOL: usize = 2;
    let scratch = Scratch::new("xtask-bounded");
    let marks = LocalDir::new("xtask-bounded");
    let pool = PrivatePool::new("xtask-bounded", POOL);
    let mut command = inner(&write_child(&scratch, marks.path()), 8, 3);
    command.env("MAKEFLAGS", pool.makeflags());
    let outcome = run_with(&mut command, b"", Duration::from_mins(5));
    ran_one_test(&outcome);
    let (peak, ran) = peaks(marks.path());
    assert_eq!(ran, 24);
    assert!(peak <= POOL + 1, "{peak} children at once on {POOL} tokens and the implicit one");
    assert!(peak >= 2, "peak {peak}: the pool was never used");
    assert_eq!(pool.idle(), POOL, "every token came back");
    assert_eq!(live(marks.path()), [0u32; 0]);
}

// "Jobserver", "Fallback": without `MAKEFLAGS`, with the descriptor form,
// and with a path that is not a fifo (one warning), the threads alone
// bound the children.
#[test]
fn process_run_without_a_usable_pool() {
    let scratch = Scratch::new("xtask-fallback");
    let plain = scratch.write("not-a-fifo", "++");
    for (label, makeflags, warns) in [
        ("none", None, false),
        ("fds", Some("-j8 --jobserver-auth=3,4".to_owned()), false),
        ("plain", Some(format!("--jobserver-auth=fifo:{}", plain.display())), true),
    ] {
        let marks = LocalDir::new(&format!("xtask-fallback-{label}"));
        let mut command = inner(&write_child(&scratch, marks.path()), 4, 2);
        if let Some(makeflags) = makeflags {
            command.env("MAKEFLAGS", makeflags);
        }
        let outcome = run_with(&mut command, b"", Duration::from_mins(5));
        ran_one_test(&outcome);
        let (peak, ran) = peaks(marks.path());
        assert_eq!(ran, 8, "{label}");
        assert!((1..=4).contains(&peak), "{label}: {peak}");
        let warnings = outcome.stderr_text().matches("cannot use jobserver fifo").count();
        assert_eq!(warnings, usize::from(warns), "{label}: {}", outcome.stderr_text());
    }
}

// "Jobserver", "Signals": an interrupted xtask kills its children, returns
// its tokens, and ends by the signal; no child outlives it, nothing is
// reported for the killed children, and the pool has all its tokens.
#[test]
fn an_interrupted_xtask_kills_its_children_and_returns_its_tokens() {
    const POOL: usize = 2;
    for (name, number) in [("TERM", 15), ("INT", 2)] {
        let scratch = Scratch::new(&format!("xtask-signal-{name}"));
        let marks = LocalDir::new(&format!("xtask-signal-{name}"));
        let pool = PrivatePool::new("xtask-signal", POOL);
        // The children stay alive: nothing releases them.
        std::fs::write(marks.join("hold"), "").unwrap();
        // A shell that writes its pid and becomes the client, so that the
        // test can signal a process `run_with` started.
        let pid_file = scratch.join("pid");
        let mut wrapped = Command::new("/bin/sh");
        wrapped.args(["-c", "echo $$ > \"$0\" && exec \"$@\""]).arg(&pid_file);
        let command = inner(&write_child(&scratch, marks.path()), 8, 1);
        wrapped.arg(command.get_program()).args(command.get_args());
        for (name, value) in command.get_envs() {
            match value {
                Some(value) => wrapped.env(name, value),
                None => wrapped.env_remove(name),
            };
        }
        wrapped.env("MAKEFLAGS", pool.makeflags());
        let (outcome, children) = std::thread::scope(|scope| {
            let client = scope.spawn(move || run_with(&mut wrapped, b"", HELD_LIMIT * 2));
            let waited = Instant::now();
            while live(marks.path()).len() < POOL + 1 {
                assert!(!client.is_finished(), "{name}: the client ended early");
                // A hang guard, not a verdict: the client itself waits for
                // a token of the machine's pool before it starts.
                assert!(waited.elapsed() < HELD_LIMIT, "{name}: nothing started");
                std::thread::sleep(Duration::from_millis(20));
            }
            // The other five threads wait for a token; nothing more starts.
            std::thread::sleep(Duration::from_millis(300));
            let children = live(marks.path());
            assert_eq!(children.len(), POOL + 1, "{name}");
            assert_eq!(pool.idle(), 0, "{name}: the children hold the pool");

            let pid = std::fs::read_to_string(&pid_file).unwrap();
            // The shell's own `kill`: a small machine has no other. Started
            // directly, not through `mortar_testutil::run`: a signal is not
            // a job, and a wait for a token of the machine's pool here
            // could outlive the client.
            let kill = ["-c", "kill \"-$1\" \"$2\"", "sh", name, pid.trim()];
            let signalled = Command::new("/bin/sh").args(kill).output().unwrap();
            let complaint = String::from_utf8_lossy(&signalled.stderr);
            assert!(signalled.status.success(), "{name}: {complaint}");
            (client.join().unwrap(), children)
        });
        let errors = outcome.stderr_text();
        let status = outcome.status.expect("the client ended long before the time limit");
        assert_eq!(status.signal(), Some(number), "{name}: {status:?}\n{errors}");
        let killed = format!("xtask: signal {number}: killed {} child process(es)\n", POOL + 1);
        assert!(errors.contains(&killed), "{name}: {errors}");
        for pid in children {
            assert!(!Path::new(&format!("/proc/{pid}")).exists(), "{name}: child {pid} lives on");
        }
        assert_eq!(pool.idle(), POOL, "{name}: the pool has all its tokens\n{errors}");
        // The killed children were nobody's result, and no thread went on.
        let out = outcome.stdout_text();
        assert!(!out.contains("test result") && !errors.contains("panicked"), "{name}: {out}");
        assert_eq!(peaks(marks.path()), (POOL + 1, POOL + 1), "{name}: no child started later");
    }
}

/// A stand-in for cargo: appends its arguments to `$FAKE_LOG`, and fails
/// the step named `$FAKE_FAIL` (always), or the step named `$FAKE_VANISH`
/// once, as rustc fails when an artifact is not visible to it.
const FAKE_CARGO: &str = r#"PATH=/usr/bin:/bin
echo "$*" >> "$FAKE_LOG"
echo "   Checking with $1" >&2
if [ "$1" = "${FAKE_FAIL:-}" ]; then echo "error: boom" >&2; exit 101; fi
if [ "$1" = "${FAKE_VANISH:-}" ] && [ ! -e "$FAKE_LOG.vanished" ]; then
  : > "$FAKE_LOG.vanished"
  echo "error[E0463]: can't find crate for \`mortar_elf\`" >&2; exit 101
fi
if [ "$1" = test ]; then
  mkdir -p "$MORTAR_NAMED_SKIPS" && printf 'fake::skip\tnot here\n' > "$MORTAR_NAMED_SKIPS/fake"
fi
exit 0"#;

// "ci": the steps run in order, each a cargo run through the harness's
// runner; the gate stops at the first that fails; a step that failed
// because an artifact vanished is run once more; a usage error is exit
// status 2.
#[test]
fn ci_runs_its_cargo_steps_in_order_and_stops_at_a_failure() {
    let scratch = Scratch::new("xtask-ci");
    let cargo = scratch.write_script("cargo", FAKE_CARGO);
    let ci = |label: &str, args: &[&str], env: &[(&str, &str)]| {
        let log = scratch.join(format!("{label}.log"));
        let mut command = Command::new(env!("CARGO_BIN_EXE_xtask"));
        command.arg("ci").args(args).env("CARGO", &cargo).env("FAKE_LOG", &log);
        // Its own named skips: the build directory's are those of the gate
        // that runs this test.
        command.env("MORTAR_NAMED_SKIPS", scratch.join(format!("{label}.skips")));
        command.env_remove("MAKEFLAGS").env_remove("FAKE_FAIL").env_remove("FAKE_VANISH");
        for (name, value) in env {
            command.env(name, value);
        }
        let outcome = run_with(&mut command, b"", Duration::from_mins(5));
        let ran = std::fs::read_to_string(&log).unwrap_or_default();
        (outcome.status.and_then(|s| s.code()), ran, outcome.stderr_text())
    };
    let all = "fmt --all --check\nclippy --workspace --all-targets -- -D warnings\n\
               test --workspace\n";

    // The cargo steps only: the link-test step is `xtask linktest`'s own
    // (tests/linktest.rs).
    let cargo_steps = ["--quick", "--step", "fmt", "--step", "clippy", "--step", "test"];
    let (status, ran, errors) = ci("pass", &cargo_steps, &[]);
    assert_eq!((status, ran.as_str()), (Some(0), all), "{errors}");
    // The step's standard error is passed on as it comes.
    let said = "[ci] fmt\n   Checking with fmt\n[ci] clippy\n   Checking with clippy\n[ci] test\n   \
                Checking with test\n[ci] named skips: 1\n[ci]   fake::skip: not here\n[ci] ok\n";
    assert_eq!(errors, said);

    let (status, ran, errors) = ci("fail", &[], &[("FAKE_FAIL", "clippy")]);
    assert_eq!(status, Some(1), "{errors}");
    assert_eq!(ran, all[..all.find("test").unwrap()], "nothing runs after a failed step");
    assert!(
        errors.contains("error: boom\n") && errors.ends_with("[ci] clippy: FAILED\n"),
        "{errors}"
    );
    assert!(!errors.contains("[ci] ok"), "{errors}");

    // --no-fail-fast: every step runs, and the failed ones are named last.
    let (status, ran, errors) = ci(
        "keep-going",
        &["--no-fail-fast", "--step", "fmt", "--step", "clippy", "--step", "test"],
        &[("FAKE_FAIL", "clippy")],
    );
    assert_eq!(status, Some(1), "{errors}");
    assert_eq!(ran, all.replace("test --workspace", "test --workspace --no-fail-fast"), "{errors}");
    assert!(errors.ends_with("[ci] FAILED: clippy\n"), "{errors}");

    let (status, ran, errors) = ci("vanish", &["--step", "test"], &[("FAKE_VANISH", "test")]);
    assert_eq!(
        (status, ran.as_str()),
        (Some(0), "test --workspace\ntest --workspace\n"),
        "{errors}"
    );
    assert!(
        errors.contains("[ci] [retry] cargo test --workspace failed with error[E0463]"),
        "{errors}"
    );
    assert!(errors.ends_with("[ci] ok\n"), "{errors}");

    for bad in [&["--fast"][..], &["--step", "lint"], &["--step"]] {
        let (status, ran, errors) = ci("usage", bad, &[]);
        assert_eq!((status, ran.as_str()), (Some(2), ""), "{bad:?}");
        assert!(
            errors.starts_with("xtask ci: ") && errors.contains("usage: cargo xtask ci"),
            "{bad:?}: {errors}"
        );
    }
}

// "Jobserver", "The pool": `scripts/env.sh` makes cargo, rustc, the test
// binaries and xtask clients of the pool exactly while its fifo exists;
// without it, unless told so, it says once on standard error that it fell
// back.
#[test]
fn env_sh_names_the_pool_while_its_fifo_exists() {
    if mortar_testutil::tool("bash").is_none() {
        return;
    }
    let env_sh = Path::new(env!("CARGO_MANIFEST_DIR")).join("../scripts/env.sh");
    let pool = PrivatePool::new("env", 1);
    let sourced = |fifo: &Path, off: bool| {
        let mut command = Command::new("bash");
        command.args([
            "-c",
            "source \"$0\"; source \"$0\"; echo \"${MAKEFLAGS-none} ${CARGO_BUILD_JOBS-none}\"",
        ]);
        command.arg(&env_sh).env("MORTAR_JOBSERVER_FIFO", fifo);
        for name in ["MAKEFLAGS", "CARGO_BUILD_JOBS", "MORTAR_NO_JOBSERVER", "_MORTAR_NO_POOL_SAID"]
        {
            command.env_remove(name);
        }
        if off {
            command.env("MORTAR_NO_JOBSERVER", "1");
        }
        let outcome = run_with(&mut command, b"", Duration::from_mins(1));
        assert!(outcome.success(), "{}", text(&outcome));
        let said = outcome.stderr_text().matches("env.sh: no jobserver pool at ").count();
        (outcome.stdout_text(), said)
    };
    assert_eq!(sourced(pool.path(), false), (format!("{} none\n", pool.makeflags()), 0));
    assert_eq!(sourced(pool.path(), true), ("none 8\n".to_owned(), 0));
    let missing = pool.path().with_file_name("missing.fifo");
    assert_eq!(sourced(&missing, false), ("none 8\n".to_owned(), 1), "said once, sourced twice");
}

/// Run by the tests above in a child process: `THREADS` threads, each
/// running the script `ROUNDS` times through `xtask::process::run`.
#[test]
#[ignore = "run as a child process by the other tests of this file"]
fn inner_children() {
    let Some(script) = std::env::var_os(SCRIPT) else { return };
    let script = PathBuf::from(script);
    let number = |name: &str| std::env::var(name).unwrap().parse::<usize>().unwrap();
    let (threads, rounds) = (number(THREADS), number(ROUNDS));
    let limits = Limits { time: HELD_LIMIT, memory_kib: 1 << 20 };
    std::thread::scope(|scope| {
        for _ in 0..threads {
            scope.spawn(|| {
                for _ in 0..rounds {
                    let output = process::run(&script, |_| {}, b"", limits).unwrap();
                    assert_eq!(output.end, End::Exit(0), "{output:?}");
                }
            });
        }
    });
}
