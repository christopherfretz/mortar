# The harness

`xtask` is the project's harness: `cargo xtask <command>`. This file is the
contract of its commands and of the rules every process it starts lives
under: the gate (`ci`), the machine's pool of parallelism tokens
("Jobserver"), the limits of a child process ("Limits"), the link
tests ("Link tests"), the gate's hosted runs on GitHub Actions
("Hosted CI"), and the measurement of the linkers' speed and memory
(`bench`). The test binaries start processes too, through
`mortar_testutil`, and the same rules bind them.

## ci

`cargo xtask ci [--quick] [--no-fail-fast] [--step <name>]...` is the gate
(`docs/VALIDATION.md` "Gates"). Its steps, in order:

| step | command |
|---|---|
| `fmt` | `cargo fmt --all --check` |
| `clippy` | `cargo clippy --workspace --all-targets -- -D warnings` |
| `test` | `cargo test --workspace` |
| `linktest` | `cargo xtask linktest --arch <host>` (`--quick`), `cargo xtask linktest` (full) |
| `census` | `cargo xtask census --quick` (`--quick`), `cargo xtask census` (full) |
| `mason` | `cargo xtask mason libcxx --sample ci --mode mortar --mode dyn-mortar --plumb --baseline tests/mason/baseline-mortar.tsv` (both); `cargo xtask mason programs` (full) |
| `sweep` | `cargo xtask sweep --quick` (`--quick`), `cargo xtask sweep` (full) |
| `static-identical` | `cargo xtask static-identical` (both; every architecture) |

Before a step it prints `[ci] <step>` on standard error. It stops at the
first step that fails, printing `[ci] <step>: FAILED`, with exit status 1,
and ends with `[ci] ok` and exit status 0 only if every step it ran passed.

`--no-fail-fast` is a diagnosis run: every selected step runs whatever
failed before it, `cargo test` gets `--no-fail-fast` (every test binary
runs), and a gate with failures ends `[ci] FAILED: <step>, <step>...`
with exit status 1. The hosted lanes run it only when asked by hand
("Hosted CI", rule 6); a push and a pull request run the plain gate.

After the `test` step (passed or not) the gate prints `[ci] named skips:
<n>` and, indented, `<name>: <reason>` for each test part skipped by name
on this host ("Hosted CI", "Named skips"). It empties the directory of
the records before the step and names it to the step's cargo in
`MORTAR_NAMED_SKIPS` (`<build directory>/tmp/named-skips` unless that
variable is already set: a gate run by a test, as
`xtask/tests/jobserver.rs` does, is given a directory of its own, so it
cannot empty the outer gate's).

`linktest` is run in-process, after the cargo steps ("Link tests"): any
exit status but 0 fails it. With `--quick` it runs the corpus on the
host's architecture only (every architecture when the host is none of the
three), which adds well under a minute on the reference machine; the full
gate runs every architecture (the others under qemu). The differential
lanes join the full gate as they land (`docs/VALIDATION.md`).

`census` is run in-process after `linktest` ("census"): any exit status
but 0 fails it. With `--quick` it runs the host's architecture and a
subset of the drivers and variations (20 lines on AArch64; 2 s measured
by WP-056); the full gate runs every census line of AArch64 and x86-64
(440 lines; 19 s on the reference machine with `-j 6`).

`mason` is run in-process after `census` (WP-058): libc++'s `ci`
sample (eleven tests, `tests/mason/README.md` "libc++") through the
Mason lane, linked by the debug mortar fully static (`mortar`) and on
kiln's dynamic line (`dyn-mortar`: static libc++, dynamic glibc, PIE),
every output verified with its line, the outcomes held to
`tests/mason/baseline-mortar.tsv`; the full gate adds the programs table
(`cargo xtask mason programs`, static and dynamic variants, AArch64 and
x86-64). It fails on a test that does not pass and is not in the
baseline, a verifier finding, a second link that differs, or a cell
that does not hold. Without kiln and hod builds or the lane's
`setup` it says `skipped` and passes (the hosted lanes have neither),
unless `MASON_REQUIRE_TOOLS=1`. Cost on the reference machine: see
`spec/landed/mason/WP-058.md`.

`sweep` is run in-process after `mason` (WP-088): the byte sweep of
`spec/plumb.md` §12.3 on mortar's own dynamic outputs. GCC compiles a
hello program, mortar links it behind a `-B` directory whose `ld` is
mortar, with its map (`--mortar-map`), and every byte of the output's
synthetic sections is flipped (bit 0, bit 7) and the mutant judged by
the verifier; a static-pie's sections are swept 256 bytes at each end.
The cases are a C PIE (lazy and `-z now`), a C `-no-pie`, a C++ PIE and
a C static-pie. Any exit status but 0 fails it: a finding on the
unmutated output, a panic of the verifier on a mutant, or a byte no flip
makes a new finding for (a survivor; mortar's outputs have none). It
runs on the host's architecture (AArch64 when the host is neither 64-bit
one; `cargo xtask sweep --arch aarch64,x86_64` for both), so the two
hosted lanes cover both targets. With `--quick` it runs the cases
`pie.lazy` and `nopie` (about 30 s); the full gate runs every case
(with the debug `xtask`: about 4 minutes a target on the reference
machine, 7.6 and 8.4 minutes on the hosted x86-64 and AArch64 lanes;
WP-088).
Each sweep thread holds a jobserver token (at most 4, `--threads`).

`static-identical` is run in-process after `sweep`, with or without
`--quick`, on every architecture ("static-identical"); any failure fails
it.

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
tests"; `--no-fail-fast` runs every step after a failed one and names the
failed ones last; the passing run lists the one named skip its stand-in
records, in a directory of its own).

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
  unique to the process and the call (`mortar-<label>-<pid>-<n>`),
  removed on drop. A scratch held in a `static` or left by a killed test
  is never dropped, so the first `Scratch` of a process removes the
  entries of that form in `<build directory>/tmp` whose process no longer
  runs (no `/proc/<pid>`; nothing is swept without `/proc`, or in the
  system temporary directory). After a run at most the last test
  binaries' scratch remains. Some scratch outputs are sparse files of
  hundreds of megabytes of apparent size (a section placed far away to
  force a veneer): a tool that copies the build directory without
  preserving holes sees them at full size.

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
`scratch_is_created_and_removed`, `scratch_of_dead_processes_is_swept`.

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
                     [--results <file>] [--keep] [--no-plumb] [-v]
                     [<file or directory>...]
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
- `--no-plumb`: do not run the verifier on the links' outputs ("L3").
  The harness's own test uses it, because its fake linkers are GNU ld
  under every name. The gate never does.
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
#@ anomaly(mold): A11                       verifier findings expected ("L3")
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
  `gcc` and the binutils: `g++` (a `cxx` step, `link g++` or `shlib g++`),
  `gfortran`, `clang` (a `(clang)` compile step or `link clang`/`clang++`),
  `yaml2obj`, `llvm-readelf`, `llvm-dwarfdump`, `gdb`, `lldb`, `eu-elflint`
  (a `check` with that tool). A missing word and an unused one are both errors. A
  fixture states its own `requires`, which every test naming it gets.
- `fixture: <name>...`: the fixtures (each named once) whose built files the test's
  inputs start from ("Fixtures").
- `xfail(<qualifiers>): <reason>`, `skip(<qualifiers>): <reason>`: a
  marker. Its qualifiers must name at least one linker; at most one
  marker applies to a (linker, architecture); every architecture keeps a
  reference linker without a marker. A reference whose output
  legitimately differs under `output: reference` is left out with `skip`,
  not `xfail` (a run step does not fail when its output differs, so
  `xfail` would be `XPASS`).
- **The xfails file.** `tests/link/xfails` holds mortar's expected
  failures that are not the tests' own (`#` comments and blank lines
  aside; only mortar, so a reference is never masked), one per line, of
  two forms:
  - `mortar,<arch>: <reason>`, **run-wide**: an architecture mortar has no
    back end for (at most one per architecture). The reason is mortar's
    `xfail` in every test of the architecture.
  - `mortar[,<arch>...] <test>: <reason>`, **of one test** (its name, as
    `dyn/kinds/output-kind-table.test`), on the architectures named, every
    architecture when none is (at most one per test and architecture):
    the work package that makes the test pass, `M2: WP-0xx (<name>)`.
    When the package lands, deleting its lines is all it takes.

  A line takes the place of the test's own `xfail(mortar)` (the run-wide
  line first); a test's `skip(mortar)` still applies. A test that has its
  own `xfail(mortar)` on an architecture its line also names is a run
  error (one place says why). A mortar link that passes under a line is
  `XPASS`, so the line goes when the package or back end lands. A line
  that does not parse, and on a run of the whole corpus a line naming no
  test, is a run error.
