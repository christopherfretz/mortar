# The harness

`xtask` is the project's harness: `cargo xtask <command>`. This file is the
contract of its commands and of the rules every process it starts lives
under: the gate (`ci`), the machine's pool of parallelism tokens
("Jobserver"), the limits of a child process ("Limits"), and the
measurement of the linkers' speed and memory (`bench`). The test binaries
start processes too, through `mortar_testutil`, and the same rules bind
them. Still to be specified here: the link tests (WP-010).

## ci

`cargo xtask ci [--quick] [--step <name>]...` is the gate
(`docs/VALIDATION.md` "Gates"). Its steps, in order:

| step | command |
|---|---|
| `fmt` | `cargo fmt --all --check` |
| `clippy` | `cargo clippy --workspace --all-targets -- -D warnings` |
| `test` | `cargo test --workspace` |

Before a step it prints `[ci] <step>` on standard error. It stops at the
first step that fails, printing `[ci] <step>: FAILED`, with exit status 1,
and ends with `[ci] ok` and exit status 0 only if every step it ran passed.
`--quick` and the full gate run the same steps until a later work package
gives the full gate more.

`cargo` is the one that runs `xtask` (the `CARGO` variable cargo sets for
what it runs), else `cargo` on `PATH`.

**A step is a child of the harness** and is started as every child is,
through `xtask::process` (`run_step`): its standard input is closed, its
standard output is the gate's, and its standard error is copied to the
gate's line by line as it arrives. It has no time or memory limit and
holds no jobserver token of the harness's, because cargo is a jobserver
client itself ("Jobserver", "The clients"); an interrupt of the gate stops
it and everything it started ("Jobserver", "Signals").

A cargo step whose standard error has, as a whole line (leading and
trailing blanks aside), one of these two rustc errors is run **once**
more, after the note `[ci] [retry] cargo <arguments> failed with <which>:
...`:

