# The harness

`xtask` is the project's harness: `cargo xtask <command>`. This file is the
contract of its commands and of the rules every process it starts lives
under: the gate (`ci`), the machine's pool of parallelism tokens
("Jobserver"), the limits of a child process ("Limits"), and the link
tests ("Link tests"). The test binaries start processes too, through
`mortar_testutil`, and the same rules bind them. Still to be specified
here: `bench` (WP-012).

## ci

`cargo xtask ci [--quick] [--step <name>]...` is the gate
(`docs/VALIDATION.md` "Gates"). Its steps, in order:

| step | command |
|---|---|
| `fmt` | `cargo fmt --all --check` |
| `clippy` | `cargo clippy --workspace --all-targets -- -D warnings` |
| `test` | `cargo test --workspace` |
| `linktest` | `cargo xtask linktest --arch <host>` (`--quick`), `cargo xtask linktest` (full) |

Before a step it prints `[ci] <step>` on standard error. It stops at the
first step that fails, printing `[ci] <step>: FAILED`, with exit status 1,
and ends with `[ci] ok` and exit status 0 only if every step it ran passed.

`linktest` is run in-process, after the cargo steps ("Link tests"): any
exit status but 0 fails it. With `--quick` it runs the corpus on the
host's architecture only (every architecture when the host is none of the
three), which adds well under a minute on the reference machine; the full
gate runs every architecture (the others under qemu). The differential
lanes join the full gate as they land (`docs/VALIDATION.md`).

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
`the_steps_are_fmt_clippy_test_and_linktest`, `retries_only_missing_crates_once`,
`retries_a_crate_seen_only_as_rmeta_once`; `xtask/tests/jobserver.rs`
`ci_runs_its_cargo_steps_in_order_and_stops_at_a_failure` (the command,
with a stand-in for cargo: the order, the lines, the stop at a failure,
the one rerun, the usage errors; its passing run names the three cargo
steps, as the link-test step is `xtask linktest`'s own, tested in "Link
tests").

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

## Link tests

`cargo xtask linktest` runs the link-test corpus, `tests/link/**/*.test`
(`docs/VALIDATION.md` layer L2): each test builds its inputs from sources
or `yaml2obj` text, links them with every linker of the run, runs what
came out and checks it with the binutils. The **reference linkers** (GNU
ld, `bfd`; LLVM's `lld`; `mold`) run a test first, and mortar is judged by
it only when they establish it ("The self-check"). A test is a statement
about ELF and the psABIs, not about mortar: a case none of the references
can pass is a wrong case.

### The command

```
cargo xtask linktest [--linker bfd,lld,mold,mortar] [--arch aarch64,x86_64,i386]
                     [--filter <text>]... [-j <n>] [--mortar <binary>]
                     [--results <file>] [--keep] [-v] [<file or directory>...]
```

- `--linker`, `--arch`: comma lists (repeatable; `--opt=v` works too);
  every linker and every architecture by default.
- `--filter <text>` (repeatable): only the tests whose name contains one
  of the texts.
- paths: test files, or directories searched recursively for `*.test`;
  `tests/link` by default. A test under `tests/link` is named by its path
  relative to it (`resolve/weak-undef.test`), any other by the path as
  given. A path that does not exist is a usage error.
- `-j <n>` (also `-jN`, `--jobs`; 1 to 64): worker threads, by default
  the processors, at most 8. Every child holds a jobserver token
  ("Jobserver") whatever the number of threads, so `-j` bounds the
  harness's own concurrency, not the machine's.
- `--mortar <binary>`: the mortar to judge; by default the workspace's is
  built first (`cargo build --package mortar --bin mortar`, a cargo step
  as in "ci") and `<build directory>/debug/mortar` is used.
- `--results <file>`: where the results file goes (default `<build
  directory>/linktest/results.tsv`).
- `--keep`: keep the work directory, `<build directory>/linktest/run-<pid>`
  (otherwise removed at the end), which has a directory per unit,
  `<NNNN>-<test name>/<arch>/`, with the built `inputs/` and a copy of
  them per linker in which that linker's steps ran.
- `-v`: list the PASS results too.

A usage error prints `xtask linktest: <what>` and the usage on standard
error, with exit status 2.

### The matrix

A **unit** is one test on one architecture; the units run on the worker
threads, each start to finish. In a unit:

1. the embedded files are written to `inputs/` and the build steps run
   there, once. A build step that fails or is killed gives the unit one
   `ERROR` or `KILLED` record without a linker (`inputs: ...`);
2. each linker runs the link, run and check steps in a copy of `inputs/`
   of its own (a copy: a linker that damages its inputs reaches no other
   linker; members of a thin archive are copied beside it). It stops at
   its first step that does not pass. The references run in the order
   bfd, lld, mold;
3. the self-check; then mortar, if it holds.

The architectures and their tools:

| architecture | GNU tools | runs | `%{triple}` |
|---|---|---|---|
| `aarch64` | `gcc`, `g++`, `gfortran`, `ld.bfd`, `ar`, `nm`, `objdump`, `readelf` | natively on an AArch64 host | `aarch64-linux-gnu` |
| `x86_64` | the same, prefixed `x86_64-linux-gnu-` off an x86-64 host | `qemu-x86_64 -L /usr/x86_64-linux-gnu` | `x86_64-linux-gnu` |
| `i386` | the same, prefixed `i686-linux-gnu-` | `qemu-i386 -L /usr/i686-linux-gnu` | `i686-linux-gnu` |

(On a host of the architecture the tools are unprefixed and the program
runs natively.) The cross `ar`, `nm`, `objdump` and `readelf` are used
for the cross architectures because the native ones do not know x86
objects: the native `ar` writes an archive without a symbol index. The
LLVM tools (`clang`, `clang++`, `yaml2obj`, `llvm-readelf`, `ld.lld`) are
those of the newest pinned LLVM major (`mortar_testutil::LLVM_MAJORS`),
never an unversioned one; `mold` is `mold` or `ld.mold`.

**Missing tools.** An architecture whose GNU tools, emulator or system
root are missing is not run, with a note naming the Debian package; a
missing linker is left out of the matrix with a note; a test that
`requires:` a missing tool gives a `SKIP` record without a linker
(`requires X: not found`). Under `MORTAR_REQUIRE_TOOLS=1` each of those is
an `error:` line in the summary and exit status 3 (the run goes on with
what there is). A run of mortar without any reference notes that nothing
is proven.

**Every child** is started through `xtask::process::run` ("Limits"), with
a jobserver token and `LC_ALL=C` (diagnostics and tool output are
matched as text), in its unit's directory. Its limits:

| child | time | address space |
|---|---|---|
| a build step, a check tool | 120 s | 4 GiB |
| a link step | 60 s | 4 GiB |
| a run step | 10 s; ×6 under qemu | 1 GiB; +4 GiB under `qemu-i386` |

`#@ timeout:` and `#@ mem:` replace the link and run limits of a test.
`qemu-i386` reserves the guest's whole 4 GiB of address space, which
counts against `ulimit -v`: under 4 GiB it dies with "Unable to find a
guest_base" (measured), so its run gets the program's limit plus 4 GiB.
`qemu-x86_64` runs in the program's limit.

### The test file

A test is a text file, `tests/link/<area>/<name>.test`, of three parts in
this order: directives, steps, embedded files.

```
# What it checks, in prose, with the gABI or psABI point it rests on.
#@ spec: resolve.md "Weak symbols"          the spec section (one or more)
#@ arch: aarch64 x86_64                     default: all three
#@ requires: g++                            tools beyond gcc and binutils
#@ xfail(mortar): no linking yet (M1)       expected to fail, with why
#@ skip(mold,i386): <why>                   not run there
#@ timeout: 30                              s, of each link and run step
#@ mem: 512                                 MiB, of each link and run step
cc -O1 main.c -o main.o                     build steps, once per unit
ar rcs libm.a m.o
link gcc -static -no-pie main.o -L. -lm -o prog    per linker
  exit: fail                                expectations, indented
  diag(bfd): undefined reference to `m'
run prog arg
  stdout: line one
check readelf -sW prog
  has: FUNC * main$
  not(mold): ...

#--- main.c                                 an embedded file
int main(void) { return 0; }
```

A line whose first character is `#` and that is neither a directive nor
an embedded-file marker is a comment. Blank lines are ignored. Every
error of a file is reported, with its line, as one `ERROR` record of the
file with neither architecture nor linker.

**Directives** (`#@ <key>: <value>`), before the first step:

- `spec: <file>.md <section>`: required, repeatable. The value must have
  that form (a lower-case `.md` name, then the section); the section is
  where the behavior is specified, which may still be to be written.
- `arch: <arch>...`: once; the architectures the test runs on.
- `requires: <word>...`: once; exactly the tools the steps use beyond
  `gcc` and the binutils: `g++` (a `cxx` step or `link g++`), `gfortran`,
  `clang` (a `(clang)` compile step or `link clang`/`clang++`),
  `yaml2obj`, `llvm-readelf`. A missing word and an unused one are both
  errors.
- `xfail(<qualifiers>): <reason>`, `skip(<qualifiers>): <reason>`: a
  marker. Its qualifiers must name at least one linker; at most one
  marker applies to a (linker, architecture); every architecture keeps a
  reference linker without a marker. A reference whose output
  legitimately differs under `output: reference` is left out with `skip`,
  not `xfail` (a run step does not fail when its output differs, so
  `xfail` would be `XPASS`).
- `timeout: <seconds>` (1 to 3600), `mem: <MiB>` (16 to 65536).

**Qualifiers** are words in parentheses after a keyword, of three kinds:
compiler (`gcc`, the default, or `clang`) on compile steps only; linker
(`bfd lld mold mortar`) and architecture on link, run and check steps, on
expectations and on markers. Words of one kind are alternatives; kinds
are conjoined: `cc(clang,i386)` is "with Clang, on i386"; `has(bfd,lld)`
applies to bfd and to lld. An architecture outside the test's `arch` is
an error.

**Steps** start in column 1. Their words are split on blanks; `"..."`
keeps blanks (with `\"` and `\\`).

| step | what |
|---|---|
| `cc`, `cxx`, `fc` `<args>` | the C, C++, Fortran compiler of the architecture, with `-c` added |
| `as <args>` | the C driver with `-c` (so `.S` files are preprocessed) |
| `yaml2obj <args>` | `yaml2obj` |
| `ar <args>` | the architecture's `ar` |
| `link <driver> <args>` | a link; `<driver>` is `ld` (the linker itself) or `gcc`, `g++`, `gfortran`, `clang`, `clang++` |
| `run <program> [<args>]` | runs a linked program |
| `check <tool> <args>` | `readelf`, `nm`, `objdump` (the architecture's), `llvm-readelf`, or `text` (the file named by the one argument, read) |

The build steps come before every link, run and check step. Expectations
are the indented lines under a step:

| step | expectation | meaning |
|---|---|---|
| link | `exit: 0` (default) | the link succeeds |
| link | `exit: fail` | it fails ("Expected failures") |
| link | `diag: <pattern>` | some line of its standard error matches |
| link | `no-diag: <pattern>` | no line does |
| run | `exit: <n>` (default 0), `exit: signal <n>` | how the program ends |
| run | `stdout: <text>` | its standard output, one line per expectation, exactly; with none, it prints nothing |
| run | `output: reference` | prints what the references printed ("The self-check") |
| check | `has: <pattern>` | some line of the tool's output matches (at least one `has` or `not`) |
| check | `not: <pattern>` | no line does |

A check tool that does not exit with status 0 fails the step.

**Embedded files**: `#--- <relative name>` starts one, which runs
verbatim to the next `#---` line or the end of the file (LLVM's
`split-file` marker; a bare `---` would collide with `yaml2obj`
documents). The name may have directories; it may not leave the unit.

**Substitutions**, in step arguments and expectation values (not in
embedded files): `%{arch}`, `%{triple}`, `%{emulation}` (the `-m` of
GNU ld), `%{dynamic-linker}`, `%{sysroot}`, `%{libdir}`, `%{elf-class}`
(`ELF64`/`ELF32`), `%{machine}` (readelf's `Machine:` text), `%{linker}`
(not in build steps); `%%` is `%`. A value substituted into a pattern is
matched literally.

**Patterns** match one line, anywhere in it: a run of blanks matches one
or more blanks, `*` matches any text, `^` and `$` at the ends anchor the
match, and `\` makes the next `\ * ^ $` literal (before any other
character it is an error).

### Links

A `link ld` step runs the linker with the arguments as written:
`ld.bfd` of the architecture, `ld.lld`, `mold`, or mortar. A driver link
gives the driver a linker of the matrix:

- the GCC drivers get `-B<wrapper directory>/`, where `ld` is a wrapper
  script of the linker (GCC cannot be given a linker path, and
  `-fuse-ld=` takes fixed names);
- the Clang drivers get `--target=<triple> --ld-path=<wrapper
  directory>/ld` (which names the linker and nothing else; `-B` would also
  move Clang's search for the GCC installation).

The wrappers are `/bin/sh` scripts written when the run starts, before any
child: one per (architecture, linker). A wrapper runs its linker, appends
the linker's exit status (and, to `<file>.args`, its arguments) to the
file named by `MORTAR_LINKTEST_STATUS`, and exits with that status. The
step's own end is the linker's: a recorded status above 128 is that
signal; no status at all means the driver never ran the linker, which is
an `ERROR` of the step (the driver's message quoted).

**Expected failures** (`exit: fail`). The harness removes the output
file (the last `-o X`, `-oX`, `--output X`, `--output=X`; else `a.out`)
before the link. The linker must exit with a non-zero status, not by a
signal; a link killed at its limit is `KILLED`, never a met
expectation; and the output file must not exist afterwards, for every
linker: a failed link leaves no output. (Measured: none of the references
writes one in a fresh directory; GNU ld also removes a stale one, lld and
mold leave a stale one alone.) The test must give a `diag` that applies
to mortar on every architecture: a refusal names its reason
(`docs/ARCHITECTURE.md`, correct-or-refused-by-name).

**L3** (WP-028): the verifier, `mortar_plumb`, is to check every output a
link step produced, by every linker, where the link step ends
(`xtask/src/linktest/engine.rs`, marked `L3 (WP-028)`).

**Runs**. A program on the host's architecture is run natively after a
check that it is a file beginning with `\x7fELF` (`process::run` starts a
program through `/bin/sh`, which would run a file the kernel refuses as a
shell script). Any other runs under the architecture's qemu with `-L
<system root>`.

### Outcomes

Each (test, architecture, linker) gets one **record**:

| outcome | when |
|---|---|
| `PASS` | every step passed |
| `FAIL` | a step did not pass, without a marker |
| `XFAIL` | a step did not pass, and an `xfail` marker applies |
| `XPASS` | every step passed, and an `xfail` marker applies: the marker is stale |
| `SKIP` | a `skip` marker applies; or a requirement is missing; or the references did not prove the test |
| `KILLED` | a child of the linker's steps was killed at a limit, marker or not |
| `ERROR` | the harness could not do what the test says (a tool missing for a step, a driver that did not run the linker, a file that does not parse) |
| `ORACLE` | the references refuted the test: one record per (test, architecture) without a linker |

`KILLED` and `ERROR` are not covered by markers: a marker says how mortar
answers, and neither is an answer.

### The self-check

Once the references of a unit have run:

- **Refuted**, if a reference that ran was `FAIL`, `XPASS`, `KILLED` or
  `ERROR`, or if two references that passed printed differently under
  `output: reference`: the unit gets an `ORACLE` record naming them, the
  references keep their own records, and mortar is not run.
- **Unproven**, if no reference passed (all skipped or xfail): mortar is
  `SKIP`.
- **Established** otherwise: mortar runs, and under `output: reference` it
  must print what the references printed.
- **Unchecked**, when no reference is in the run (`--linker mortar`):
  mortar runs against the test's own expectations, and the run says so in
  a note; an `output: reference` test cannot judge it and gives `SKIP`.

A reference `XFAIL` (a known bug of that reference, which the test
records) is not a refutation.

### Results and exit status

The **summary**, on standard output: the configuration line (tests,
architectures, linkers, jobs, mortar), the notes, the `error:` lines, one
line per result that is not `PASS` (`<OUTCOME> <test> [<linker>:
<arches>]: <what>`; results of a test with the same outcome and text share
a line), the table of counts (a row per linker, a `-` row for the records
without a linker, a column per architecture), and the totals with the wall
time. The **results file** has one line per record, six tab-separated
fields: outcome, test, architecture (`-`), linker (`-`), detail, spec.
Records are in a fixed order: test name, then architecture, then linker
(bfd, lld, mold, mortar), a record without a linker last; so a run's
results do not depend on the threads.

| exit status | when |
|---|---|
| 0 | none of the below |
| 1 | a `FAIL` or `XPASS` (of a reference, only if its unit is not `ORACLE`), or any `KILLED` |
| 3 | otherwise an `ERROR` or `ORACLE`, a missing tool under `MORTAR_REQUIRE_TOOLS=1`, or an error of the run |
| 2 | a usage error |

A reference's `FAIL` under an `ORACLE` counts as the `ORACLE`: the test is
wrong, not a linker.

### Where the references differ

Measured with GNU ld 2.44, lld 22.1.8 and mold 2.37.1; each test that
meets a difference says so in its comment and holds each linker to its
own behavior with qualified expectations (or a marker), and the column
says what mortar does.

| what | bfd | lld | mold | mortar | test |
|---|---|---|---|---|---|
| a later archive's member needs an earlier archive (`-lh -lu`) | undefined | resolved | resolved | as bfd (open question) | `resolve/archive-backref` |
| an executable's global symbols in `.symtab` | GLOBAL | GLOBAL | LOCAL | GLOBAL | `resolve/symtab` |
| `.eh_frame_hdr` without `--eh-frame-hdr` | none | none | always | none | `layout/eh-frame-hdr`, `lang/cxx-exceptions` |
| map file format | `Linker script and memory map` | `VMA LMA Size Align Out In Symbol` | `VMA Size Align Out In Symbol` | plumb.md (WP-011) | `layout/map-file` |
| AArch64 range veneer name | `__far_veneer` | `__AArch64AbsLongThunk_far` | `far$thunk1` | a64.md | `reloc/aarch64-veneer` |
| an undefined weak symbol in a static executable's `.symtab` | absent | WEAK UND | LOCAL UND | not checked | `resolve/weak-undef` |
| merged common symbols of different sizes | largest | largest | **first** (a bug) | largest | `resolve/common` (`xfail(mold)`) |
| IFUNC address equality with `-fpie` objects in a `-no-pie` link, x86-64 | **broken** | kept | kept | kept | `ifunc/irelative` uses `-fno-pie` |
| IRELATIVE relocations of a static executable | `.rela.plt` | `.rela.dyn` | `.rela.dyn` | not checked | `ifunc/irelative` |
| `.text` not placed while another section is (`--section-start`) | at its default | next to it | next to it | not checked | `reloc/aarch64-veneer` places both |

### The corpus

Every case states the spec section (`#@ spec`) and the gABI or psABI
point it checks. Until mortar links (M1), every case is `xfail(mortar):
no linking yet (M1)`; a case that mortar starts passing is `XPASS` and
fails the gate until its marker goes. All cases run on all three
architectures except `lang/fortran-c` (no i686 `gfortran` on the
reference machine) and `reloc/aarch64-veneer` (AArch64 only).

### Tests

`xtask/src/linktest/format.rs`: `a_whole_test`,
`defaults_and_directive_forms`, `markers`, `steps`,
`requires_names_exactly_the_tools_used`, `expectations`,
`a_failing_link_names_mortars_diagnostic`, `embedded_files`,
`substitutions`, `words_of_a_step`. `xtask/src/linktest/pattern.rs`:
`a_pattern_is_a_literal_substring`, `blanks_match_any_run_of_blanks`,
`a_star_matches_any_text`, `anchors_tie_the_match_to_the_ends`,
`a_backslash_makes_a_special_character_literal`, `pattern_errors`,
`any_line_of_a_text`, `escaped_text_matches_itself_only`.
`xtask/src/linktest/target.rs`: `names_of_an_architectures_tools`,
`the_matrix_has_three_architectures_and_four_linkers`,
`what_an_architecture_substitutes`, `finding_an_architectures_tools`.
`xtask/src/linktest/outcome.rs`: `a_marker_turns_a_result_into_an_outcome`,
`which_outcomes_fail_a_run`, `the_references_establish_a_test_or_do_not`,
`references_that_disagree_on_output_refute_a_test`.
`xtask/src/linktest/report.rs`: `the_exit_status_of_a_run`,
`the_results_file`, `the_summary`, `who_a_line_is_for`.
`xtask/src/linktest/engine.rs`: `the_output_file_of_a_link_line`.
`xtask/src/linktest.rs`: `the_command_line`, `usage_errors`,
`collecting_test_files`.

`xtask/tests/linktest.rs`
`every_outcome_is_reachable_and_the_references_go_first`: the command
with fake linkers (scripts that run GNU ld unless told to fail or to
sleep): every outcome; a failing reference gives `ORACLE` and mortar is
not run; a passing mortar under `xfail(mortar)` is `XPASS` and exit
status 1; a child killed at the time limit is `KILLED` whatever the
marker; an expected failure with its diagnostic; `output: reference`;
exit status 3 for a refuted test alone, 0 for a good one; a run without
references; the usage errors.

The corpus itself is the test of the format against real linkers: `cargo
xtask linktest` must give every reference `PASS` (bar the recorded
reference bugs, `XFAIL`) and mortar `XFAIL` on every case until M1.