- `timeout: <seconds>` (1 to 3600), `mem: <MiB>` (16 to 65536).
- `diag: <id> <file>...` (M6, `spec/odr.md` §12.1): repeatable; mortar
  only. Every mortar link step of the test reports the diagnostic `<id>`
  (`spec/diag.md` §3.1) with these files as its places, in order. A
  mortar link that reports a one-definition-rule diagnostic no `diag`
  directive names fails (the legal corpus, `spec/odr.md` §12.2, has
  none). The references have no such diagnostics; their outcome is the
  steps' own expectations.
- `anomaly(<qualifiers>): <id>...`: the recorded reference-linker defects
  (`spec/plumb.md` §10.2) the verifier is expected to find on these
  linkers' outputs ("L3"), beyond the run-wide defaults of
  `tests/link/anomalies`. The qualifiers name at least one reference
  linker and never mortar.

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
| `meta <description> <in> -o <out>` | the test producer (`spec/meta.md` §12): a `.mason.meta` container added to the object `<in>` by the architecture's `objcopy`; with a shared object `<in>.so`, its placeholder `.note.mason` table filled from `row` lines (`spec/odr.md` O-50 to O-52; `cargo xtask meta table`, WP-075), and `=full`'s second note (placeholder type `0x4d4f4402`) from `hidden` and `type` lines (WP-076) |
| `shlib <driver> <args>` | a shared library, by the architecture's `gcc`, `g++` or `gfortran` with `-shared -fuse-ld=bfd`: GNU ld links every fixture library, whatever the run's linkers |
| `link <driver> <args>` | a link; `<driver>` is `ld` (the linker itself) or `gcc`, `g++`, `gfortran`, `clang`, `clang++` |
| `run <program> [<args>]` | runs a linked program |
| `check <tool> <args>` | `readelf`, `nm`, `objdump` (the architecture's), `llvm-readelf`, `llvm-dwarfdump` (WP-080: `--verify`, the DWARF of a split-DWARF output), `text` (the file named by the one argument, read), `elflint`, `ldd`, `gdb`, `lldb` ("Checks") |

The build steps come before every link, run and check step. Expectations
are the indented lines under a step:

| step | expectation | meaning |
|---|---|---|
| link | `exit: 0` (default) | the link succeeds |
| link | `exit: fail` | it fails ("Expected failures") |
| link | `diag: <pattern>` | some line of its standard error matches |
| link | `no-diag: <pattern>` | no line does |
| run | `exit: <n>` (default 0), `exit: signal <n>` | how the program ends |
| run | `exit: no-start` | it cannot be started (its interpreter is not there): status 127 natively (the harness's shell, whose exec fails), 255 under qemu (which cannot load it); the same expectation on both hosts |
| run | `stdout: <text>` | its standard output, one line per expectation, exactly; with none, it prints nothing |
| run | `output: reference` | prints what the references printed ("The self-check") |
| run | `env: <NAME>=<value>` | runs the program with that environment variable (repeatable; `LD_PRELOAD=./libhook.so`, `LD_BIND_NOW=1`) |
| check | `has: <pattern>` | some line of the tool's output matches (at least one `has`, `then`, `count` or `not`) |
| check | `then: <pattern>` | some line after the line the previous `has` or `then` matched matches (an order: `DT_NEEDED` entries, `.dynamic` tags) |
| check | `count: <n> <pattern>` | exactly `n` lines match (a relocation count by type: `count: 3 R_AARCH64_RELATIVE`) |
| check | `not: <pattern>` | no line does |
| check | `allow: <pattern>` | `elflint` only: a complaint that is accepted ("Checks") |

A check tool that does not exit with status 0 fails the step, except
`elflint`.

**Checks** beyond the binutils, for dynamic outputs (`spec/dyn.md` §10):

- `check elflint <file>`: `eu-elflint --gnu-ld <file>`; its standard
  output and error together. Every non-blank line that is not `No errors`
  must match an `allow` line that applies to the linker, whatever its exit
  status; `allow` lines are a test's allow-list for the other linkers
  (mortar's outputs need none since its RELRO padding is named
  `.pad.relro`, `spec/layout.md` L-156). A check with no other
  expectation is valid.
- `check ldd <program>`: the architecture's dynamic loader, `<system
  root><interpreter> --list ./<program>`, natively or under the
  architecture's qemu, with the run's `LD_LIBRARY_PATH` ("Runs"): what
  `ldd` prints, without trusting the `ldd` script of the host.
- `check gdb <program> <command>...` and `check lldb <program>
  <command>...`: the debugger in batch mode on the program, with the
  run's `LD_LIBRARY_PATH`, each argument a command (`"break main" run bt
  "info sharedlibrary"`): `gdb -nx -batch -ex 'set pagination off' -ex
  'set debuginfod enabled off' -ex <command>... <program>`, `lldb
  --no-lldbinit -b -o 'settings set target.disable-aslr false' -o
  <command>... <program>` (the container refuses `personality`). The
  host's debuggers run only the host's programs: on another architecture
  the step is `SKIP` with that reason, by name, and the linker's later
  steps do not run (a `SKIP` is not a refutation in "The self-check").

**Embedded files**: `#--- <relative name>` starts one, which runs
verbatim to the next `#---` line or the end of the file (LLVM's
`split-file` marker; a bare `---` would collide with `yaml2obj`
documents). The name may have directories; it may not leave the unit.

**Fixtures.** `tests/link/fixtures/<name>.fixture` is a file of the same
format with only the `requires` directive, build steps and embedded
files: the shared libraries a dynamic test links against (sonames,
version scripts, `DT_NEEDED` chains, `AS_NEEDED` scripts). A fixture is
built once per run and architecture, by the first unit that names it,
under `<work directory>/fixtures/<arch>/<name>`; a unit's `inputs/`
starts as a copy of its fixtures' files, in the order named, then the
test's embedded files (a test's file replaces a fixture's of the same
name) and build steps. A fixture that does not parse gives every test
naming it one `ERROR` record; one that fails to build, an `ERROR` of
each unit that needs it (`inputs: fixture <name>: ...`).

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

### L3: the verifier on every link

Every link step that is to succeed (no `exit: fail`) writes a map, and
the verifier, `mortar_plumb` (`spec/plumb.md`), judges its output once the
step's own expectations have passed. This applies to every linker,
mortar included (`docs/VALIDATION.md` layer L3).

- **The map.** If the link line names one already (`-Map=X`, `-Map X`,
  `--Map=X`, `-Wl,-Map=X`, `-Wl,-Map,X`), that map is used. Otherwise the
  harness adds `-Map=.linktest-plumb-<line>.map` to a `link ld` step, or
  `-Wl,-Map=.linktest-plumb-<line>.map` to a driver step, in the linker's
  directory. All four linkers take `-Map=`: mortar must accept it and
  write its own format (`spec/plumb.md` §3.2). The format is the
  linker's: `gnu`, `lld`, `mold` or `mortar`. The inputs are the files the
  map names, a relative path being read from the linker's directory.
- **What is judged.** The verifier judges what its stages cover
  (`spec/plumb.md` §1): AArch64 ELF64 `ET_EXEC` without `PT_INTERP` and
  `PT_DYNAMIC`. Every other output is counted, by reason, as not judged:
  `machine` (x86-64, i386; any non-AArch64 machine), `pie-or-shared`
  (`ET_DYN`: PIE, static PIE and shared objects, stage 3), `dynamic`
  (`ET_EXEC` with `PT_INTERP` or `PT_DYNAMIC`, stage 3) and `relocatable`
  (`-r`). A file that is not ELF is judged, and fails S1.
- **Stage 3.** Dynamic outputs (`pie-or-shared`, `dynamic`) are judged
  when the verifier has stage 3, which the run reads once at its start
  (`engine::stage3_present`: the verifier's constant
  `mortar_plumb::STAGE` is 3 or more; WP-050 set it to 3). Without it
  they are counted as not judged, `stage 3 absent`, with a note. Stage 3
  needs the link's words (`spec/plumb.md` §12.1): the `ld` wrapper writes
  the words the linker received, one per line, to
  `.linktest-ld-status-<line>.line`, and a reference's output is judged
  with them (a `link ld` step's own words directly). A mortar output is
  judged with its map's `arg` records only; a map without them is P-D2
  ("no command line"), a failure, so a mortar dynamic output never
  passes unjudged. A reference's output whose link the verifier names
  outside stage 3 (P-D1 "outside stage 3: ...": an option `spec/dyn.md`
  refuses, static-pie, version definitions, a closure object no search
  finds) is counted as not judged, `outside stage 3`, its other findings
  still judged; mortar's fails on that finding like any other.
- **The erratum option** (WP-103). The verifier is told whether the link
  had `--fix-cortex-a53-843419` (`Map.fix_843419`, `spec/plumb.md`
  §5.10) from the words the linker received, for every linker, mortar
  included: the `.linktest-ld-status-<line>.line` words of a driver
  step, a `link ld` step's own words (`xtask::plumb::line_fix_843419`:
  the last of `--fix-cortex-a53-843419[=...]` and
  `--no-fix-cortex-a53-843419` wins, through `@file`, `-Wl,` and
  `-Xlinker`; neither is no option). A driver's default is in those
  words: GCC on Debian passes the option to every link, Clang does not.
  Before WP-103 every lane left it to be inferred from the fixes found,
  so an output with an unfixed site and no fix at all passed unjudged.
  `census` states it the same way, and `mason` through the line it hands
  `verify_link_line`.
- **The verdict.** A finding fails the step: `FAIL`, or `XFAIL` under an
  `xfail` marker, with the detail `<step>: L3: the verifier finds
  <output>: <rule> <message> at <address> (and N more)`. Run
  `cargo xtask plumb` on the kept work directory (`--keep`) for the whole
  report. The only exception is a finding that `spec/plumb.md` §10.2
  records as a defect of a reference linker
  (`mortar_plumb::anomaly::classify`), and that is expected of that
  linker: by a run-wide default, or by the test with the following
  directive:

  ```
  #@ anomaly(mold): A9 A11          ids of plumb.md §10.2, for these linkers
  ```

  The qualifiers name at least one reference linker, never mortar, and
  can also name architectures. A recorded anomaly that the test does not
  declare is a failure, and its detail names the directive that would
  expect it. A declared anomaly that none of the linker's judged outputs
  in the unit has is stale, and is a `FAIL` as well. A map or an output
  that cannot be read is an `ERROR`.
- **The defaults.** `tests/link/anomalies` lists the anomalies a
  reference linker shows on essentially every link of a kind, so that no
  test declares them:

  ```
  # a comment; blank lines are ignored
  mold: A1 A2 A3 A9                 <linker>[,<linker, arch or cross>...]: <id>...
  bfd: A10
  ```

  Each line names at least one reference linker, never mortar, and can
  name architectures (none is every architecture) and the word `cross`
  (only on an architecture that is not the host's: its C runtime is then
  Debian's cross packages, another build than the native one, and a
  default can be the reference linker's way with that build's objects;
  A13 of mold on x86-64 is); the ids are those of
  `spec/plumb.md` §10.2. A line that does not parse is a run error (exit
  3); without the file there are no defaults. A defaulted anomaly is
  expected on every judged output of that linker and architecture, and is
  not stale in a unit that does not show it. mortar has no defaults even
  if the file named it. Instead, the run checks the file itself: a
  default that no judged output of the run of that linker and
  architecture shows is a run error naming the line. A run of part of the
  corpus (paths or `--filter`) may well not meet every default, so it
  does not check that, and says so in a note. A test declares only what
  is outside the defaults, under the strict rule above in both directions.
- **The note.** The summary has the note `L3: the verifier judged N link
  outputs in T s` (T is the verifier's time summed over the threads), the
  number of findings expected as anomalies, the counts not judged by
  reason, and per linker `erratum 843419 outputs: <linker> N with the
  option (S sites fixed, U not judged), M without (K sites)` (WP-103):
  the outputs whose line had the option, their fixed and not judged
  sites (`erratum_sites`, `erratum_not_judged`), and the outputs whose
  line had not, whose K sites are none of them judged. On a mortar
  output with the option U is 0: the verifier judges every site of it.
  `census` prints the same counts in a line of its own.

**Cost.** This was measured with WP-028 on the reference machine, on the
AArch64 corpus at `-j 8` with a load average of about 28, in the debug
profile `xtask` builds in. The verifier judges 119 outputs, and 18129 of
its findings are expected anomalies (almost all A9 and A1/A2 on mold). Its
time summed over the threads is 55–57 s. The linktest step takes 8.3 s and
11.1 s with `--no-plumb`, and 19.7 s and 18.7 s with L3, so L3 adds about
10 s to `cargo xtask ci --quick`. That is under the 20 s at which the quick
gate would sample, so every output is judged in the quick gate as in the
full one.

**Runs**. A program on the host's architecture is run natively after a
check that it is a file beginning with `\x7fELF` (`process::run` starts a
program through `/bin/sh`, which would run a file the kernel refuses as a
shell script). Any other runs under the architecture's qemu with `-L
<system root>`. Its environment has `LD_LIBRARY_PATH` set to the linker's
directory (absolute), so a fixture library is found as it is on a user's
machine, unless an `env:` line sets it; then the `env:` lines. Natively
they are the process's environment; under qemu they are given as `-E
<NAME>=<value>` (qemu's own environment is not the guest's), so
`LD_PRELOAD` and `LD_BIND_NOW` reach the guest's loader.

### Outcomes

Each (test, architecture, linker) gets one **record**:

| outcome | when |
|---|---|
| `PASS` | every step passed |
| `FAIL` | a step did not pass, without a marker |
| `XFAIL` | a step did not pass, and an `xfail` marker applies |
| `XPASS` | every step passed, and an `xfail` marker applies: the marker is stale |
| `SKIP` | a `skip` marker applies; or a requirement is missing; or the references did not prove the test; or a check cannot run on the architecture (a debugger under qemu: "Checks") |
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
| a later archive's member needs an earlier archive (`-lh -lu`) | undefined | resolved | resolved | resolved (as lld, mold): `spec/resolve.md` §4.3 | `resolve/archive-backref` |
| a weak definition named after an archive whose member defines the key strongly | member extracted, 2 | 2 | not extracted, 1 | 2 (`spec/resolve.md` §4.1) | `resolve/weak-after-archive` |
| a strong definition named after such an archive | duplicate | duplicate | links silently, 3 | duplicate | `resolve/weak-after-archive` |
| the kept COMDAT copy lacks a symbol another copy defines | first loaded copy, links | first loaded copy, links | lowest live copy, `undefined symbol` | lowest live copy, `comdat-mismatch` (`spec/resolve.md` §6.1, §6.3) | `resolve/comdat-mismatch` |
| the default entry `_start` defined only in an archive member | extracted | extracted | extracted, made local | extracted (`spec/resolve.md` §4.2) | `resolve/entry-default-root` |
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

The dynamic cases (`tests/link/dyn/`, M2) are the rows of `spec/dyn.md`
§16 and the default lines of its §2.1, on AArch64 and x86-64 (i386 is
outside M2); `spec/landed/dyn/WP-051.md` lists them. Each is mortar's
expected failure by a line of `tests/link/xfails` naming the package that
makes it pass, and their references pass but for the wrong outputs of
`dyn.md` §13.2, which are `xfail` markers citing W-n. They add about 10 s
to the quick gate's linktest (80 tests, measured at load 13).

The one-definition-rule cases (`tests/link/odr/`, M6) are the violation
corpus and the legal corpus of `spec/odr.md` §12.1-§12.2, with the
`diag` directive; before kiln writes records, their units come from the
test producer (`spec/meta.md` §12). The **L11 lane** (`tests/mason/`,
WP-074) builds them with kiln and runs the false-positive oracle of
`spec/odr.md` §12.3 (libc++'s test suite, the open-source lanes, and the
C corpus of O-33 for C-linkage kind 5); it joins the full gate as it
lands (`docs/VALIDATION.md`).

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
`xtask/src/linktest/engine.rs`: `the_output_file_of_a_link_line`,
`the_map_of_a_link_line`, `what_the_verifier_does_not_judge`,
`a_default_anomaly_goes_stale`.
`xtask/src/linktest/format.rs`: `anomaly_declarations`,
`default_anomalies`, `arch_xfails` (both forms of the xfails file),
`dynamic_steps_and_expectations` (`shlib`, `env`, `then`, `count`,
`allow`, the new check tools), `fixtures`.
`xtask/src/linktest/engine.rs`:
`a_run_wide_xfail_applies_to_mortar_on_its_architecture` (and a test's
line), `the_environment_of_a_run`, `stage_3_is_absent_today`.
`xtask/src/linktest/outcome.rs`: `a_skipped_check_does_not_refute`.
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

## census

`cargo xtask census [--quick] [--arch <a>[,<a>]] [--filter <s>]...
[--mortar <path>] [-j <n>] [--keep]` is the driver-line census of
`spec/dyn.md` §2 as a lane (`xtask/src/census.rs`, WP-056). The
drivers are those of `scripts/ci-tools.sh`, found through the tool
guard: GCC's `gcc`, `g++` and `gfortran` of each architecture (plain on
the host, `<triple>-` otherwise), `clang-22`/`clang++-22` and
`clang-19`/`clang++-19` (with `--target=<triple>` off the host), and
`kiln++` with hod where the workspace has their builds, on an AArch64
host: natively for AArch64 and with `--target=x86_64-linux-gnu` for
x86-64 (WP-058); skipped by name otherwise, with the reason as a note.
kiln++ rejects `-shared-libgcc`, `-static-libgcc`, `-static-libstdc++`
and `-static-pie` (`unknown argument`): those lines are "no line". Each
compiles a hello (C; C++ with an exception, iostream and a
`thread_local`; Fortran) once per flavour (plain, `-fsanitize=address`,
`-flto`) and links it in each variation of `VARIATIONS` (the census's
twenty and the `x-` options of real builds: `-z norelro`, `-no-pie -z
now`, `-static-pie`, `-static-pie --export-dynamic`, `-init`/`-fini`,
`--export-dynamic-symbol` by name and by glob, `-Bstatic -Bdynamic`,
`--disable-new-dtags`, `-z text`). The driver runs with
`-B<wrapper dir>/`: a wrapper per linker name records the linker's
arguments, then runs the real linker (GNU ld of the architecture; lld
and mold for `-fuse-ld=`), so the reference links the line and its
program runs (under qemu off the host). mortar links the same words (the
wrapper's `-L` removed, `-o` and `--mortar-map=` its own), its program
runs, and the verifier judges the output with the line
(`plumb::verify_link_line`).

A line passes when mortar's program exits and prints as the
reference's and the verifier is clean; or mortar refuses it with a key
of `OUT_OF_M2` (the rule that puts it out of the milestone is printed);
or GNU ld cannot link it either and mortar refuses it; or the driver
writes no line (a note); or its findings are all of one rule
`KNOWN_FINDINGS` lists for that variation (a verifier defect a record
names). Where the reference's own program is known broken
(`REFERENCE_BROKEN`: `x-static-pie-export` on x86-64, where GNU ld's
output dies at start-up, `plumb.md` A34), mortar's program is compared
with the reference program of the variation the entry names
(`x-static-pie`), and the line fails when the reference's program runs
after all (a stale entry). Anything else fails: a refusal no rule names, a program that
runs differently, a line mortar links that GNU ld cannot, any other
finding. It prints a table (architecture, variation, outcome, lines),
one line per line that is not plain ok, and `census: <n> lines, <d>
drivers, <p> pass, <f> fail, in <t> s`; exit status 1 when one fails.
Scratch is `<build directory>/census/run-<pid>`, removed unless
`--keep`. `--quick` is the host's architecture, `QUICK_DRIVERS` x
`QUICK_VARIATIONS`. Under `MORTAR_REQUIRE_TOOLS=1` an architecture
whose tools are missing is an error. Tests: `xtask/src/census.rs`.

## static-identical

`cargo xtask static-identical [--update] [-j <n>] [--mortar <binary>]` checks that mortar's
links are byte-identical to a checked-in baseline,
`tests/identical/baseline.on-<host>.tsv`. It holds the contract that
mortar's output is deterministic (`docs/ARCHITECTURE.md`, principle 6),
on which incremental linking rests: a change that moves one byte of any
output, map or diagnostic of the corpus is seen, and has to be explained
in a baseline diff. It began as the guard of milestone M2
(`spec/landed/dyn/WP-040.md`: dynamic output landed behind one refusal
while static bytes stayed what they were), hence its name; since WP-093
it covers the dynamic, PIE and static-pie links too. A package whose
change is meant to change them rewrites the baseline with `--update` in
the same commit, and its hand-back names the links that changed.

**The corpus** is every test of `tests/link` (the dynamic, PIE and
static-pie cases of `tests/link/dyn` among them) and of `tests/identical`
(larger static C, C++, Fortran and Clang links: `-g`, `--gc-sections`,
`--threads=1` and the default, `--build-id`), on every architecture whose
tools are there. `tests/identical` uses the link-test format ("The test
file"); `cargo xtask linktest` does not run it (its tests have no
expectations of their own). A test is named relative to `tests/link`, else
to `tests/`. No directory and no test is left out. What has no mortar
output has no output digest, and is treated the same way everywhere in the
corpus: a link step whose scope names only reference linkers
(`link(bfd,lld,mold)`) is not a mortar link and has no row; a refusal (a link
step with `exit: fail`, or an `xfail(mortar)` link) is a row like any other, with
its `end` (`exit status 1`), `-` for the output and the map, and the
digest of its diagnostics, so a refusal that changes its words or turns
into a link is seen; a `skip(mortar)` unit has no rows. Shared objects a
test builds as inputs (`shlib`, by gcc and GNU ld) are inputs, like its
objects: their bytes are the host's toolchain's, which "One baseline per
host" absorbs; they are not digested themselves.

Measured by WP-093 on main `42e7ad3` (the same count on both hosts):
without `tests/link/dyn`, 385 links of 90 tests (142 AArch64, 131 x86-64,
112 i386); with it, 898 links of 217 tests (399 AArch64, 386 x86-64, 113
i386), of which `dyn/` is 513 links of 127 tests (257, 255, 1) and 90 of
those are refusals.

**The digest mode** of the link-test engine runs mortar alone (no
reference, no self-check, no verifier) and only the build and link steps:
run and check steps are not run, markers but `skip(mortar)` are ignored
(an `xfail` link is recorded like any other, with its exit status), and a
link that fails unexpectedly ends its unit. Compile steps get
`-ffile-prefix-map=<inputs>=.` after `-c`, so that no object names the
work directory. Every mortar link that is to succeed is given
`--mortar-map=.linktest-plumb-<line>.map`, as under L3. Each link step is
one row:

| column | value |
|---|---|
| `test`, `arch`, `line` | the link step: test name, architecture, line in the file |
| `end` | how the linker ended (`exit status 0`, `signal 11`, ...) |
| `output` | SHA-256 of the output file, `-` when there is none |
| `map` | SHA-256 of the `--mortar-map`, `-` when there is none |
| `stderr` | SHA-256 of the linker's (the driver's) standard error |

**One baseline per host**: `baseline.on-aarch64.tsv` on an AArch64
machine, `baseline.on-x86_64.tsv` on an x86-64 one. The digests are of
mortar's links of inputs the host builds: its own architecture's with
its native GCC and C library (Debian's `libc6-dev`, `libgcc-14-dev`),
the others' with the cross packages (`libc6-dev-<arch>-cross`, ...),
which are other builds of other package revisions. The same test on the
same architecture is thus other bytes on the two hosts (an i386 link,
cross on both, is the same). A change meant to change static bytes
updates both baselines: the one of the host at hand with `--update`, the
other from its hosted lane's log, where a failed check prints, after the
difference lines, `static-identical: the run's rows of the links that
differ:` and those rows in the baseline's format (test names and digests
only). A missing baseline is an empty one: every link differs and is
printed.

The map and the standard error are digested after replacing the link's
directory by `.`, the unit's directory by `<unit>` and the run's work
directory by `<work>`, so the baseline does not depend on the checkout it
was cut in. One input word changes from run to run and is not mortar's:
gcc's driver passes every link it makes without `-static`
`-plugin-opt=-fresolution=<TMPDIR>/ccXXXXXX.res`, the name of its LTO
plugin's resolution file, a new temporary name each time (libiberty's
`make_temp_file`: `cc`, six ASCII letters or digits, the suffix), and a
mortar map records every word of the command line (`arg`,
`spec/plumb.md`). So, before the paths are replaced, a map line that is
exactly `arg -plugin-opt=-fresolution=<path>`, with `<path>` free of
blanks and ending in `/cc`, six ASCII letters or digits and `.res`, has
its `<path>` replaced by `<gcc-resolution>.res`
(`xtask/src/linktest/engine.rs` `normalise_map`). Nothing else is
replaced: not the word in another form, not in the output (mortar does
not write its command line into the image), not in the standard error.
WP-093 ran the dynamic cases three times with other `TMPDIR`s and job
counts (2, the default 8, and 10) and once from a copy of the checkout at another path
with its own build of mortar: without this rule 404 map digests moved and
nothing else; with it, 0 of 806 rows (and 0 of 898 on main `42e7ad3`,
run with two `TMPDIR`s).

`--mortar <binary>` runs a binary whose file name is its program name in
diagnostics (`<name>: error: ...`): one not named `mortar` changes every
`stderr` digest of a link with a diagnostic. Name it `mortar` (copy it
into a directory of its own). The baseline is these rows, tab-separated, sorted by test,
architecture and line, after `#` comment lines.

**The check**: every row of the run must equal the baseline's row of the
same link, and every link of each side must be on the other. A
difference is printed one link per line: `~ <test> [<arch>] line <n>:
<columns that differ>` (with the two ends when `end` differs), `+ ...` a
link the baseline lacks, `- ...` a baseline link not made. Then a summary
line (links, tests, architectures, how many differ, seconds) and
`static-identical: ok` with exit status 0, or `static-identical: FAILED`
with exit status 1. A unit that did not run because a tool is missing is
not compared (a note says how many) unless `MORTAR_REQUIRE_TOOLS=1`. An
error of the run itself (an input that does not build, mortar not built)
is a failure whatever the digests. `--update` prints the same difference
lines, then writes the baseline (it refuses when a unit did not run); a
usage error is exit status 2. `--mortar` runs another build of mortar (another
commit's) against the baseline instead of building the workspace's.

The run is well under two minutes on the reference machine (the debug
mortar; no program is run). Measured by WP-093 on the development
container (load 8 to 12): 30 s for 301 links without the dynamic cases
and 22 to 28 s for 806 with them, at the default job count, on main
`61e806c` (the links are small; the build of the inputs, which both runs
share, is most of it), 81 s with `-j 2`; 51 to 54 s for 898 on main
`42e7ad3`, which added `tests/link/split` (`-g` C++ links). Hosted, on
`61e806c`: 33.5 s (AArch64) and 37.0 s (x86-64) without the dynamic cases,
about 60 s and 66 s with them.

Tests: `xtask/src/identical.rs` `baseline_and_diff`, `the_command_line`,
`the_corpus_is_every_link_test`; `xtask/src/linktest/engine.rs`
`a_map_loses_only_gccs_resolution_file_name`; the command itself in the
gate. The lane's own sensitivity (WP-093, a scratch build of mortar with
`DF_1_NODELETE` added to `DT_FLAGS_1`): in PIEs only, 263 links of 145
tests differ (262 under `dyn/`); in static-pies only, 38 links of 22
tests (34 under `dyn/`); without the dynamic cases the same lane saw 1
and 4.

## Hosted CI

GitHub Actions runs the gate on real hardware of both 64-bit
architectures (`.github/workflows/ci.yml`, `ci-lane.yml`,
`.github/actions/mortar-ci`), with the structure and conventions of
hod's and kiln's hosted CI: an entry workflow that calls a reusable lane
workflow once per architecture, a composite action for the set-up, fan-in
jobs with stable check names, the same concurrency rule, read-only
`permissions`, and one cargo cache per architecture.

| | `ubuntu-24.04-arm` (AArch64) | `ubuntu-24.04` (x86-64) |
|---|---|---|
| container | Debian 13, `buildpack-deps:trixie-scm` | the same |
| tools | `scripts/ci-tools.sh` | the same list |
| AArch64 programs under test | native | `qemu-aarch64 -L /usr/aarch64-linux-gnu` |
| x86-64 programs under test | `qemu-x86_64 -L /usr/x86_64-linux-gnu` | native |
| i386 programs under test | `qemu-i386 -L /usr/i686-linux-gnu` | the same |
| job | `gate`: `cargo xtask ci` | the same |
| fan-in check | `check` | `check-x86_64` |

1. **Two lanes.** `ci.yml` calls `ci-lane.yml` with `ubuntu-24.04-arm`
   and with `ubuntu-24.04`; their jobs show up as `arm / gate` and
   `x86_64 / gate`. The fan-in jobs `check` (AArch64) and `check-x86_64`
   pass only when every job of their lane succeeded: they run with
   `if: always()`, so a failed, cancelled or skipped lane fails them
   rather than counting as passed. They are plain jobs, not a matrix, so
   their names do not change when a lane's jobs do.
2. **The whole gate, one job per lane.** The `gate` job runs
   `cargo xtask ci` (all of "ci": fmt, clippy, the tests, the link tests
   on all three architectures) through `scripts/env.sh`, with
   `MORTAR_REQUIRE_TOOLS=1`. hod and kiln split their gate into parallel
   jobs by `--step`; mortar's takes about a quarter of an hour (rule 11),
   and every job pays its set-up again, so it is one job until it grows
   past half an hour. `--step` is there to split it (`test` alone is
   most of it; an assertion of `xtask/tests/hosted_ci.rs` then changes
   with it).
3. **The reference environment.** The job runs in
   `buildpack-deps:trixie-scm`: Debian 13, the distribution of the
   development container, with git, curl and procps (what checkout and
   the Rust set-up need) and none of the `-dev` libraries of the full
   `buildpack-deps:trixie` that hod uses, because
   `crates/mortar_script/tests/corpus.rs` reads every text library of the
   machine: the hosted machine has the libraries of the tools' packages,
   as the development container does. `.github/actions/mortar-ci` then:
   - runs `scripts/ci-tools.sh` (rule 4);
   - marks the checkout a safe git directory (the container's root over a
     checkout the runner owns) and sets `CARGO_TARGET_DIR` to `../target`,
     outside the checkout, as for a worktree on the development machine
     (`scripts/env.sh` keeps a preset one);
   - installs Rust 1.95.0 with rustfmt and clippy
     (`dtolnay/rust-toolchain`, as hod and kiln; `rust-toolchain.toml`
     pins the same version, which `xtask/tests/hosted_ci.rs` checks);
   - restores the cargo cache (rule 7);
   - starts the jobserver pool, `MORTAR_JOBS=$(nproc)
     scripts/jobserver.sh start` ("Jobserver"): one token per processor
     of the runner, on the default fifo, so `scripts/env.sh` exports
     `MAKEFLAGS` and `RUST_TEST_THREADS=6` there as it does on the
     development machine. The pool's processes end with the container.
4. **The tools: one list.** `scripts/ci-tools.sh` is the list of the
   reference tools the gate needs, the same Debian 13 packages on both
   hosts (a `<tool>-<triple>` package is the host's own compiler under
   its triplet name, and the cross compiler for another architecture):
   the host's `build-essential`, `binutils` and `gfortran`; `gcc-`,
   `g++-` and `binutils-<triple>` of all three architectures and
   `gfortran-<triple>` of AArch64 and x86-64 (none for i686:
   `lang/fortran-c` does not run there); the C and C++ libraries of the
   two cross targets (`libc6-dev-<arch>-cross`,
   `libstdc++-14-dev-<arch>-cross`, their system roots `/usr/<triple>`);
   `clang-22`, `llvm-22`, `lld-22`; Debian's own LLVM 19 (`clang-19`,
   the compiler censuses' clang; `lldb-19`; `lld`, the `ld.lld` on `PATH`
   that `gcc -fuse-ld=lld` runs); `mold`; `gdb` and `gdb-multiarch`;
   `elfutils`; `file`; `qemu-user`; and what the job and the harness run
   (git, curl, make, coreutils, procps, python3, GNU time, xz-utils). Without an argument it installs what is missing
   (`apt-get`, as root) and then checks; `--check` installs nothing and
   checks that every package is installed, every tool the tests look for
   is found (unprefixed for the host; prefixed for all three triples; the
   two other architectures' emulators and system roots; the LLVM 22 tools
   in `/usr/lib/llvm-22/bin`), and the pinned versions; `--list` prints
   the packages. Its exit status is 1 for anything missing or a version
   that differs, 2 for a usage error or a host that is neither AArch64
   nor x86-64.
   - **Versions.** GNU binutils 2.44, GCC 14.2, LLVM 22.1.8, Debian's
     LLVM 19.1.7, mold 2.37.1, gdb 16.3, elfutils 0.192 and qemu 10.0.13,
     pinned in the script, because tests and specs record what
     these releases do ("Where the references differ", `spec/script.md`,
     `spec/elf.md`, `spec/archive.md`): another release is another
     oracle, which is re-measured before the pin moves. Debian 13's own
     packages are these releases; Ubuntu 24.04's (the runners' system:
     binutils 2.42, GCC 13, mold 2.30, no LLVM 22) are not. hod and kiln
     solve this the same way, with a Debian 13 container on the runner
     (hod `buildpack-deps:trixie`; kiln `gcc:16.1.0-trixie`, for its GCC
     16 oracle), not with a third-party archive: apt.llvm.org would give
     an LLVM 22 but not Debian's binutils, GCC or mold, and its builds are
     not the development container's. A Debian point release that moves
     a pinned tool turns the set-up step red with a line naming it.
   - **Required.** The lane sets `MORTAR_REQUIRE_TOOLS=1`, so a tool a
     test looks for and this list does not give fails that test, and
     `xtask/tests/hosted_ci.rs`
     `this_machine_has_the_reference_tools_of_the_hosted_lanes` runs
     `--check` in every gate, so the development machine's gate fails
     when it lacks a tool of the list: the list cannot fall behind the
     tests, nor the machines behind the list, unnoticed.