- a line that starts with `error[E0463]` (can't find crate);
- exactly ``error: crate `<name>` required to be available in rlib format,
  but was not found in this form`` (`<name>` one word).

The build directory is on a workspace file system (a bind mount of the
host) whose directory listings, under concurrent builds, now and then lag
behind the files cargo has just written. rustc finds a dependency that is
not on its command line (a crate of the workspace used through another)
by listing `-L dependency=<deps>`: when neither the dependency's `.rlib`
nor its `.rmeta` is listed it says E0463; when the `.rmeta` (written at
the end of analysis, seconds earlier) is listed and the `.rlib` (renamed
into place at the end of code generation) is not yet, and a binary is
being linked, it says the second. One cause; nothing is wrong with the
code, and the rerun finds the artifacts or rebuilds them. Any other
failure (the `dylib` form of the second message among them: mortar links
no dylib), and a second failure of the step, fails the gate.

`--step <name>` (repeatable; also `--step=<name>`) runs only the named
steps, in the gate's order whatever the order they are named in; without
it every step runs. An unknown step or argument, or `--step` without a
name, is a usage error: `xtask ci: <what>` and the usage on standard
error, exit status 2, and no step runs.

Tests: `xtask/src/ci.rs` `steps_are_selected_by_name`,
`the_steps_are_fmt_clippy_and_test`, `retries_only_missing_crates_once`,
`retries_a_crate_seen_only_as_rmeta_once`; `xtask/tests/jobserver.rs`
`ci_runs_its_cargo_steps_in_order_and_stops_at_a_failure` (the command,
with a stand-in for cargo: the order, the lines, the stop at a failure,
the one rerun, the usage errors).

## Jobserver

Every agent on a machine shares one CPU and memory budget, so the harness
and the test binaries are GNU make **jobserver clients**, like the cargo
and rustc they run alongside.

- **The pool.** `scripts/jobserver.sh` owns it: `start` makes a fifo
  (`$MORTAR_JOBSERVER_FIFO`, else `/run/mortar/jobserver.fifo`) holding one
  byte per token (`$MORTAR_JOBS`, else 12), a holder process that keeps it
  open, and a loop that runs `refill` every 60 s. `scripts/env.sh` exports
  `MAKEFLAGS=--jobserver-auth=fifo:<path>` while that fifo exists (and
  `MORTAR_NO_JOBSERVER` is unset); otherwise it caps `CARGO_BUILD_JOBS`
  and there is no pool.
- **What is honoured.** `MAKEFLAGS` containing `--jobserver-auth=fifo:<path>`
  (make 4.4's fifo style; the last `--jobserver-auth=` word wins), read
  once per process. The fifo is opened read+write and stays open until the
  process ends; a token is one byte read from it, and the same byte is
  written back on release.
- **The clients.** One implementation, `mortar_testutil::jobs` (`Client`,
  `Token`, `global()`), for the two kinds of process that start children:
  - a **test binary**: every child started through
    `mortar_testutil::run_with` (and so `run` and `write_script`) holds one
    token for its whole lifetime;
  - **xtask**: every child started through `xtask::process::run` or
    `run_measured` holds one token for its whole lifetime.

  A time limit, and the clock of a measured child, start once the token is
  held: waiting for the pool is never a timeout and never measured. The
  `cargo` steps of `cargo xtask ci` (`process::run_step`) hold no token of
  xtask's: cargo is a client itself, and so are the test binaries it runs.
  The `/bin/sh` that delivers a signal to a process group is not counted.
  What a child starts itself (a linker under its compiler driver) runs on
  the child's token.

  Nothing in `xtask` or in a test starts a process any other way: a bare
  `Command` is a child the pool does not count.
- **The implicit token.** As with make, each client process owns one token
  that is not in the pool: one child may run without reading the fifo;
  each further concurrent child needs a pool byte. So N concurrent clients
  on a pool of P tokens run at most P + N children. (A client that is
  itself the child of a client runs on the token its parent holds for it,
  so the bound holds for a tree of them.)
- **Threads** stay what they are: a test binary has `RUST_TEST_THREADS`
  test threads (`scripts/env.sh` sets 6, unless set, while there is a
  pool: libtest is not a client) and a command of the harness has the
  worker threads it starts, so they are an upper bound on in-flight
  children; the pool can only lower the effective parallelism. A thread
  waits for a token in `acquire`, and takes the implicit token if that
  becomes free first.
- **Release.** Tokens are released by `Drop`: on a normal exit, an error
  return, a panic of the thread, and a child killed by a limit.
- **What the pool's refill sees.** Tokens get lost (a client killed with
  SIGKILL takes its tokens with it), so the pool script's `refill` tops the
  pool up to its size less the tokens in use, which it must never
  under-count. It counts as in use, besides the compilers it knows by
  name, **one token per child process of every process that has the fifo
  open**. The client fits that accounting, and must keep doing so:
  - it keeps the fifo open from its first use to the end of the process;
  - it holds a pool token only for a child process of its own, from just
    before the child is started until just after it is reaped and its
    output is read, and one token per child at most;
  - a byte read for a waiting thread is handed to that thread or written
    straight back; nothing is kept in reserve.

  So the tokens a client holds never exceed its live children for longer
  than a spawn or a reap takes, and `refill` adds a token only for a
  deficit that two passes a minute apart both saw. (A test's child that
  leaves a descendant holding its output keeps `run_with`, and its token,
  waiting without a child; that is a defect of the test, not of the
  accounting.) `scripts/jobserver.sh refill` run twice while a client
  holds tokens for sleeping children adds nothing.

  A pass looks at the pool in one process, in milliseconds: it counts the
  idle tokens, finds the clients and the tokens they hold, and counts the
  idle tokens again, keeping the larger count. A client whose tokens go
  back while the pass looks is then counted idle, not missed by both
  counts. A process whose descriptors cannot be read counts as a client. A
  deficit is refilled only when two consecutive passes saw it, by the
  smaller of the two, with or without clients: a pool that lost tokens
  while nothing ran is whole again two passes later.
- **The shared pool's guard.** `scripts/jobserver.sh refill`, `resize`
  and `stop` refuse the shared pool, the default fifo
  (`/run/mortar/jobserver.fifo`), with exit status 3 and a line on standard
  error that says why and how to name a private pool, unless the caller
  confirms with `--shared` (any position after the verb) or
  `MORTAR_JOBSERVER_SHARED=1`. The refill loop the script starts (the
  process whose pid is in `<pool>.refill.pid`) needs no confirmation.
  `start` and `status` are not guarded. A private pool is named on the
  command itself: `MORTAR_JOBSERVER_FIFO=<path> scripts/jobserver.sh
  <verb>`. `MORTAR_JOBSERVER_SHARED_FIFO` moves what the script takes for
  the shared pool; only its tests set it, to a path of their own, so that
  no command of theirs can reach the machine's pool even without
  `MORTAR_JOBSERVER_FIFO`.
- **No pool.** `scripts/env.sh` without a fifo exports
  `CARGO_BUILD_JOBS=8` (unless set) and says so once on standard error
  (`env.sh: no jobserver pool at <fifo>: ...`; once per environment: a
  second `source` in it is quiet), unless `MORTAR_NO_JOBSERVER` is set,
  which asks for no pool. Nothing parses the standard error of `env.sh`.
- **Signals.** xtask catches **SIGINT and SIGTERM** on a dedicated thread
  (the `signal-hook` crate, a dependency of `xtask` alone), with or without
  a pool. The thread starts with the first child or cargo step. The
  handler's `unsafe` stays inside that dependency, so xtask remains
  `forbid(unsafe_code)`. If the signals cannot be caught or the thread
  cannot be started, xtask says so once (`xtask: warning: ...`) and runs
  on: an interrupt then ends it as it would any process. On either signal:
  1. **The children die first.** Every child started through
     `process::run` or `run_measured` is in a table from spawn to reap
     (`xtask::jobs::children`), and each is the leader of its own process
     group.
     - Each live group is killed with `SIGKILL`: the limit wrapper, the
       tool, and what the tool started all go together.
     - The `cargo` steps (`process::run_step`) are in the same table when
       the signal thread is running. Their group gets `SIGTERM` first, so
       cargo can stop its rustc and test children, and `SIGKILL` 3 s later
       if it is still alive. Cargo is a jobserver client of its own and
       holds no xtask token, so tokens that cargo or a test binary still
       held when it died are lost until the pool is refilled.
     - The thread then waits until each child is reaped and no spawn is in
       flight. It waits at most 10 s, for a child in uninterruptible sleep.
     - A child whose spawn was in flight is killed as it registers.
     - From then on nothing is started: `process::run`, `run_measured` and
       `run_step` do not return to their callers any more (their threads
       park), so a child the interrupt killed is nobody's result, nothing
       is reported for it, and nothing races the signal thread to the exit.
     - stderr says `xtask: signal N: killed K child process(es)` when K
       is not 0.
  2. Every pool byte the process still holds (by live tokens, or already
     read for waiting workers) is written back to the fifo. This happens
     only after step 1, so an orphan never runs on a returned token. Tokens
     whose children were reaped in step 1 went back through their normal
     `Drop`. The helper thread may still be in a read that nobody waits
     for any more; it takes the next byte that reaches the fifo, as likely
     as not one of those just written, and writes it straight back. The
     signal thread waits until that read is done (at most 200 ms: with an
     empty pool the read gets nothing), so that the process does not end
     with a byte in the helper's hands; no other read is started.
  3. Later token drops return nothing, and new acquisitions are free. Each
     token is therefore returned exactly once.
  4. The signal's default action ends the process (falling back to exit
     status 128 + signal). stderr says `xtask: signal N: returned K
     jobserver token(s)` when K is not 0.

  The memory and time limits are unchanged: they end one child's group
  and leave the table to its normal reap.
- **A test binary handles no signal.** `mortar_testutil` has no dependency
  to do it with, and libtest owns the process. A test binary that is
  killed loses the pool tokens it held, and its children live on until
  they end or their limits stop them. (A test binary that calls `xtask` as
  a library gets xtask's signal thread with its first `process::run` and
  behaves as xtask does.)
- **What a normal exit can lose.** A helper thread left in a read that
  nobody waits for takes a byte that reaches the fifo and writes it back
  at once. A process that ends between those two steps takes the byte with
  it. Nothing in a process can close that window for an exit it does not
  control; the pool script's refill covers it.