5. **Results.** After the gate, whether it passed or not (unless the run
   was cancelled), `scripts/ci-summary.py` writes the link tests' table
   of outcomes per linker and architecture, and every record whose
   outcome fails a run, to the job summary, from
   `$CARGO_TARGET_DIR/linktest/results.tsv` ("Results and exit status";
   removed before the gate, so a restored cache cannot show an older
   run's), and the file is uploaded as the artifact
   `linktest-results-<cache key>` (kept 14 days). No results file means
   the gate stopped before its link-test step.
6. **Triggers.** `ci.yml` runs on a `push` to `main`, on every
   `pull_request` and on `merge_group` (the merge queue), always the plain
   gate; and by hand (`workflow_dispatch`: Actions, "ci", "Run workflow",
   on any branch), with the input `no_fail_fast` for a diagnosis run
   (`cargo xtask ci --no-fail-fast`, "ci"). The repository and its Actions
   logs are public: nothing the gate prints may name the machine it was
   developed on (paths, user names).
7. **Caches.** `Swatinem/rust-cache`, `shared-key` `ci-arm` or
   `ci-x86_64` (rust-cache adds the Rust version, the hashes of
   `Cargo.lock` and the manifests, and the job's environment to the key):
   `~/.cargo` and the builds of the dependencies, not of the workspace
   crates (a fresh checkout gives every source a new mtime, so cargo
   rebuilds them anyway). The `gate` job saves it, also when the gate
   failed. With one third-party dependency tree (`signal-hook`, for
   xtask) an entry is some tens of MB; the Rust toolchain itself is
   downloaded by each job.
8. **Concurrency.** One run per ref at a time (`ci-<ref>`). A newer run
   cancels a pull request's or a branch's run in progress, never
   `main`'s: there the newer run waits until the one in progress ends
   (GitHub keeps one waiting run per group, the newest), so every run on
   `main` that starts completes.
9. **Check names and what is required.** The checks to require (branch
   protection or a ruleset, and the merge queue) are the fan-ins, `check`
   and `check-x86_64`, never the lanes' own job names. Both lanes have
   passed (`spec/landed/harness/WP-015.md`), so both can be required;
   `check-x86_64` stays a check of its own, so that the lanes can be
   required separately.
10. **What a red lane means.** A red set-up step is the environment: a
    package Debian no longer has, or a pinned version that moved
    (rule 4). A red `cargo xtask ci` on the AArch64 lane is what the
    same gate in the development container would say, and is a gate
    failure like that one. On the x86-64 lane it is that, or a test that
    assumes an AArch64 host ("A test's subject decides its tools"): the
    fix is in the test (or in `xtask::linktest::target`), never a skip of
    the lane, and a named skip only where the host itself is the reason
    ("Named skips").
11. **Time and cost.** A run is four jobs: two `gate` jobs and two
    fan-in jobs of a few seconds. Measured (`spec/landed/harness/WP-015.md`),
    a `gate` job takes about 16 minutes on `ubuntu-24.04-arm` and about
    19 on `ubuntu-24.04`, of which the set-up (packages, Rust, cache) is
    2 to 3 minutes and `cargo test` most of the rest. Standard runners
    are free for a public repository.

### A test's subject decides its tools

A test is about one architecture (or each of several), whatever host it
runs on, and uses that architecture's tools, through
`mortar_testutil::target::Arch`, which follows the link-test harness
(`xtask::linktest::target`):

- `gnu(tool)`: the plain name on that architecture's own host (`gcc`,
  `ld.bfd`), `<triple>-<tool>` elsewhere; `tool(tool)` looks it up;
- `clang_args()`: `--target=<triple>` off its host; `sysroot()`:
  `/usr/<triple>`;
- `command(program, argv0)` and `runner(program)`: the program itself on
  its host, `qemu-<arch> -L /usr/<triple>` elsewhere; `gdb()`: `gdb` on
  its host, `gdb-multiarch` elsewhere;
- `is_host()`: false for every architecture under `MORTAR_TEST_CROSS=1`,
  so an AArch64 machine can run its AArch64 tests the way an x86-64 host
  does.

A test never takes the host's default toolchain for an architecture it
does not name, never runs a program of another architecture without its
emulator, and never weakens an assertion for a host: an expectation that
truly differs by target or by host is a parameter (one value per case,
each stated), never a looser bound for all.

**Native and cross runtimes.** A host builds its own architecture's
programs against Debian's native C and C++ runtime (`libc6-dev`,
`libstdc++-14-dev`) and the others against the cross packages
(`libc6-dev-<arch>-cross`, `libstdc++-14-dev-<arch>-cross`, built
separately: other package revisions, a `.note.package` in the cross
`crt1.o`, other archive members). So the same AArch64 test links other
input bytes on an AArch64 host than on an x86-64 one, and some outcomes
follow the inputs, each stated where it is checked:

- the static-identical baseline is one per host ("static-identical");
- an anomaly default can apply to the cross runtime only (the word
  `cross` in `tests/link/anomalies`, "L3");
- with the cross libstdc++ the COMDAT copies mortar keeps (resolve.md
  §6, ruling C: the least live FileId) are other files' than GNU ld's
  (the first loaded) for some groups; `crates/mortar/tests/gc.rs`
  tolerates, by name and only for files under `/gcc-cross/`, the
  sections that this alone makes differ, and prints them;
- `crates/mortar/tests/front_diff.rs` bounds the `.eh_frame` mapping
  symbols lld keeps and mortar drops by 4 with the native runtime and by
  16 with the cross one, where it also holds `.eh_frame`'s size and the
  CIE and FDE counts to lld's (suspected cause: the COMDAT keepers leave
  other input `.eh_frame` sections without records, S-50, S-76; not
  proven).

A GNU ld runs the environment's `LD_LIBRARY_PATH` and `LD_RUN_PATH`
into its closure search only as a native linker, so oracles that record
GNU ld state that per architecture by `is_host()`
(`crates/mortar_resolve/tests/shlib_oracle.rs`).

### The tool guard

`mortar_testutil::tool` and `find_listed_tool` look a tool up only by a
name `scripts/ci-tools.sh` covers (rule 4: `TOOLS`, `TRIPLE_TOOLS` for
each of the three triples, `LLVM_TOOLS` bare or with `-<major>`,
`SETUP_TOOLS`, and `FALLBACK_TOOLS`, tried only when a listed name is
missing). `mortar_testutil::ci_tools` reads the lists from the script as
compiled in; a lookup of any other name panics, on every machine and
whatever `MORTAR_REQUIRE_TOOLS` says, naming the tool and the script. So
a tool the development machine has and the hosted lanes would lack fails
the first test that asks for it, locally, not a hosted run; the fix is
to add the tool (and its package) to the script in the same change. A
test that builds a name at run time (`<triple>-<tool>`, `qemu-<arch>`)
is covered by the same lists; a name a shell script of a test runs is
the test's to resolve with `tool` and pass in by path.

### Named skips

A test, or a part of one, that cannot run on this host for a reason of
the host (a debugger session runs a process of the host's own
architecture; the AArch64 `libelf.so.1` is installed only on an AArch64
host) calls `mortar_testutil::named_skip(name, reason)`: never for a
missing tool, which is `tool`'s note or, under `MORTAR_REQUIRE_TOOLS=1`,
its failure. The skip is printed and recorded as the file
`$MORTAR_NAMED_SKIPS/<name>` (one line, `<name>\t<reason>`); `cargo
xtask ci` lists and counts them after the `test` step ("ci"). No named
skip happens on an AArch64 host (the AArch64 lane prints `[ci] named
skips: 0`); on the x86-64 lane they are the gdb and lldb sessions of
`crates/mortar/tests/debuggers.rs` and `gc.rs` and the `libelf` line of
`s8_shared_objects.rs`, ten in all. A new one is a change to review, as
an `xfail` is.

### Tests

`xtask/tests/hosted_ci.rs`:
`the_hosted_lanes_run_the_gate_as_the_development_machine_does` (the
lane runs `cargo xtask ci` once, through `scripts/env.sh`, with
`MORTAR_REQUIRE_TOOLS: "1"`, and `--no-fail-fast` only from the by-hand
input; the set-up runs `scripts/ci-tools.sh` and `scripts/jobserver.sh
start` and installs the Rust of `rust-toolchain.toml`; `ci.yml` calls the
lane for both runners, has both fan-ins, and runs on `push` to `main`,
`pull_request`, `merge_group` and by hand),
`this_machine_has_the_reference_tools_of_the_hosted_lanes`
(`scripts/ci-tools.sh --check` passes; a failure under
`MORTAR_REQUIRE_TOOLS=1`, a note otherwise).
`crates/mortar_testutil/src/ci_tools.rs`
`the_lists_cover_what_the_tests_ask_for`, `a_list_is_one_line` (the
guard); `crates/mortar_testutil/src/target.rs`
`tool_names_follow_the_host` (the convention, with and without
`MORTAR_TEST_CROSS=1`); `xtask/tests/jobserver.rs` (the named skips'
listing, "ci"). The workflows themselves are tested only by running
them.

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
| `--update-baseline --reason <text>` | the same with every workload and every linker, then write the run as the baseline, append to its log, regenerate `docs/status/bench.md` and `docs/status/perf.md` |
| `--report` | regenerate `docs/status/bench.md` and `docs/status/perf.md` from the baseline, measuring nothing |
| `--judge <file>` | judge a saved run (an earlier `results.tsv`) against the baseline as `--check` does, measuring nothing; the same exit status |
| `--prepare` | prepare the workloads and print their census; measure nothing |
| `--list` | list the manifest's workloads, the planned ones included |
| `--workload <name>` | only this workload (repeatable); a planned one, or the calibration workload, is an error |
| `--linker bfd\|lld\|mold\|mortar` | only this linker (repeatable) |
| `--runs <N>` | measured runs per link after the warm-up: 1 to 100, default 5 |
| `--baseline <file>` | default `bench/baselines/<machine id>.tsv` |
| `--mortar <binary>` | default: the checkout's, built by `cargo build --release -p mortar --bin mortar` (a cargo step, `process::run_step`) |
| `--work <dir>` | the cache, default `<build directory>/bench-work` ("The cache") |
| `--no-verify` | no verifier on mortar's outputs ("Verification") and no runs of them ("Running the outputs"), for timing-only loops: said on standard error and in the results (`verify off`); no `output_hash` is recorded |
| `-h`, `--help` | the usage on standard output, status 0 |