- **SIGKILL cannot be handled.** No process can catch it, so `kill -9`
  (including the kernel's OOM killer), or any other fatal signal a client
  does not catch, loses the tokens the client held, as it would for make
  and cargo. The pool script's refill loop restores them within two
  minutes.
- **Counting a pool.** `mortar_testutil::jobs::refill(path, target)` opens
  the fifo read+write with `O_NONBLOCK`, counts the idle tokens by reading
  until `EWOULDBLOCK` and writes the same bytes back; with a target it
  also writes `+` bytes up to that many idle tokens, and leaves a pool
  above the target alone. It never blocks, whatever the state of the pool:
  a fifo nobody holds open has no idle token. A path that is not a fifo is
  an error. The non-blocking open uses a hard-coded `O_NONBLOCK` per OS
  (Linux, Android, macOS and the BSDs); on other platforms it fails with
  an error. It is what the tests count their own pools with
  (`PrivatePool::idle`); topping up is only correct while no client holds
  tokens, and the machine's pool is counted and refilled by
  `scripts/jobserver.sh` alone.
- **Fallback.** With no `MAKEFLAGS`, the descriptor forms
  (`--jobserver-auth=R,W`, `--jobserver-fds=R,W`, which need inherited
  descriptors), a path that is not a fifo or cannot be opened (one warning
  per process, `jobserver: warning: cannot use jobserver fifo <path>
  (<why>); running without it`), or a later fifo I/O error (a warning;
  acquisition becomes free), the threads alone bound the parallelism,
  exactly as without a jobserver. A fifo that has no token and never gets
  one (its holder is gone) is still a pool: a client runs its children one
  at a time, on its implicit token, and does not hang.
- **Tests use a pool of their own.** A test of the client makes its fifo
  with `mortar_testutil::jobs::PrivatePool`, in a directory of the
  system's temporary directory whose name is unique to the process and
  the call (`LocalDir`), removed on drop; not in the build directory, which
  is a bind mount, where a fifo is unreliable. No test reads or writes the
  machine's pool, and all pass with and without one.

### Origin

The client, the signal handling and the pool script are ported from
hod's (`hod_testutil::jobs`, `xtask/src/jobs.rs`, `xtask/src/jobs/children.rs`,
`scripts/jobserver.sh` there), which came from kiln's
(`xtask/src/jobs.rs`, `xtask/src/util/process.rs`). The environment
variables are `MORTAR_*` and the pool is under `/run/mortar`.

### Tests