At most one action. `--update-baseline` without a non-blank `--reason`,
with `--workload`, `--linker` or `--no-verify`, and `--reason` without
`--update-baseline`, are usage errors: a baseline is always a whole run,
and every change of it says why (`docs/AGENTS.md` "Review gates").

Exit status: 0; 1 when the verifier finds a mortar output wrong
("Verification") or mortar's program runs differently from GNU ld's
("Running the outputs"; any action, and `--update-baseline` then writes
nothing), or when `--check` finds a mortar link regressed or lost;
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
| `class` | all | `small`, `medium`, `large`, `extreme`, `rust`, `calibration` (exactly one workload: "Calibration"), or `on-request` (built and measured only when named with `--workload`: inputs too large to keep, generated, measured and deleted) |
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
| `run` | all built | the arguments the linked program is run with once ("Running the outputs"); `run =` runs it without arguments, no `run` does not run it; `$S` in an argument is the source directory (the unpacked archive's `root` for `cmake`, `bench/workloads/src` for `sources`, else the workload's directory) |

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
| `cxx-medium-g-static`, `cxx-medium-static` | medium | the same protoc builds with `-DCMAKE_EXE_LINKER_FLAGS=-static` |
| `cxx-medium-gc-static` | medium | protoc at `-O0 -ffunction-sections -fdata-sections`, linked `-static` and `-Wl,--gc-sections` (`CMAKE_EXE_LINKER_FLAGS_NONE`) |
| `rust-self-static` | rust | `rust-self` with `-C target-feature=+crt-static -C relocation-model=static`: static, not PIE |
| `synthetic-<shape>-static` | extreme | each `synthetic-*` above with `ldflags = -static`, the same sources and flags |
| `synthetic-library-static` | extreme | one archive of 20000 members of which `main` needs every 50th, `-static` |
| `synthetic-objects-g10x` | on-request | `synthetic-objects` at 8150 objects with `-g` (`-O1 -g`), `-static`: at least 10× protoc `-g`'s live input, the large workload of `spec/incr.md` §1.5's go/no-go rule (WP-061 stage B) |
| `fortran-static` | small | `fortran.f90` (a Fortran main calling C through `BIND(C)`) and `fortran-sum.c`, gfortran `-O1`, `-static` against libgfortran |

**Static twins.** Until M2 mortar links static, non-PIE executables only
and refuses `-pie`, `-shared` and dynamic links by name. Every workload
linked as PIE or dynamic therefore has a static twin, `<name>-static`,
built from the same sources with the same compiler flags and linked
`-static` (WP-036). The originals stay: they are M2's workloads, and
until then their mortar rows are `refused` with mortar's first line,
which fails nothing.
| `calibration` | calibration | 100 objects × 50 functions (the `objects` shape) |

The long symbol names of `synthetic-objects` (some 200 000 of 40 bytes
and more) are the "one huge string table" of `docs/VALIDATION.md`; the
million-symbol and hundred-thousand-section extremes are approached, at
the scale this machine's disk and time allow, by `synthetic-objects` and
`synthetic-sections`. Larger sizes are a manifest change.

`run` lines: `hello-*` and `fortran-static` without arguments; protoc
(`cxx-medium*`) `-I$S/src --include_imports
--descriptor_set_out=/dev/fd/1 $S/src/google/protobuf/unittest.proto`
(the descriptor set of `unittest.proto` and its imports, written to
standard output); `rust-self*` `--version`; the `synthetic-*` workloads
without arguments (their `main` exits 0) except `synthetic-objects*`,
which is not run: each of its functions calls two further on, so a run
makes exponentially many calls.

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

**Mason workloads** (M6, `spec/odr.md` O-45): `kiln-hod-medium` and
`libcxx-test` compiled with kiln's records (`-fmason-metadata`), each
linked by mortar twice, in the default mode and with `--odr=off`. The
difference is the one-definition-rule checks' cost; above 3 % of the
link's wall time (with the checks O-47 lists as on) it is a performance
bug. `--odr=off` is the row compared with the other linkers.

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
(its key lines as written, comments, blank lines and `run` dropped:
`run` changes nothing that is built), the first
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

**Default threads**: what each linker chooses without `--threads`. lld
starts one thread per processor in its CPU affinity mask
(`llvm::hardware_concurrency`); mold one per processor TBB sees (the
affinity mask); mortar `std::thread::available_parallelism` (the
affinity mask, and a cgroup CPU quota if one is set). On this machine
all three are the `cores` of the results header (12; no CPU quota is
set). GNU ld has one thread. The default configuration of a linker is
therefore not the same number of threads on another machine; the
machine id has the processor count in it.

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
  that linked once, in the same order. The linkers are **interleaved**:
  run N of every configuration comes before run N+1 of any, so a burst
  of load on the shared machine falls on all of them alike.
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

### Verification

No mortar output is timed that the verifier has not judged (WP-035;
`docs/VALIDATION.md` L3). For every workload, when mortar is among the
linkers and `--no-verify` is not given, before the warm-up:

- mortar links the workload once more, **untimed**: `mortar
  @<response> -o <scratch>/<workload>-mortar-verify.out
  --mortar-map=<scratch>/<workload>-mortar-verify.map` in the recorded
  directory, at its default threads, under the limits of a measured link
  (`process::run`, one token). `--mortar-map` does not change the
  output's bytes; the golden check below compares them with the timed
  links' anyway.