`crates/mortar_testutil/src/jobs.rs` (the client, each over a pool of the
test's own): `parses_makeflags`, `no_jobserver_is_free`,
`rejects_a_non_fifo`, `bounds_concurrency_by_pool_plus_implicit`,
`implicit_token_frees_a_waiter`,
`release_all_returns_held_and_spare_bytes`,
`release_all_waits_for_the_helpers_read`,
`refill_tops_up_without_blocking`,
`a_stale_fifo_is_counted_without_blocking`, `tokens_return_on_panic`,
`a_broken_pool_is_free`. `crates/mortar_testutil/src/lib.rs`:
`run_holds_a_token_for_the_childs_lifetime` (and returns it when the
child is killed at its time limit),
`the_time_limit_starts_once_the_token_is_held`.

`crates/mortar_testutil/tests/jobserver.rs` runs the test binary itself as
a client of a private pool: `a_test_binary_is_bounded_by_the_pool` (8
threads on 2 tokens: never more than 3 children alive, measured by the
children), `without_a_usable_pool_nothing_waits` (each fallback, and the
one warning), `a_pool_without_tokens_serializes_and_does_not_hang`,
`the_pool_script_does_not_mint_tokens_for_this_client`
(`scripts/jobserver.sh start`, `refill` twice while the client's children
sleep, `stop`, on a pool of its own; each pass is judged only while the
client lives, and the held children's limit is ten minutes, a hang guard),
`the_pool_script_refuses_the_shared_pool_unless_told` (the guard, with
the test's pool made the "shared" one, with and without
`MORTAR_JOBSERVER_FIFO`).

`xtask/src/jobs/children.rs` ("Signals", step 1):
`shutdown_kills_and_reaps_live_children`,
`run_step_is_registered_and_terminated`,
`run_step_ignoring_term_is_killed`, `run_step_without_table`,
`run_step_runs_a_command`.

`xtask/tests/jobserver.rs` runs its own test binary as a client whose
threads start children through `xtask::process::run`:
`process_run_is_bounded_by_the_pool` (8 threads on 2 tokens: at most 3
children, the pool left full), `process_run_without_a_usable_pool`,
`an_interrupted_xtask_kills_its_children_and_returns_its_tokens` (SIGTERM
and SIGINT: the children are gone, the pool has all its tokens, the
process ended by the signal, no child started afterwards),
`env_sh_names_the_pool_while_its_fifo_exists` (and the one line without
it).

**Time in these tests.** No verdict depends on how busy the machine is.
`a_stale_fifo_is_counted_without_blocking` judges the count once no
process has the abandoned fifo open (a child forked by another test
thread keeps its bytes alive until it calls `exec`, which under load has
outlasted any fixed wait); `release_all_waits_for_the_helpers_read`
checks that the wait ended with the helper's read, under a grace that is
a hang guard (`release_all` itself keeps its 200 ms);
`the_time_limit_starts_once_the_token_is_held` gives `true` 3 s. Bounds
of minutes in these tests are hang guards: waits for the machine's pool
are in them.

## Limits

Every process the harness starts runs under a time limit and an
address-space limit, so that a harness run ends whatever its children do,
and no child can take the machine's memory. `xtask::process` is the one
place where `xtask` starts a process:

| function | for | limits | jobserver token |
|---|---|---|---|
| `run` | a tool or a program under test | `Limits` | one, for the child's lifetime |
| `run_measured` | the same, with its resource usage ("Measured children") | `Limits` | one, for the child's lifetime |
| `run_step` | a cargo step of the gate ("ci") | none | none: cargo is a client |

The limits of a child are the caller's (`Limits { time, memory_kib }`):
the commands that run linkers and programs state theirs in their own
sections.

### What a child gets

A child of `run` or `run_measured` is started as `/bin/sh -c '<set the
limits>; exec "$@"' sh <KiB> <seconds> <program> <arguments>`, and has:

- a **wall-clock limit**: when it passes, the process and everything it
  started (its process group) are killed with `SIGKILL`. The limit starts
  once the child's jobserver token is held;
- an **address-space limit** and a **CPU-time limit** set in the child
  before the program starts (`ulimit -v`, `ulimit -t`), which the kernel
  enforces, also if the harness dies, and which everything the child
  starts inherits. The CPU limit is the wall-clock limit rounded up to a
  second, plus one, times the number of processors the harness may use
  plus one: a child cannot use that much before its wall-clock limit,
  whatever the number of its threads (a linker runs on all processors),
  so it only ends a child whose harness is gone;
- core files switched off (`ulimit -c 0`): a program under test may be
  expected to die by a signal;
- at most 16 MiB of each output stream kept; the rest is discarded and
  the result says `truncated`;
- its own process group; its standard input is what the caller gives it
  (nothing, or bytes), its standard output and error are captured;
- the arguments, environment and working directory the caller adds; the
  environment is otherwise the harness's.

A child that never reads its input does not hold the harness, and neither
does one that leaves a descendant holding its output: the output of a
child that has ended is waited for 2 s, then what is left of its group is
killed and what was read is the output.

The result (`Output`) has how the child ended, both output streams,
`truncated`, and `wall`: the time from just before the process was
started, with its token held, to the return of `wait`, taken by a thread
blocked in `wait` (not by polling, which would add its interval). It
includes starting the shell.

### How a child ended

| `End` | when | as text |
|---|---|---|
| `Exit(n)` | it exited with status `n` | `exit status n` |
| `Signal(n)` | it was terminated by signal `n` | `signal n` |
| `Killed(Time)` | the harness killed it at the wall-clock limit | `killed: time` |
| `Killed(Memory)` | it did not succeed, and said that it was refused memory | `killed: memory` |

**A killed child is not a failed child.** `Killed` says the limit ended
the child, not what the child would have answered: a caller that expects
a failure (an expected diagnostic, an expected-failure marker) is not met
by it, and reports it as killed (`docs/VALIDATION.md` "Resource limits").

`Killed(Time)` is exact: the harness did the killing. What the child
wrote before is kept.

`Killed(Memory)` is a recognition. The kernel enforces the address-space
limit by refusing the allocation that would exceed it, and keeps no
record that it did; what is left to see is what the child wrote before it
gave up. A child that did not exit with status 0 is `Killed(Memory)` when
a line of its standard error

- contains one of: `failed to map segment from shared object` (the
  dynamic loader), `Cannot allocate memory` (`strerror(ENOMEM)`), `memory
  exhausted` (BFD, gnulib, GCC), `out of memory allocating` (libiberty:
  GNU ld, GNU as, GCC), `LLVM ERROR: out of memory` (lld, clang),
  `throwing an instance of 'std::bad_alloc'` (libstdc++), `uncaught
  exception of type std::bad_alloc` (libc++abi);
- or contains `memory allocation of ` and ends with ` bytes failed`
  (Rust's allocation error handler: mortar itself);
- or starts with `MemoryError` (Python).

The bare name `std::bad_alloc` is not a mark: a linker prints it in a
diagnostic about its symbols. A child that exited with status 0 is never
killed, whatever it wrote. A child that was refused memory and ended
without such a line (a null pointer it did not check: a segmentation
fault) is reported by its exit status or signal, as is a child the
kernel's CPU limit or its OOM killer ended (`Signal(9)`).

### Measured children

`run_measured(program, configure, stdin, limits, probe, usage_file)` is
`run` for a child whose time and memory are the result (`bench`):

- the child is started as `/bin/sh -c '<set the limits>; exec "$@"' sh
  <KiB> <seconds> /usr/bin/time -f '%M %U %S' -o <usage_file> -- <program>
  <arguments>`: the limits are set outside GNU `time`, which then starts
  the program with them inherited, so its report is the program's alone.
  GNU `time` is used because the kernel's resource record of a child
  (`wait4`) cannot be read from safe Rust without a dependency;
- it does not get `MAKEFLAGS` or `CARGO_MAKEFLAGS`: a measured child is
  one process and is not to find a jobserver;
- `usage_file` is removed before the child starts and after its report is
  read;
- the result's `usage` is the program's **peak resident set size** in KiB
  (`%M`) and its **CPU time**, user plus system, in microseconds (`%U`,
  `%S`; GNU `time` reports hundredths of a second). It is absent for a
  child killed at the time limit and when the report cannot be read;
- `wall` is as for `run`, and also includes starting GNU `time`;
- a program terminated by a signal is `Signal(n)`: GNU `time` exits with
  the signal's number and says `Command terminated by signal n` in its
  report, which is read.

`Probe::detect(time, scratch)` uses the GNU `time` at `time` if it
accepts `-f <format> -o <file>` and reports three numbers for a child
that does nothing; otherwise the probe is empty, the child runs without
GNU `time`, and only `wall` is measured.

### Errors of the harness

`run` and `run_measured` return an error, not a result, for what is the
harness's or the machine's failure and not the program's: the program is
not a file (`cannot run '<program>': no such file`), the process cannot
be started, or a limit cannot be set (the wrapper exits with status 125
after `xtask: cannot set the resource limits`; a program that exits with
125 by itself is an ordinary `Exit(125)`). Once xtask is being interrupted
they do not return at all ("Jobserver", "Signals").

### Children of a test binary

A test starts its processes through `mortar_testutil::run_with(command,
stdin, timeout)`, `run(program, args)` (no input, one minute) or
`write_script`:

- the child holds a jobserver token for its lifetime ("Jobserver", "The
  clients"), and the time limit starts once the token is held;
- its input is `stdin`, its output streams are captured whole;
- at the time limit the child is killed and the outcome's `status` is
  `None` (`timed_out()`); there is no address-space limit: a test that
  runs a linker or a linked program on inputs of unknown size does it
  through the harness;
- `write_script` writes an executable `#!/bin/sh` script through a child
  process, so that the script is never "text file busy" when it is run
  next (a file this process had open for writing is inherited by every
  process another test thread forks at that moment);
- scratch files are in `<build directory>/tmp` (`Scratch`), under a name
  unique to the process and the call, removed on drop.

### Tests

`xtask/src/process.rs`: `captures_output_and_status`,
`reports_a_signal`, `time_limit_kills_the_group` (a child that sleeps
past its limit is `killed: time`, and a grandchild dies with it),
`never_hangs_on_pipes`, `limits_are_set_in_the_child`,
`memory_limit_fires` (`dd` asking for a 256 MiB buffer under a 64 MiB
limit is `killed: memory`; a script that goes on after the refusal and
exits with status 0 is not),
`refused_memory_is_recognized_by_what_the_child_wrote`,
`output_is_capped`, `harness_errors`; measured children:
`usage_report_parsing`, `a_measured_run`,
`signals_and_the_time_limit_of_a_measured_run`, `probe_detection` (each
with a stand-in for GNU `time`), and
`real_probe_measures_a_known_allocation` (with `/usr/bin/time`: a child
that allocates and touches 64 MiB more than another is measured within 1%
of that; skipped with a note without GNU `time`, a failure under
`MORTAR_REQUIRE_TOOLS=1`).

`crates/mortar_testutil/src/lib.rs`: `run_captures_output_and_status`,
`scripts_are_runnable_at_once`, `run_kills_at_the_time_limit`,
`scratch_is_created_and_removed`.

## bench

`cargo xtask bench` measures how fast the linkers link a fixed set of
workloads and how much memory they take (`docs/VALIDATION.md` "Speed and
memory"), and judges mortar against this machine's recorded baseline.
GNU ld, lld and mold are measured on the same inputs as references: their
rows are information, never a failure. The bench links nothing of its own
choosing: every workload is a link line captured from a real build, and is
replayed as it was captured.

AArch64 only for now (the host). The x86-64 and 32-bit x86 rows need an
x86-64 machine: a time measured under `qemu-user` says nothing about speed.
They are a planned row ("Planned workloads").

### Command

`cargo xtask bench [options]`, the options as `--name value` or
`--name=value`:

| option | meaning |
|---|---|
| (no action) | prepare what is not current, measure, print the table, write `<cache>/results.tsv` |
| `--check` | the same, then judge mortar's links against the baseline ("The gate") |
| `--update-baseline --reason <text>` | the same with every workload and every linker, then write the run as the baseline, append to its log, regenerate `docs/status/bench.md` |
| `--report` | regenerate `docs/status/bench.md` from the baseline, measuring nothing |
| `--prepare` | prepare the workloads and print their census; measure nothing |
| `--list` | list the manifest's workloads, the planned ones included |
| `--workload <name>` | only this workload (repeatable); a planned one, or the calibration workload, is an error |
| `--linker bfd\|lld\|mold\|mortar` | only this linker (repeatable) |
| `--runs <N>` | measured runs per link after the warm-up: 1 to 100, default 5 |
| `--baseline <file>` | default `bench/baselines/<machine id>.tsv` |
| `--mortar <binary>` | default: the checkout's, built by `cargo build --release -p mortar --bin mortar` (a cargo step, `process::run_step`) |
| `--work <dir>` | the cache, default `<build directory>/bench-work` ("The cache") |
| `-h`, `--help` | the usage on standard output, status 0 |

At most one action. `--update-baseline` without a non-blank `--reason`,
with `--workload` or with `--linker`, and `--reason` without
`--update-baseline`, are usage errors: a baseline is always a whole run,
and every change of it says why (`docs/AGENTS.md` "Review gates").

Exit status: 0; 1 when `--check` finds a mortar link regressed or lost;
3 when the run cannot be trusted: an error of the harness (a build, a
fetch, a census, a missing tool or baseline), a reference linker that did
not link, a link killed by a limit, or a `--check` with a slower time on
an unsteady machine ("Noise"); 2 for a usage error (`xtask bench: <what>`
and the usage on standard error). An error is one line,
`xtask bench: error: <what>`. Progress lines start with `[bench]` on
standard error; the table and the check go to standard output.

Nothing of the bench runs in `cargo xtask ci`: its unit tests build no
workload and fetch nothing.

### Workloads

`bench/workloads/manifest` lists them, one section each: `[name]` (lower
case letters, digits, `-`), then `key = value` lines; `#` starts a comment
line. A value is words separated by blanks, with no quoting. A key appears
once per section; a key the section's kind does not take, an unknown kind,
class or shape, and a missing required key, are errors naming the line.

| key | kinds | meaning |
|---|---|---|
| `class` | all | `small`, `medium`, `large`, `extreme`, `rust`, or `calibration` (exactly one workload: "Calibration") |
| `kind` | all | `sources`, `synthetic`, `cmake`, `cargo`, `planned` |
| `about` | all | one sentence for the report |
| `sources` | sources | files of `bench/workloads/src/` |
| `shape`, `params` | synthetic | the generator's shape and its two parameters ("Generated workloads") |
| `url`, `sha256`, `root` | cmake | the source archive, its SHA-256, the directory in it with the top `CMakeLists.txt` |
| `options`, `prebuild`, `target` | cmake | cmake `-D` options; make targets built first without capture; the make target whose link is the workload |
| `rev`, `package`, `bin`, `rustc_args` | cargo | a commit of this repository; `cargo rustc -p <package> --bin <bin> -- <rustc_args>` |
| `driver`, `cflags`, `ldflags` | sources, synthetic, cmake (no `ldflags`) | the compiler driver on `PATH`, compiler flags, driver flags of the link |
| `capture` | all built | the pattern the basename of the captured link's output matches (`*` is any run of characters); default `out` |
| `needs` | planned | what the workload waits for |

The workloads (sizes as the census of this machine counted them are in
`docs/status/bench.md`):

| workload | class | built from |
|---|---|---|
| `hello-c`, `hello-c-static` | small | `hello.c`, gcc `-O2`, linked `-pie` / `-static` |
| `hello-cxx`, `hello-cxx-static` | small | `hello.cc` (iostreams, a map, an exception), g++ `-O2`, `-pie` / `-static` against libstdc++ |
| `cxx-medium-g`, `cxx-medium` | medium | protobuf 3.21.12 (the last release without abseil), its `protoc`: `main.o`, `libprotoc.a`, `libprotobuf.a`; g++ `-O0 -g` / `-O0` |
| `rust-self` | rust | mortar's binary, dev profile, at a pinned commit, as rustc invokes the linker |
| `synthetic-objects` | extreme | 1000 objects × 200 functions calling across objects |
| `synthetic-sections` | extreme | 2 objects × 40000 functions, each with a section of code and of data: more than 0xff00 sections per object (extended section numbering) |
| `synthetic-strings` | extreme | 200 objects × 5000 string literals, every other one the same in every object |
| `synthetic-comdat` | extreme | 300 C++ units that all define the same 500 inline functions, `-O0 -g` |
| `synthetic-archives` | extreme | 100 archives × 40 members; half of each archive is needed by the one before |
| `calibration` | calibration | 100 objects × 50 functions (the `objects` shape) |

The long symbol names of `synthetic-objects` (some 200 000 of 40 bytes
and more) are the "one huge string table" of `docs/VALIDATION.md`; the
million-symbol and hundred-thousand-section extremes are approached, at
the scale this machine's disk and time allow, by `synthetic-objects` and
`synthetic-sections`. Larger sizes are a manifest change.

### Planned workloads

`kind = planned` sections are specified and not built: `--list` and the
report name them with what they need. Today: `libcxx-test` (one libc++
test program as the L8 lane links it; needs kiln and hod and the libc++
build), `cxx-large-g` and `cxx-large` (a large open-source C++ program's
objects captured once and pinned; needs tens of GB of disk),
`kiln-hod-medium` (the medium program compiled by kiln and hod),
`incremental-function`, `incremental-object`, `incremental-archive`
(M7), and `cross-x86-64` (the x86-64 and i386 rows, on an x86-64 machine).
A planned workload becomes another kind when what it needs exists.

### The cache

Nothing built is checked in. A workload is built from its pinned sources
into `<work>/workloads/<name>/`, where `<work>` is `--work`, else
`<parent of CARGO_TARGET_DIR>/bench-work` when `CARGO_TARGET_DIR` is set
(the workspace's build directory, shared by its checkouts; git-ignored),
else `<checkout>/target/bench-work`. The cache also holds `downloads/`
(fetched archives, named by the URL's last component), `src/<first 16
digits of the SHA-256>/` (each archive unpacked once for the workloads
that share it, with an `.unpacked` marker), `scratch/` (outputs of links,
removed after each run) and `results.tsv` (the last run).

**One run at a time.** A run holds `<work>/lock`, a file created
exclusively with the process id in it, for its whole length, and removes
it at the end. A lock whose process does not exist (`/proc/<pid>`) is
taken over; a live one is an error (`another bench run (process N) is
using <work>`). `--list` and `--report` take no lock.

**The stamp.** A prepared workload has `stamp`:

```
# mortar-bench-stamp 1
key	fnv1a64:<16 hex digits>
cwd	<directory the link runs in>
census	<objects> <archives> <shared> <others> <input bytes> <symbols> <output bytes> <input hash>   (tab-separated)
file	<size>	<modification time, ns>	<path>     (one per file the census read)
```

The key is FNV-1a 64 over the stamp's first line, the manifest section
(its key lines as written, comments and blank lines dropped), the first
line of `<driver> --version` and of `ld.bfd --version`, and per kind: the
name and bytes of each source; the generator's version (`GENERATOR` in
`xtask/src/bench/synth.rs`); `cmake --version` and `make --version`;
`rustc --version`. A workload is ready when its stamp's key is the current
key, its `response.txt` exists and every `file` line still has its size
and time. When only files changed (a system library was upgraded), the
census alone is redone. Otherwise the workload's directory is removed and
the workload is built again from nothing.

### Preparation

Every tool of a build is a child of `process::run` (one token, limits:
15 minutes and 8 GiB of address space per compiler or archiver, 3 hours
and 8 GiB per make, 20 minutes per download), with `TMPDIR=<workload>/tmp`
and `LC_ALL=C`. A step that fails is an error with its last 12 lines of
standard error. Sources are copied into the build directory and compiled
there under relative names, so that what the objects record of their
directory is the cache's, the same for every checkout.

- **sources**: `<driver> <cflags> -c <source> -o <stem>.o` for each, then
  the captured link `<driver> <ldflags> <objects> -o out`.
- **synthetic**: the generator's sources are written to the build
  directory and compiled `<driver> <cflags> -c <64 files>` per process, on
  at most 8 threads (fewer on a smaller machine), each child holding its
  token; for the `archives` shape, `ar rcsD lib<NNNNN>.a <members>` per
  archive. The captured link is `<driver> <ldflags> main.o <the other
  objects, or the archives in order> -o out`.
- **cmake**: the archive is fetched with `curl -fsSL --retry 3` to
  `<file>.part`, checked with `sha256sum` against `sha256` (a mismatch
  removes it and is an error) and renamed; it is fetched only when the
  downloaded file is missing or its digest is not the pinned one, and
  unpacked with `tar -xzf … --no-same-owner`. Then `cmake -S
  <src>/<root> -B <workload>/build -G "Unix Makefiles"
  -DCMAKE_CXX_COMPILER=<driver> -DCMAKE_CXX_FLAGS=<cflags>
  -DCMAKE_C_FLAGS=<cflags> <options>`, `make <prebuild…>`, and the
  captured `make <target>`. make is a jobserver client: it inherits
  `MAKEFLAGS` from `process::run` and takes its parallelism from the
  machine's pool; `-j` is never passed, and without a pool make runs
  serially. (The Ninja generator is not used: the ninja here is not a
  jobserver client.)
- **cargo**: `git -C <checkout> archive --format=tar -o <workload>/src.tar
  <rev>`, unpacked into `<workload>/src`; then, as a cargo step
  (`process::run_step`: cargo is a client of the pool itself),
  `cargo rustc --offline --locked --quiet --manifest-path
  <workload>/src/Cargo.toml -p <package> --bin <bin> -- <rustc_args>` with
  `CARGO_TARGET_DIR=<workload>/target`, `CARGO_PROFILE_DEV_DEBUG=true`,
  `CARGO_INCREMENTAL=0`, and `RUSTFLAGS`, `CARGO_ENCODED_RUSTFLAGS`,
  `CARGO_BUILD_RUSTFLAGS`, `RUSTC_WRAPPER` removed. `rust-self` passes
  `-C save-temps`: without it rustc deletes the `symbols.o` its link reads
  (under `deps/rustcXXXXXX/`) once the link is done.

### Capture

The link line is captured by a recording `ld`: a `/bin/sh` script at
`<workload>/wrapper/ld`, put first in the driver's program search by
`COMPILER_PATH=<workload>/wrapper` for the captured step only. (gcc and
clang both find `ld` there; unlike `-B`, it adds no `-L` to the line.
GNU ld's `--reproduce` does not exist, and lld's and mold's rewrite the
command line.) It writes its working directory and each argument, each
ended by a NUL, to a new `mktemp <workload>/records/link.XXXXXXXX`, then
execs `/usr/bin/ld.bfd` with the same arguments for the cmake and cargo
kinds (make and cargo go on with the output), and exits 0 otherwise. A
record that does not end with a NUL, whose directory is not absolute, an
argument that is not UTF-8, and an argument starting with `@` (a response
file of the driver's, whose contents are not recorded) are errors.

Exactly one record must have an output (the last `-o <f>`, `-o<f>`,
`--output <f>`, `--output=<f>`; `a.out` without one) whose basename
matches `capture`; otherwise the error lists the outputs recorded.

The captured arguments are stored as `<workload>/response.txt`: one per
line; one made only of letters, digits and `_@%+=:,./-` as it is, any
other (an empty one too) in double quotes with `\` before each `\` and
`"`. That is the one form GNU ld (libiberty's `buildargv`), lld and mold
read back alike: mold does not join a quoted part to an unquoted one.

gcc's link line carries `-plugin <liblto_plugin.so>
-plugin-opt=-fresolution=<TMPDIR>/ccXXXXXX.res`: the `.res` file is gone
at replay, and GNU ld, lld and mold all replay the line without it.

### Replay

Every link of the bench, the census's too, is `<linker>
@<workload>/response.txt -o <scratch output> <flags>` run in the recorded
working directory: the last `-o` wins in all four linkers, so the
captured output is never written. The scratch output is removed before
and after each run.

### Census

Once per preparation GNU ld replays the link with
`--dependency-file=<scratch>` (limits: 10 minutes, 16 GiB). A workload GNU
ld cannot replay is not admitted (an error). From the first rule of the
dependency file (make's format: `\` before a line end continues it,
before a blank keeps the blank, `$$` is `$`), each prerequisite is taken
relative to the working directory, canonicalized, and kept once, in the
order first seen (GNU ld names a file again and keeps `..`). Each file is
counted by its first bytes: `\x7fELF` with `e_type` 1 is an object, 3 a
shared object; `!<arch>\n` or `!<thin>\n` an archive; anything else (a
linker script such as `libc.so`) other. The workload's

- **input bytes** are the files' sizes summed;
- **symbols** are the entries of the `.symtab` of GNU ld's output, and
  **output bytes** its size (each linker's own output size is in its
  row);
- **input hash** is FNV-1a 64 (`fnv1a64:` and 16 hex digits) over each
  argument of the response file, normalized, then for each file its
  normalized path and its contents (each field ended by a NUL).

Normalized: the workload's directory written `$W`, and under it each path
component that is a driver's temporary name (`cc` and six letters or
digits, with or without an extension; `rustc` and six letters or digits)
written `*`. The hash is then the same on every preparation, in every
checkout, and changes exactly when what the link reads changes.

### Linkers

| name | program | configurations | flags |
|---|---|---|---|
| `bfd` | `/usr/bin/ld.bfd` | `1` | none: it has one thread and rejects `--threads` |
| `lld` | `ld.lld-22`, else `/usr/lib/llvm-22/bin/ld.lld`, else 19 (`mortar_testutil::find_tool`) | `default`, `1` | `--threads=1` for `1` |
| `mold` | `mold` on `PATH` | `default`, `1` | `--no-fork` always; `--threads=1` for `1` |
| `mortar` | `--mortar`, else the checkout's release build | `default`, `1` | `--threads=1` for `1` |

`--no-fork`: by default mold's process forks, the child links, and the
parent exits once the output is written, without waiting for the child.
GNU `time` then reports the parent: measured here, 8.6 MB for a link whose
real peak is 65 MB. With `--no-fork` the process measured is the one that
links. Each linker's version (the first line of `--version`) is in the
results header.

### Measurement

- **One measured child at a time**, through `process::run_measured`
  (`spec/harness.md` "Measured children": GNU `time`'s peak RSS and CPU
  time, `MAKEFLAGS` removed), with `LC_ALL=C`, holding **one token** of
  the pool, as every child does. No extra tokens are held for a linker's
  threads: the pool's refill counts one token per child process and would
  mint tokens for the difference ("Jobserver", "What the pool's refill
  sees"). The cost of the shared machine is measured instead
  ("Calibration", "Noise").
- **Limits** of a measured link: 10 minutes, 16 GiB of address space.
  (lld at its default number of threads aborts under 1 GiB: it cannot
  start its threads.)
- Per workload: a **warm-up** round, every configuration once, in the
  order of the table, which decides each configuration's status and is
  discarded; then `--runs` **rounds**, each running every configuration
  that linked once, in the same order.
- **Status** of a configuration, from how a run ended:
  - `ok`: exit status 0 and a non-empty output;
  - `refused`: mortar only: a non-zero exit status (not a signal), no
    output, and something on standard error, whose first line is the
    detail;
  - `killed`: a limit ended the run (`End::Killed`): the detail is
    `killed: time` or `killed: memory`. Never a failure of the linker's
    and never a time; reported on standard error as `KILLED`, and the run
    exits with status 3;
  - `failed`: anything else; the detail is the end and the first line of
    standard error.

  A configuration that is `ok` at the warm-up and does not link in a
  measured round is `failed` or `killed` with `run N: <detail>`. A link
  that is not `ok` has **no figures**: the results file refuses a
  `refused`, `killed` or `failed` row with figures, and an `ok` row
  without them.
- **Figures** of an `ok` configuration: wall time (`Output::wall`, from
  just before the process starts, its token held, to the return of
  `wait`), fastest, median and slowest; peak RSS, smallest, median,
  largest; median CPU time; the size of the last run's output. The median
  of an even number of runs is the lower middle value plus half the
  difference, rounded down. Without a usable GNU `time` (`Probe`) there
  are no memory or CPU figures, and the header says so.
- The **spread** of a row is how far its median wall time is above its
  fastest, in thousandths of the fastest.

### Calibration

The workload of class `calibration` is linked by lld (22) at its default
threads, once to warm up and five times, before the first workload and
after the last: the median of each is `calibration_before_us` and
`calibration_after_us`. It is the run's control: how fast this machine
links the same fixed inputs right now. lld is the control whether or not
`--linker` selects it.

### Noise

The machine is shared with other agents and builds, so a measurement is
never assumed quiet; what the load did to it is measured and recorded:

- a run is **noisy** (`noisy yes`) when its two calibrations differ by
  more than 10% of the faster;
- the 1, 5 and 15 minute load averages before and after the run are
  recorded (`load_before`, `load_after`). The load average is not a noise
  criterion: on a machine where idle agents keep it near the number of
  processors it says little about interference, which the calibration
  measures directly. (`docs/VALIDATION.md` asks for the pool to be idle;
  the pool cannot be observed without taking its tokens, so the
  calibration stands for it.)
- a row whose spread is above 5%, a noisy run, a noisy baseline, and a
  machine more than 25% slower than at the baseline (by the calibration)
  make a **slower time** unjudgeable: `NOISY`, not `REGRESSED`. Memory is
  judged whatever the machine is doing.

### Results file

The results of a run (`<work>/results.tsv`) and a baseline are the same
format, text, tab-separated:

```
# mortar-bench 1
# <key>	<value>                                        (header, in order)
# workload-fields	name	class	objects	archives	shared	others	input_bytes	symbols	output_bytes	input_hash
# link-fields	status	workload	linker	threads	runs	wall_us_min	wall_us	wall_us_max	cpu_us	rss_kib_min	rss_kib	rss_kib_max	output_bytes	spread_permille	detail
workload	<fields>
link	<fields>
```

- Header keys: `date` (UTC, `YYYY-MM-DDThh:mm:ssZ`), `machine` (the id),
  `cpu`, `cores`, `commit` (the checkout's, 12 digits, `+changes` when
  its tracked files differ), `runs`, `probe` (`Probe::describe`),
  `limits`, `calibration` (`lld default on <input hash>`),
  `calibration_before_us`, `calibration_after_us`, `noisy` (`yes`, `no`),
  `load_before`, `load_after`, `version.<linker>`, and in a baseline
  `reason`. A key appears once, and all keys come before the first record.
- A field of free text escapes `\` `\t` `\n` `\r`; an empty field is `-`,
  and a field that is `-` is `\-`. A number absent (a row without figures,
  memory without the probe) is `-`. `spread_permille` is derived and
  checked only for its form.
- The `workload-fields` and `link-fields` lines must be exactly this
  reader's; anything else, an unknown record or status, a field that is
  not a number, a partial set of figures, and figures on a row that is not
  `ok` (or none on one that is), are errors naming the line.
- Writing goes to `<file>.tmp-<pid>` and is renamed into place.

**The machine id** is `<arch>-<cpu>-<cores>c` in lower-case letters,
digits and single dashes: the architecture xtask was built for, the
`model name` of `/proc/cpuinfo` (else `implementer <CPU implementer> part
<CPU part>`, else `unknown`), and the parallelism available. It names no
host and no user.

### Baselines

`bench/baselines/<machine id>.tsv`, checked in, one per machine, written
only by `--update-baseline`, which measures every workload with every
linker, refuses to write a baseline in which a reference linker did not
link `ok`, records `--reason` in the header, appends `<date>\t<commit>\t<reason>`
to `bench/baselines/<machine id>.log`, and regenerates
`docs/status/bench.md`. A baseline change is reviewed like code: the
reason says why mortar's numbers moved.

### The gate

`--check` judges each link of **mortar** in the run against the
baseline's link of the same workload, linker and threads; the reference
linkers are never judged. In this order:

| case | verdict | fails |
|---|---|---|
| the run's link `failed` | not judged | no |
| the run's link `killed`, the baseline's not `ok` (or absent) | not judged | no |
| no baseline link, or it `failed` or was `killed` | new | no |
| the workload's input hash differs from the baseline's | advisory | no |
| both refused | not judged | no |
| `ok` now, refused in the baseline | new | no |
| the baseline is of another machine | advisory | no |
| refused or killed now, `ok` in the baseline | **LOST** | yes |
| memory: median RSS above the baseline's by more than 2% and by more than 1024 KiB | **REGRESSED (memory)** | yes |
| time: median wall above the baseline's × drift by more than 5% and by more than 2 ms, on a steady measurement | **REGRESSED (time)** | yes |
| the same on an unsteady one ("Noise"), memory within bounds | NOISY | no; exit 3 |
| faster or smaller by those margins | improved | no |
| otherwise | ok | no |

**Drift** is the calibration now over the baseline's (each the faster of
before and after), at least 1: a machine that got slower widens the time
margin by as much, one that got faster does not tighten it. Memory is not
scaled. Without a common calibration (another control link or input
hash) a time is unsteady. The comparisons are exact (128-bit integers).

### Report

`docs/status/bench.md` is generated from the baseline (by
`--update-baseline` and `--report`), never edited by hand: the machine,
date, commit, runs, calibration, load, probe and reason; the linkers'
versions; the workloads with their census; per workload a table of every
configuration with its median wall time, the fastest-to-slowest range,
median RSS, both as ratios to mold at its default threads (lld where mold
did not link `ok`), and the output size, or its status and detail; and
the planned workloads. When mortar links, its ratios against mold, lld
and GNU ld are the ones the release gate records. A run prints the same
per-workload table on standard output.

### Generated workloads

`xtask/src/bench/synth.rs`: a pure function of a shape and its two
parameters; the same request gives the same bytes on any machine, and a
change of the output changes `GENERATOR` and the pinned hashes of its
test.

| shape | sources | what it stresses |
|---|---|---|
| `objects (n, f)` | `n` objects of `f` functions; each calls one function of the next object and one further on (never backwards), and reads the object's table | global symbols, relocations, cross-object references |
| `sections (n, f)` | `n` objects of `f` functions, each with a variable of its own, built with `-ffunction-sections -fdata-sections`; calls in chains of 64 | input sections, extended section numbering |
| `strings (n, s)` | `n` objects with a table of `s` literals; even positions the same in every object | mergeable string sections |
| `comdat (n, i)` | a header of `i` inline functions (an inline variable per eight, a class template) and `n` C++ units that all use them | COMDAT groups, duplicate debug information |
| `archives (a, m)` | `a` archives of `m` members of eight functions; the first half of each archive's members call the same member of the next archive | archive member extraction |

Every shape has a `main` that calls into every unit (for `archives`, into
the needed members of the first), so a link drops nothing.

### Tests

`xtask/src/bench/record.rs`: `results_file_round_trip`,
`results_are_written_and_read`, `malformed_files`, `spread`, `hashes`,
`machine_id`, `utc_dates`. `xtask/src/bench/stats.rs`: `the_contract`,
`medians`, `threshold_decisions`, `noisy_run_detection`,
`check_pass_and_fail`, `links_without_a_comparison`,
`an_unsteady_measurement_is_not_a_regression`,
`the_calibration_link_is_the_control`, `headers_decide_what_binds`,
`judging_a_run`, `percents`. `xtask/src/bench/response.rs`:
`capture_records_are_parsed`, `the_output_of_a_link`, `the_wrapper_script`,
`response_files_round_trip`, `response_files_are_parsed_by_gnu_rules`,
`dependency_files_are_parsed`, `globs`. `xtask/src/bench/prepare.rs`:
`arguments_are_normalized`, `files_are_classified`,
`symbol_tables_are_counted`, `stamps_round_trip`, `the_cache_is_locked`.
`xtask/src/bench/measure.rs`: `linker_configurations`, `statuses`,
`figures_of_runs`. `xtask/src/bench/manifest.rs`:
`the_manifest_is_read`, `malformed_manifests`.
`xtask/src/bench/synth.rs`: `the_generator_is_deterministic`,
`the_generators_output_is_pinned`, `the_shapes`.
`xtask/src/bench/report.rs`: `the_table`, `the_status_page`.
`xtask/src/bench.rs`: `options`, `the_checked_in_manifest`,
`baseline_paths`.

The end-to-end behavior (building, capturing, replaying with the real
linkers, the census) is exercised by `cargo xtask bench` itself and not by
a unit test: it builds and fetches, which the gate does not. Its evidence
is the committed baseline and `spec/landed/harness/WP-012.md`.