- If mortar refuses, the verdict is `refused: <first line>`; any other
  failure is `failed: <detail>`.
- If the output is outside what the verifier judges
  (`linktest::engine::not_judged`, "L3": `machine`, `pie-or-shared`,
  `dynamic`, `relocatable`), the verdict is `not judged: <reason>`: named
  and counted, never silent.
- Else `xtask plumb <out> --map <map> --map-format mortar` runs as a
  child process in the recorded directory (the inputs are the files the
  map names; `--fix-cortex-a53-843419` or `--no-fix-…` as the response
  file says, else inferred, `spec/plumb.md` §5.10). A child, so that what
  the verifier allocates never stays in the process that starts the
  measured links. Exit 0 is `clean`; 1 is `findings <n>: <rule> <count>,
  ...`; anything else `failed`.

**A finding** fails mortar's rows of the workload (both thread
configurations): `failed`, no figures, the detail `the verifier finds <n>
(<rule> <count>, ...): <first three findings> (kept: <output>)`; the
output and its map are kept in the scratch directory for `cargo xtask
plumb`. The run exits 1 after printing the table.

**The golden output hash** (`output_hash.<workload>`) is recorded only
from a `clean` output, and only when the warm-up output at the default
threads hashes the same; a different hash fails mortar's rows (`the timed
output (...) is not the verified output (...)`). A `not judged` or
`refused` workload has no golden hash.

After the table, a summary names every workload under its verdict:
verified clean, FINDINGS, linked and not judged yet (with the reason),
refused by mortar (with its first line), failed. The verdicts are in the
results header (`verify`, `verify.<workload>`). The cost and the
before/after medians are in `spec/landed/harness/WP-035.md`.

### Running the outputs

A workload whose section has `run` has its program run once (WP-036),
right after the verifier and before the warm-up, untimed, when the
verifier judged mortar's output `clean` or `not judged`:

- GNU ld links the response file once more to
  `<scratch>/<workload>-bfd-run.out` (`process::run`, the limits of a
  measured link); a GNU ld that does not link is an error of the
  harness;
- mortar's verified output and GNU ld's are each run with the `run`
  arguments, in an empty directory of their own under the scratch
  directory, with no input, `LC_ALL=C`, under 1 minute and 4 GiB of
  address space and no core file (`process::run`);
- the same exit status (or signal) and the same standard output, byte
  for byte, is `same: <status>, <n> bytes of standard output`; anything
  else is `differs: <how>` (the status of each, or the first byte at
  which the outputs differ, and where mortar's output is kept); a GNU
  ld program killed by a limit is `not run`. Standard error is not
  compared.

**A difference** fails mortar's rows of the workload as a finding does:
`failed`, no figures, the detail `mortar's output runs differently from
GNU ld's: <how>`; no output hash is recorded; the run exits 1, and
`--update-baseline` writes nothing. The verdicts are in the results
header (`run.<workload>`) and in the summary after the table.

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

**The margins, measured (WP-036).** The host the gate runs on is shared
(12 cores, load average 8 to 15). Three whole runs of one commit
(`cargo xtask bench`, 5 runs per link, at 09:42, 10:00 and 10:28 on
2026-11-15), each judged against each of the others as a baseline
(`--judge`, six pairs), measured:

- mortar's median wall time per link moved between runs by 7% to 223%
  (the median row 53%); the reference linkers' by up to 262%. The
  calibration does not follow it: the faster calibrations of the three
  runs are within 8% of one another while rows moved by a factor of two.
  One run of three was `noisy` (calibration 70.8 ms before, 35.0 ms
  after).
- On the links the gate judges steady (spread at most 5% in both runs,
  neither run noisy, drift within 25%) the largest slowdown was 28.2%
  (`synthetic-sections-static` at one thread); with the earlier margin
  of 5% it was a false `REGRESSED (time)` on two rows of one pair.
- mortar's median RSS moved by at most 2.6% (6.4 MiB,
  `synthetic-objects-static` at the default threads, where the threads
  allocate concurrently), the median row 0.6%; with the earlier margin
  of 2% it was a false `REGRESSED (memory)` on one pair.

The margins are set above those figures: **35% of time** (28.2% × 1.25)
and **4% of memory** (2.6% × 1.5); with them none of the six pairs, and
none of the three runs judged against the recorded baseline, has a
`REGRESSED` row. On this host the time gate therefore catches a
slowdown of about a third, not one of 5%: a smaller one must be shown
by a run on a quiet machine, or by many runs (`--runs`), not by the
gate. A host with less noise should measure its own margins in the same
way before narrowing them.

**A baseline recorded under load.** The drift widens the time margin
when a run's machine is slower than at the baseline and never narrows
it, so against a baseline whose calibration is well above the host's
quiet one (the 12-core host's first baseline: 109 ms before and 112 ms
after, load average 23 to 20, against 33 to 36 ms in the quieter runs) a
time is `REGRESSED` only when it is 35% above the baseline's loaded time:
the gate still catches links lost, memory growth (judged whatever the
load) and gross slowdowns, but not small ones. The baseline's `reason`
says it was recorded under load, and `docs/status/perf.md` repeats it;
it is replaced by `--update-baseline` on a quieter machine.

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
  `load_before`, `load_after`, `version.<linker>`, `verify` (`on`, `off`;
  when mortar is measured), `verify.<workload>` (the verdict,
  "Verification"), `run.<workload>` (the run's verdict, "Running the
  outputs"), `output_hash.<workload>`
  (FNV-1a 64 of mortar's warm-up output, recorded only when the verifier
  judged that output clean; the golden output check: its
  warm-up outputs at the default and at one thread must be identical, or
  both links are `failed`; `check` prints `OUTPUT CHANGED` for a workload
  whose inputs are the baseline's and whose hash is not, as information:
  a performance commit keeps it, WP-032a), and in a baseline `reason`. A
  key appears once, and all keys come before the first record.
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
link `ok` or whose run is noisy ("Noise": a noisy baseline would make
every later time `NOISY`; WP-036), records `--reason` in the header, appends `<date>\t<commit>\t<reason>`
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
| memory: median RSS above the baseline's by more than 4% and by more than 1024 KiB | **REGRESSED (memory)** | yes |
| time: median wall above the baseline's × drift by more than 35% and by more than 2 ms, on a steady measurement | **REGRESSED (time)** | yes |
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
median CPU time (user + system), median RSS, wall and RSS as ratios to
mold at its default threads (lld where mold did not link `ok`), and the
output size, or its status and detail; and the planned workloads. When
mortar links, its ratios against mold, lld and GNU ld are the ones the
release gate records. A run prints the same per-workload table on
standard output, and after it one line per `ok` link of mortar: its
figures against the fastest and the smallest of the other linkers.

`docs/status/perf.md` is generated with it, from the same baseline: the
date, commit, machine, the load average and calibration around the run;
per `ok` link of mortar, its median wall time (range) and CPU time
against the fastest other linker's median wall in the same thread
configuration (`1`: every linker's one-thread link; `default`: the
others' default links and GNU ld's one), and its median RSS against the
smallest other's, each as mortar's over the other's; the three links
where mortar is furthest behind by wall time and by RSS; the workloads
mortar does not link, with the status and detail of its row; the runs'
verdicts; and every link's figures. Numbers only.

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
| `library (m, k)` | one archive of `m` members of eight functions that call nothing; `main` calls every `k`-th member, the first included | the search of a big archive of which little is needed |

Every shape has a `main` that calls into every unit (for `archives`, into
the needed members of the first; for `library`, into its needed
members), so a link drops nothing it was meant to keep. Adding a shape
changes no other shape's output and so not `GENERATOR`.

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
`xtask/src/bench/report.rs`: `the_table`, `mortar_against_the_fastest`,
`the_status_page`. `xtask/src/bench/run.rs`: `runs_are_compared`.
`xtask/src/bench.rs`: `options`, `the_checked_in_manifest`,
`baseline_paths`. `xtask/src/bench/verify.rs`:
`a_finding_fails_mortars_rows`, `the_golden_hash_is_the_verified_outputs`
(and a run that differs),
`findings_are_read_from_xtask_plumb`, `the_summary_names_every_workload`.

The end-to-end behavior (building, capturing, replaying with the real
linkers, the census) is exercised by `cargo xtask bench` itself and not by
a unit test: it builds and fetches, which the gate does not. Its evidence
is the committed baseline and `spec/landed/harness/WP-012.md`.
