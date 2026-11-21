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
After each flip the build id is resealed (WP-101: unsealed, B1 caught
every flip and the sweep measured nothing else); the id's own bytes are
flipped unsealed, and one unsealed flip of `e_ident`'s padding must be
B1's. The cases are a C PIE (lazy and `-z now`), a C `-no-pie`, a C++
PIE, a C static-pie and a `.gdb_index` PIE (full lane). Each case prints
`<bytes> bytes, <flips> flips: <caught> caught, <equivalent> equivalent,
<survive> survive` and the flips each rule caught. Any exit status but 0
fails it: a finding on the unmutated output, a panic of the verifier on
a mutant, a byte no flip makes a new finding for (a survivor) whose
field is not in `spec/plumb.md` §12.3's table of equivalent fields, or
an unsealed flip that is not B1's. It
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
what there is), but for `kiln` (a kiln that writes the records, found by
its probe) and `hod` (a hod that writes the `reloc-model` row, likewise),
which are errors under `MASON_REQUIRE_TOOLS=1` only: the lane's gate
with an older kiln or hod skips those tests by name (WP-134, WP-129). A run of mortar without any reference notes that nothing
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
  `kiln` (a `(kiln)` compile step: kiln writing `.mason.meta` kinds 1, 3,
  4 and 5, found by a probe compile, `MASON_KILN` or the lane's build;
  without one the test is `SKIP` naming why, and a missing kiln is
  counted as missing only under `MASON_REQUIRE_TOOLS=1`; such a test runs
  on `aarch64 x86_64` only, kiln's targets: WP-134),
  `hod` (a `(hod)` compile step: hod writing MT-26's `reloc-model` row,
  found by a probe compile of a module with a PIE level that must read
  back `pie`, `MASON_HOD` or the lane's build; missing as for `kiln`,
  and the step also needs clang: WP-129),
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
compiler (`gcc`, the default, `clang`, or `kiln` for C and C++: kiln++
or kiln with `--target`, its hod route through `KILN_HOD` and
`KILN_HOD_STRICT=1`; or `hod` for C and C++: the source to IR by clang
or clang++ with `--target` and the step's options, the object by the
probed hod `--strict` at the step's last `-O0`..`-O3` (`-O0` without
one) with `-relocation-model=static` for `-fno-pic`/`-fno-pie` and `pic`
otherwise, hod writing `pie` for IR with a PIE level; a `(hod)` step is
hod's under every `--producer`) on compile steps only; linker
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
| `partial <reference> <args>` | a relocatable input made by a reference linker's partial link (WP-175): `<reference>` is `bfd`, `lld` or `mold`, run as its links run it (the architecture's `ld.bfd`, the pinned `ld.lld`, `mold`) with `-r` before the arguments; the same file for every linker of the matrix (the `-r` outputs of old build systems). It needs no `requires`; a reference that is not there skips the unit by name |
| `objcopy <args>` | the architecture's `objcopy` on an input (WP-177): the `ld -r` output an old build system passes through `--localize-hidden`, `--keep-global-symbol`, `--localize-symbol` before the final link; the same file for every linker of the matrix. It needs no `requires` (the tool guard, `scripts/ci-tools.sh`, has `objcopy` for each triple) |
| `link <driver> <args>` | a link; `<driver>` is `ld` (the linker itself) or `gcc`, `g++`, `gfortran`, `clang`, `clang++` |
| `run <program> [<args>]` | runs a linked program |
| `check <tool> <args>` | `readelf`, `nm`, `objdump` (the architecture's), `llvm-readelf`, `llvm-dwarfdump` (WP-080: `--verify`, the DWARF of a split-DWARF output), `text` (the file named by the one argument, read), `cmp` (two files, compared byte for byte), `elflint`, `ldd`, `gdb`, `lldb` ("Checks") |
| `partial mortar <args>` | (RL-6, WP-186; usable since RL-7, WP-187, lifted the `-r` refusal: AArch64 until RL-9) the same as `partial <reference>` with mortar as the linker: `mortar -m <emulation> -r --mortar-map=.linktest-partial-<line>.map <args>`, judged by L3's stage R (a finding fails the unit), with LT-R4's mode and determinism checks; the same file for every linker of the matrix. A refused `partial mortar` fails the unit (mortar must link what GNU ld links; a `partial` step takes no `exit:`). Without a mortar binary in the run the unit is skipped by name |

The build steps come before every link, run and check step. Expectations
are the indented lines under a step:

| step | expectation | meaning |
|---|---|---|
| link | `exit: 0` (default) | the link succeeds |
| link | `exit: fail` | it fails ("Expected failures") |
| link | `diag: <pattern>` | some line of its standard error matches |
| link | `no-diag: <pattern>` | no line does |
| link | `exit: info` | it only informs (`--version`, `--help`; WP-112): it succeeds and writes no output file (a stale one is removed first); no map is added and the verifier judges nothing |
| link | `print: <pattern>`, `no-print: <pattern>` | some line of its standard output matches, no line does (WP-112: `-v`, `-M`) |
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

- `check cmp <a> <b>` (WP-112): the two files, in the linker's directory,
  compared byte for byte; its one line of output is `identical`, or
  `differ: sizes <m> and <n>, first at byte <offset> (<x>, <y>)` (or
  `equal up to the shorter`). It does not fail by itself: `has:
  ^identical$` says that an option changes no byte of the output (a
  `noop` row of `docs/status/options.md`), `has: ^differ` that it changes
  some.

- `check elflint <file>`: `eu-elflint --gnu-ld <file>`; its standard
  output and error together. Every non-blank line that is not `No errors`
  must match an `allow` line that applies to the linker, whatever its exit
  status; `allow` lines are a test's allow-list for the other linkers
  (a static glibc executable needs four allow patterns, for mortar, GNU
  ld and lld alike (review-in72 T-1): `IRELATIVE` records in `.rela.plt`
  reported as an invalid type and as an invalid symbol index,
  `__ehdr_start`'s `st_value out of bounds`, and `unknown bit set in
  st_other` (AArch64's variant PCS bit); an output that keeps a SystemTap
  probe note needs a fifth, `section [N] '.note.stapsdt': unknown object
  file note type 3 with owner name 'stapsdt' at offset <o>`: elflint
  knows no note of that owner, and the note is the input's, copied
  (the AArch64 host's static `libstdc++.a` carries three, the probes of
  its exception paths; `eh_frame_overflow_then_edit_unchanged_file`,
  which throws, links them in on gcc-a64 and kiln-a64; WP-223, the
  supervisor's ruling 2026-11-18). These five
  are all: `incr-update` adds them to every elflint check, each held to
  what it says (an `IRELATIVE` record by the record's type at that index;
  the symbol `__ehdr_start`; the variant PCS bit on AArch64 only; a note
  in section `N` named `.note.stapsdt` whose owner is `stapsdt` and type 3
  ends at `<o>`, the offset elflint names being the one just past the note
  it read); other outputs of mortar need none, its RELRO
  padding being named `.pad.relro`, `spec/layout.md` L-156; WP-212
  applied WP-207's §R.6). A check with no other expectation is valid.
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
  `--Map=X`, `-Wl,-Map=X`, `-Wl,-Map,X`; for mortar, its `--mortar-map`
  in the same forms, WP-112), that map is used. Otherwise the harness adds `-Map=.linktest-plumb-<line>.map` to a `link ld` step, or
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
  (`-r`; `ET_REL`). A file that is not ELF is judged, and fails S1.
- **Stage R** (RL-5 and RL-6). A `relocatable` output is judged
  when the verifier has stage R (`mortar_plumb::STAGE_R`, a constant RL-5
  sets to `true`), read once at the run's start like stage 3's: S1 accepts
  `ET_REL` (`spec/plumb.md` §14.5 P-P9) and stage R's rules run on the
  output, its map and the link's words (the `.linktest-ld-status` line of
  a driver step, a `link ld` step's own words; mortar's from its `arg`
  records). Without it, the output is counted as not judged,
  `relocatable`, as now. Every step that makes a `-r` output is judged
  so: a `link` step with `-r`, `-i`, `--relocatable` or `-Ur` (or a
  driver's `-r`), and every `partial` step ("Relocatable outputs"; at
  RL-6 a `partial mortar` step's, not yet a reference's).
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
- **The erratum option** (WP-103, WP-102). A mortar output's option is
  its map's line (the `arg` records, `spec/plumb.md` §3.2a): the
  verifier reads it there and nothing the lane says. For a reference
  linker's output the verifier is told whether the link
  had `--fix-cortex-a53-843419` (`Map.fix_843419`, `spec/plumb.md`
  §5.10) from the words the linker received: the `.linktest-ld-status-<line>.line` words of a driver
  step, a `link ld` step's own words (`xtask::plumb::line_fix_843419`:
  the last of `--fix-cortex-a53-843419[=...]` and
  `--no-fix-cortex-a53-843419` wins, through `@file`, `-Wl,` and
  `-Xlinker`; neither is no option). A driver's default is in those
  words: GCC on Debian passes the option to every link, Clang does not.
  Before WP-103 every lane left it to be inferred from the fixes found,
  so an output with an unfixed site and no fix at all passed unjudged.
  `census` and `mason` judge mortar's outputs, so the map's line.
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
  the outputs whose line had the option (for mortar, the map's line as
  the verifier read it, `Coverage::erratum_option`), their fixed and not judged
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

### Every accepted option is spelled

Every row of `docs/status/options.md` that mortar accepts (`supported`,
`partial`, `noop`) is spelled by a link step of some test under
`tests/link` (WP-112): a long option with one dash or two, alone, as
`--opt=value` or followed by its value; a one-letter option alone or with
its value glued; a `-z` keyword as `-z kw`, `-zkw`, `-z kw=value`; in a
driver's link the words of `-Wl,` and `-Xlinker` and the driver options
GCC hands the linker as they are (`-static`, `-pie`, `-no-pie`,
`-shared`, `-s`, `-r`, `-e`, `-u`, `-T`, `-L`, `-l`, `-o`; `-rdynamic` as
`-export-dynamic`). What a driver adds on its own does not count.
`cargo xtask spelled` prints the rows no test spells and their number;
`xtask/src/linktest/spelled.rs`
`every_accepted_option_is_spelled_by_a_link_test` (in `cargo test`, so
in the quick gate; under a tenth of a second) wants none. A row's test
lives beside the tests of its subject (`tests/link/layout/execstack.test`,
`tests/link/dyn/shlib/bsymbolic.test`, ...): it shows the option's effect
against the same link without it and the references' links, or, for a
`noop`, that the output is the same bytes with and without it (`check
cmp`) and, in its comment, the row's reason why that is right.

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
| `--require-defined` of a name only a shared object defines | error | (no option) | accepted | error (`spec/resolve.md` §4.2) | `resolve/require-defined` |
| `-u` of a name only an `--as-needed` shared object defines | not needed (verifier: P-D6) | not needed (P-D6) | needed, no import (P-D7) | not needed (P-D6) | `dyn/shared/root-as-needed-unneeded` (`xfail(bfd,lld,mortar)` until P-D6 follows D-11 item 2) |
| `-u` of a name a needed shared object defines | imported | no `.dynsym` entry | no `.dynsym` entry | imported | `dyn/shared/root-as-needed` |
| `-u` of a name an unneeded `--as-needed` shared object defines, then an archive member | member extracted | root unfulfilled (P-D6) | needed, no import | refused by name | `dyn/shared/root-as-needed`, `-unneeded` |
| `-z muldefs`, a TLS and a non-TLS definition | error | error in one order | accepted | error | `resolve/allow-multiple-definition` |
| a `--whole-archive` member that is not ELF | error | error | skipped | error | `resolve/whole-archive-members` |
| an archive lazy, then the same archive whole | duplicate | duplicate | links | duplicate | `resolve/whole-archive-defs` |
| a nested `--start-lib`, a stray `--end-lib` | (no option) | error | accepted | error | `resolve/start-lib` |
| `.text` not placed while another section is (`--section-start`) | at its default | next to it | next to it | not checked | `reloc/aarch64-veneer` places both |
| `--dependency-file`: a prerequisite's indent; a file named twice | two blanks; listed per mention | one blank; once | on the target's line; once | lld's form, once | `input/dependency-file` |
| `-call_shared` | known | known | unknown option | known | `dyn/shlib/bdynamic` |
| an object's executable-stack note, no `-z execstack` | RWE stack (`--warn-execstack` warns) | refused, even under `-z noexecstack` | RW stack, warns | as GNU ld | `layout/execstack` |
| `--discard-none`, `--no-build-id`, `--gc-keep-exported`, `--strip-discarded` | some unknown (the test's qualifiers say which) | some unknown | some unknown | accepted | `resolve/discard`, `layout/no-build-id`, `layout/gc-options`, `resolve/strip-discarded` |
| `--print-gc-sections` | standard error | standard output | standard output | standard error | `layout/gc-options` |
| a `.deplibs` library (`#pragma comment(lib, ...)`) | not loaded | loaded (not under `--no-dependent-libraries`) | not loaded | not loaded | `resolve/no-dependent-libraries` |
| `--hash-size`, `--reduce-memory-overheads` | `.symtab` reordered | unknown | unknown | the same bytes | `input/ignored-options` |
| `-plugin-opt=` a plugin does not know | ignored | refused | ignored | ignored | `input/ignored-options` |
| `-Bsymbolic` in an executable | nothing | `DF_SYMBOLIC` | nothing | nothing (verifier: WP-112 V2) | `dyn/shlib/bsymbolic` |
| AArch64 static PIE: the word at `_GLOBAL_OFFSET_TABLE_` | `_DYNAMIC` | `_DYNAMIC` (no symbol) | 0 | `_DYNAMIC` | `dyn/dynamic/no-dynamic-linker` (`skip(mold,aarch64)`) |
| `-z max-page-size=0x10000`: a non-PIE base | rounded down (i386 0x08040000) | own base | own base | as GNU ld (verifier: WP-112 V3) | `layout/max-page-size` |
| `--push-state -Bstatic --pop-state -lx` | `libx.so` | `libx.so` | **no `.dynamic`, crashes** (a bug) | `libx.so` | `dyn/shlib/push-state` (`skip(mold)`) |
| `--color-diagnostics` alone | unknown | always | auto | always | `diag/diagnostic-switches` |
| `--fatal-warnings`: the severity word | `warning:` | `error:` | `error:` | `error:` | `diag/diagnostic-switches` |
| `-M` with `-Map=FILE` | the map to FILE | no FILE written | the map to FILE | the map to standard output | `input/info-requests` |
| `--threads` without N | unknown | reads the next word as N | all processors | all processors | `input/bookkeeping` |
| `-r`: a common symbol | `COM`, largest size and alignment | the same | **`SHN_ABS`, value its alignment** (a mislink, `plumb.md` A90) | `COM` (`spec/resolve.md` RV-R5) | planned (`skip(mold)` `commons`; not reached by RL-6) |
| `-r`: an IFUNC definition | kept | kept | **crashes** | kept | planned (`skip(mold)` `ifunc`; not reached by RL-6) |
| `-r`: `.symver` names; a plain reference to the default version | kept; stays `UND vfoo` | kept; bound to `vfoo@@V2` | **stripped** (A95) | GNU ld's (RV-R6) | planned (RL-8) |
| `-r`: two strong definitions | `multiple definition` | `duplicate symbol` | linked silently (A96) | `duplicate-symbol` (RV-R4) | planned (RL-2) |
| `-r -u NAME`, no definition | `UND NAME` written | not written | not written | written (RV-R3) | planned (RL-2) |
| `-r`: debug records into a dropped COMDAT copy | retargeted to the kept copy | `NONE` records | against symbol 0 (A97) | dropped, the field tombstoned (`spec/image.md` I-R7) | planned (RL-8, `r-debug-gdb`) |
| `-r`: the FDE of a dropped copy | removed, a `NONE` record left | **kept** (A91) | removed | removed with its records (L-R6) | planned (RL-8, `r-eh-*`) |
| `-r`: an input `.note.GNU-stack` with `X` | X | **N** (A93) | X | X (`spec/layout.md` L-R6a) | `relocatable/r-stack-note` (RL-7, WP-187; AArch64) |
| `-r --build-id` | note written | note written | **none** (A98) | written over the file (I-R8) | `relocatable/r-busybox-clang` (RL-7, WP-187) |
| `-r -e SYM` | `e_entry` = `SYM`'s offset | the same | 0 | 0 (L-R7) | planned (RL-2) |
| `-r`: `SHF_GNU_RETAIN` and plain pieces of one name | two sections | one, `R` kept | one, **`R` dropped** (A99) | two (L-R1a) | planned (RL-8) |
| `-r`: `SHF_LINK_ORDER` pieces | `sh_link` kept | kept | **0** (A101) | kept (L-R1) | planned (RL-8) |
| `-r`: `SHF_MERGE` pieces of two entry sizes | one section, `SHF_MERGE` dropped | **`AM`, entsize 0** (A100) | two sections | GNU ld's (L-R1a) | planned (RL-8) |

### Relocatable outputs (M4-r; RL-6, WP-186)

**What RL-6 landed** (record `spec/landed/harness/WP-186.md`): L3
judges a `link` step's `-r` output by stage R (`mortar_plumb::STAGE_R`);
`partial mortar` (below, LT-R4 included); `cargo xtask plumb --compose`
runs P-P1 on a direct link, the link through `-r` and the `-r` output's
map (LT-R2 by hand: `<D> <D.map> <V> <V.map> <R> <R.map> --map-format F
[--r-format F]`, `R` named as `V`'s map names it); the census's `-r`
class; the smoke tests `tests/link/relocatable/`, whose final links are
written in the test, so the matrix links each `-r` output with every
linker (LT-R1 and LT-R3 for those tests). **Not yet:** the lane adding
the two final links and the direct form itself (LT-R1's automatic form,
LT-R2 in the lane), the `skip(mold)` reasons' check (LT-R3), LT-R5, and
stage R on a reference's `partial` output (it needs the tests' mold
anomalies declared first); P-P1's own tests are
`crates/mortar_plumb/tests/compose.rs`.

Record `spec/landed/layout/WP-154.md` §D.2, as rules. A `-r` output is
judged twice: by stage R on its own (L3), and by what it does in a final
link. The second is the lane's, since no rule of one output shows it.

**LT-R1 (two final links, both run).** For each `-r` output of a link
test (a `link` step with `-r`, or a `partial` step), the lane links it
finally twice: by GNU ld of the architecture and by mortar, each with
the test's final step words (the next `link` step that names the `-r`
output), and runs both programs with that step's `run` expectations.
Both are judged by L3 like any final link. A test whose `-r` output no
final step names gets one: `link gcc -nostdlib -static <rout> -o
<rout>.final` with the C runtime's `_start` only when the test says
`final: <driver> <args>`; without it the unit fails ("a -r output that
no link consumes", a test-file error), so that no `-r` test checks only
the `-r` bytes.

**LT-R2 (the direct form, P-P1).** The lane also links the **direct
form**: the final step with the `-r` output replaced, at its place in the
line, by the `-r` step's inputs, in their order (an archive of the `-r`
line stays an archive). For each of the two final linkers, the direct
link `D` and the link through `-r` `V` are compared by `spec/plumb.md`
§14.5 P-P1 (`mortar_plumb::compose`, RL-6's, §14.8 Q-R2), with the
`-r` output's own map: GNU ld's `D` with GNU ld's `V`, mortar's with
mortar's. A finding fails the unit. P-P1's exclusions (a)-(d) are
**named skips of the comparison only** (the two links still run and are
judged): the unit prints `composition not judged: (<letter>) <reason>`
and counts it; the reasons are P-P1's, decided by the verifier from the
two lines, never declared by the test. Both programs' outputs are
compared too: `V`'s run must print and exit as `D`'s ("the program
through -r differs from the direct link").

**LT-R3 (the mixed-toolchain case).** Each reference's `-r` output of
the same inputs (`partial bfd`, `partial lld`, `partial mold`; plus
`partial mortar` since RL-7, WP-187) is fed to mortar's final link and to
GNU ld's, and LT-R1 and LT-R2 apply to each. mold's `-r` outputs are
known wrong in four named ways; a test whose inputs have one of them
says `skip(mold)` with the reason's name, and the lane checks that the
inputs have it (a `skip(mold)` whose reason the inputs do not show is a
stale skip, a failure):

| reason | the inputs have | mold's `-r` | `spec/plumb.md` |
|---|---|---|---|
| `commons` | a common symbol (`SHN_COMMON`) | makes it `SHN_ABS` | A90 |
| `ifunc` | an `STT_GNU_IFUNC` definition | crashes (SIGSEGV) | — (no output) |
| `symver` | a `.symver` name (`@` in a symbol name) | strips it to the base | A95 |
| `duplicate-strong` | two strong definitions of one name | links them silently | A96 |

Other mold anomalies at `-r` (A92, A94, A97-A99, A101) are declared as
any anomaly is, `#@ anomaly(mold): <id>`.

**LT-R4 (mode and determinism).** For mortar's `-r` output the lane
checks the file's mode bits, `0o644` under the harness's umask 022
(`spec/image.md` I-R3; GNU ld, lld and mold measured the same), and
links the `-r` line a second time into another name and compares the two
files byte for byte (I-R11). Neither is a verifier rule (§14.7 of
`spec/plumb.md`).

**LT-R5 (no-ops).** A test of a C-R2 option (`spec/cli.md`: accepted at
`-r`, no byte changed) links the `-r` line with and without it and
`check cmp`s the two (`r-noop-options`).

**LT-R6 (a `NONE` record keeps a section).** Per target,
`relocatable/none-record-kept` (`spec/image.md` I-R5's `r-none-record-kept`;
`tests/relocatable-design/exp/rl0/none.sh`): `_start` with `.reloc .,
R_*_NONE, keepme` (`R_AARCH64_NONE`, `R_X86_64_NONE`, `R_386_NONE`),
`keepme` in its own section `.text.keepme` of a second object; the two
partially linked by `partial bfd`, `partial lld` and (since RL-7)
`partial mortar`; each `-r` output final-linked with `--gc-sections
-e _start` by GNU ld and by mortar (LT-R1), `check nm` finding `keepme`
in every final output, and the direct link's the same. Measured
(WP-180): GNU ld's, lld's and mold's `-r` each carry the one record, and
`keepme` survives every final link through them; without the record
both GNU ld and lld collect it. Stage R's P-P3 requires the record in
every `-r` output.

**The census at `-r`.** `VARIATIONS` gains the driver `-r` lines of
record §A.2 that a reference links (`r-plain` `-r`, `r-nostdlib`, `r-g`,
`r-pthread`, `r-pie` `-r -pie`, `r-no-pie`, `r-static`, `r-s`, `r-flto`,
`r-no-pie-nostdlib` `-no-pie -nostdlib -Wl,-r`, `r-gc-e`
`-r -Wl,--gc-sections,-e,a`; `r-fuse-lld` and `r-fuse-mold` run the
same words through the wrapper of that linker), and the ones every
driver refuses are not variations. For each, mortar links the line;
the line passes when it passes as above with the program being LT-R1's
final link of the `-r` output by GNU ld and by mortar, compared with the
reference's own `-r` output through the same finals, and stage R is
clean. A refusal is a pass only with a key of `OUT_OF_M4R` (the
refusals `spec/diag.md` §4.4 lists, each printed). The census's last
line adds the `-r` counts: `census: <n> lines (<r> -r), ... <p> pass
(<rp> -r), <f> fail (<rf> -r)`; RL-6 lands the counting with every
`-r` line refused by mortar (`OUT_OF_M4R` held `-r` itself until RL-7)
and RL-7 to RL-12 move the lines to pass. Since RL-7 (WP-187)
`OUT_OF_M4R` holds the later packages' names (`section groups with -r`,
`{machine} output with -r`, `-s with -r`, ...) and C-R4's lasting
refusals (`--gc-sections with -r`). A `-r` line mortar links is,
at RL-6, judged by stage R and final-linked by the driver (GNU ld) from
mortar's and the reference's `-r` output, both programs run and
compared; the final link by mortar and P-P1 on the census lines are
RL-7's to add.

### Shared-object outputs

A link test can link a shared object (`-shared`) and observe what it
does in a process: linked against by hosts that each linker links, run
plain, under an interposer and through `dlopen` (WP-120). What the
library binds to its own definitions and what an interposer preempts
(`spec/dyn.md` D-112) is then observed, not inferred.

**LT-1 (the host step).** `host <driver> <args>` links an executable
against the libraries before it, as `link` does, with the same driver
and link-line forms ("Links") and the expectations `diag`, `no-diag`,
`print` and `no-print`. It takes no linker qualifier and no `exit`: a
host is to link (it is the library's `link` that may be refused). It is
not a build step. `cargo xtask spelled` counts its lines as link lines.

**LT-2 (the library part and the pair part).** The steps before a test's
first `host` are its library part; the steps from there on are its pair
part, which runs once per pair (LT-3). The pair part has no `link` step;
a `host` needs a `link` before it, or a linked fixture (LT-4) that links
the library. A test without a `host` step is all library part and runs
as before.

**LT-3 (the pairs).** A pair is the linker of the library and the
linker of the host. A reference's record runs its library part in its
directory, then its own pair there, then, when GNU ld is in the run and
the reference is not GNU ld, its library under a GNU ld host. mortar's
record first hosts, with a mortar host, the library of each reference
whose record passed (whatever becomes of mortar's own library), then
runs its library part, its own pair and its library under a GNU ld
host. A pair `A+H` whose host another linker links runs in
`<unit>/<A>+<H>`, a copy of `A`'s directory as `A`'s library part left
it; `A`'s own pair runs in `<unit>/<A>`. Each `host` is linked by the
pair's host linker and judged by L3 as that linker's link; each `check`
is the record's. A result of a pair other than the record's own begins
`[library A, host H]`.

**LT-4 (linked fixtures).** A fixture whose name begins `shared-` may
link libraries: after its build steps it has `link` steps (no `exit`:
its library is to link) and `check` steps on them. Its inputs are built
once per architecture as any fixture's; its links and checks then run
once per architecture and linker, in a copy of them, the first time a
unit of the run asks, each link judged by L3 with the run-wide defaults
(a test's anomalies do not apply; the L3 counts are that unit's). A
linker's directory starts with what it linked (the harness's
`.linktest-*` files left behind), then the unit's inputs over it. A
fixture that links whose name does not begin `shared-` is an `ERROR` of
each test that names it. A linked fixture's links have no
static-identical row.

**LT-5 (the comparison with the GNU ld pair).** Every run of the pair
part is compared with the same run step of the baseline pair: the GNU
ld pair (its library and host linked by GNU ld), or, when GNU ld's
record does not pass, the own pair of the first reference whose record
passes. A run passes only with the baseline's end (exit status or
signal) and standard output, as well as its own expectations; a run the
baseline did not make is not compared. Before there is a baseline, a
reference's pairs are compared with its own pair. The detail names it:
`printed "7 1\n", the bfd/bfd pair printed "7 7\n"`. The pair part's
output is not the self-check's ("The self-check" compares the library
parts').

**LT-6 (run hygiene).** Every program a test runs is one the test built
(the hosts, a `shlib` interposer). A run step's `env` reaches that
program alone: natively the program is started as `/usr/bin/env
NAME=value... <program> <args>` behind the limits' shell, so an
`LD_PRELOAD` is set on the program's own `exec`, never in the
environment of the harness, the shell or a compiler; under qemu it is
qemu's `-E`. Each run has the test's time limit ("Runs"). Everything a
pair writes is under the unit's directory.

**LT-7 (the census's `-shared` lines).** `cargo xtask census` has three
`-shared` variations of `-fPIC` objects (`shared`, `shared-soname`,
`shared-ldshared`: `-Wl,-O1 -Wl,-Bsymbolic-functions -Wl,-z,relro`; not
in `--quick`, kiln not run). A line's library is loaded by `dlopen` from
a loader the line's driver links with GNU ld; mortar's library loads as
the reference's (exit status and output) and the verifier is clean, or
mortar refuses the line by name (`OUT_OF_M2`: what a `-shared` line
may still ask for that has not landed). The lane
prints `census: -shared: N lines, L linked, R refused by name`.

**LT-8 (the real builds' `-shared` lines).** `cargo xtask realbuild`
prints, after its comparison with the baseline, `against <baseline>:
-shared: N lines, F newly linked`: the run's lines of the `shared` kind,
and those linked now that the baseline did not have linked.

**LT-9 (static-identical).** A `.so` output is a row like any other,
keyed `(test, arch, line, variant)` by the line of its `link` step; a
host's row is keyed by the line of its `host` step. The digest mode runs
mortar's own pair alone (no reference library, no GNU ld host), and its
run and check steps not at all. A linked fixture has no rows (LT-4).
Existing rows do not move: a test without a `host` step runs as before.

**Measured (WP-120).** The design's D-112 table (`tests/shared-design/
exp2/bsym2`, run with lld 22.1.8) was run again with lld 19.1.7, Debian
13's own (the `ld.lld` of `gcc -fuse-ld=lld`): all 17 lines on AArch64
and on x86-64 (under qemu) print the same with both, and lld 22's equal
the design's result; both refuse `--dynamic-list-data`, `-cpp-new` and
`-cpp-typeinfo` as unknown. The lanes have no host with 64 KiB pages
(the gap WP-118 reports): no shared-object test runs on one.

**Tests.** `xtask/src/linktest/format.rs`: `linked_fixtures`,
`host_steps_and_the_pair_part`. `xtask/src/linktest/engine.rs`:
`the_pairs_of_a_record`, `a_pairs_run_against_the_baseline`,
`a_native_runs_environment_is_the_programs_alone`.
`xtask/src/census.rs`: `shared_lines_are_counted`.
`xtask/src/realbuild/outcome.rs`:
`shared_lines_that_newly_link_are_counted`. `xtask/tests/linktest.rs`
`the_pairs_of_a_shared_library_observe_preemption`: fake linkers; a
library whose own call an interposer preempts passes with every pair; a
deliberately wrong expectation (the library's own value under the
interposer) fails every reference and the test is `ORACLE`, exit status
3; a mortar that links its library `-Bsymbolic`, under a test that
expects only the references' output, fails by the comparison with the
GNU ld pair. The smoke tests `tests/link/shared/` (`interpose`,
`dlopen`, with the fixture `shared-hello`) run the three references in
full and are mortar's expected failures naming WP-123, whose join only
removes the markers.

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
`diag` directive; their units come from the test producer (`spec/meta.md`
§12), and the `kiln-*` tests' from kiln itself (`(kiln)` steps with
`-fmason-metadata`, WP-134). `cargo xtask mason producers` runs the
corpus with each producer (`--producer gcc|hod-objcopy|hod|mixed|kiln|
kiln-hod`): `kiln` and `kiln-hod` compile every C and C++ step with kiln
(through llc, and through hod at the step's own options), and an object
a `meta` step takes gets kiln's own container at the level its
description implies (`detail` for `level detail` or a typed C++-linkage
function row, else `default`) instead of the description's rows. The **L11 lane** (`tests/mason/`,
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
`xtask/src/linktest/engine.rs`: `two_files_compared` (`check cmp`),
`the_output_file_of_a_link_line`,
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

The times mortar measures change from run to run too, and the options
that print them are tested (WP-112). So, before the paths are replaced,
the standard error has, in a line holding `: time-phases: `
(`--time-phases`, image.md I-88), all after its first `: wall `; in a
line holding `: odr-stats: ` (`--odr-stats`), all after its last
`; time `; in a line beginning `incremental: ` (`--incremental-report`
without a file), all after its last ` time=`; and in a line beginning
`delta time `, all after those words, replaced by `<time>`
(`normalise_stderr`). The counts on those lines are kept. The map
digested is always `.linktest-plumb-<line>.map`: a link that names its
own `--mortar-map` has `-` in the map column, as before WP-112, so that
no row of the baseline moves.

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
not compared (a note says how many) unless `MORTAR_REQUIRE_TOOLS=1`
requires that tool (a test that `requires: kiln` or `requires: hod`
without `MASON_REQUIRE_TOOLS=1`, "Missing tools", is not compared
either). An
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

## L10

`cargo xtask l10 [--quick] [--seed <n>]... [--steps <n>] [--hops 2|3]
[--project <p>,...] [--compiler gcc|clang|kiln,...] [--target
a64|x64|i386,...] [--config <name>,...] [--edits <row>,...] [--subject
mortar|floor|<path>] [-j <n>] [--keep] [--replay <file>]...
[--regressions] [--minimize <file>] [--self-test] [--mortar <path>]` is
L10, the random edit-sequence lane of incremental linking
(`spec/incr.md` IN-63, `docs/VALIDATION.md`; WP-067). The command
(`xtask/src/l10.rs`) builds mortar (and the floor's example,
`crates/mortar/examples/incr_floor.rs`, when the run needs it), holds
`-j` jobserver tokens for the lane's workers and runs
`tests/l10/l10.py`; the work directory is `<build directory>/l10`
(`cache/`: objects by content, kept between runs; `capture/`: each
configuration's linker words; `seq/<label>/`: a running sequence, removed
when it passes unless `--keep`; `failed/`: the sequence files of
failures).

**A sequence** is (project, configuration, seed, steps). Its steps come
from `tests/l10/edits.py`: one row per edit, each an assignment of the
project's knobs (and optionally an action on the files: `touch`, a
same-size rewrite in place right after the previous link, the output
rewritten with its own bytes right after its link); a two-hop row
schedules its inverse one to three steps later and, half the time, the
edit again after that (review-in77's B-3 and B-4 shapes, where the stale
datum shows on the second or third update); `--hops 3` (and the nightly
run's seed 3) makes every two-hop edit three-hop: the edit, its inverse,
the edit again (WP-207 §D 14). `random.Random` seeded with a string is
the same on every host and Python, and the lane's workers only run
sequences, so the same seed gives the same steps and the same verdict on
any host and at any `-j`. A configuration is
`<gcc|clang|kiln>-<a64|x64|i386>-<O0|O2>-<g|nog>-<fs|nofs>-<static|pie>`
(kiln: the 64-bit targets, `-O0`, an AArch64 host, `MASON_KILN` and
`MASON_HOD` set); x86-64 and i386 programs run under qemu. The linker
words are captured once per configuration from the compiler driver
through a `-B` wrapper (as the census does). `--gc-sections` is on by
default with `-ffunction-sections` and off without; the `option-flip`
row flips it.

**Projects.** `gen200` (`projects/gen200/project.py`): 200 generated
units (C and C++), `main.cc` linked directly, three archives (lib1.a
thin), a Fortran unit with a COMMON block (a C stand-in without the
target's gfortran), a lazily extracted member; templates and inline
functions (COMDAT keepers), TLS, constructors whose `.init_array` order
the program prints, a `__start_l10_reg`/`__stop_l10_reg` registry it
iterates, `-fcommon` C. Its transcript depends on every unit.
`corpus40` (`projects/corpus40/`): R2's 40-unit corpus
(`spec/incr.md` §1.1) with its edits e1-e10, a regular and a thin
archive. `protoc` (only by `--project protoc`): the bench's protoc
objects (`cxx-medium-g-static`), read in place; an edit recompiles one
source of the bench's tree into the cache; only
`gcc-a64-O0-g-nofs-static`, skipped by name when the bench has not
prepared it. `gonogo`: a named TODO (its 1.1 GB outputs do not fit the
lane's scratch; the reason is printed).

**A step** applies the edit, compiles what changed (the content-addressed
cache: only what changed), installs the inputs in the work directory W
(an unchanged input keeps its stamp; a changed one is a new inode, as a
compiler writes it) and links three times in W: F, the plain full link;
G, the incremental-mode full link (`--incremental=full`); X, the
SUBJECT (`--incremental --incremental-report=`). Then the step's
snapshot (the inputs as hard links, made private before any in-place
rewrite; X's, F's and G's outputs and maps; X's report) is judged by
`judge.judge_step(tools, x, f, base, new) -> Verdict` (`tests/l10/
judge.py`; H-1's judge puts IN-79's checks behind the same function):

1. exit: X's status and diagnostics equal F's (lines naming `incremental`
   dropped, the output's name as `<out>`);
2. verify: `cargo xtask plumb` accepts X's output with its map;
3. equiv: `plumb --equiv` of X against F when X's map claims coverage
   (`incremental`); otherwise (an IN-12 plain link) X's bytes equal F's;
4. run: the two programs (qemu off the host) print and exit alike;
5. oracle: IN-63a's `tests/incr-oracle/oracle.py`, as a library, over
   the previous step's G and this step's G with X's report (it passes a
   fall-back; two adapters: thin archive members are read from their
   files, and archive members are identified by name and ordinal, not by
   offset).

L10 and the IN-63a oracle lane (`tests/incr-oracle/run.py`: `cargo
xtask incr-oracle`, and its quick subset `cargo test -p mortar --test
incr_oracle`): until WP-225 M6 these lanes judge the format-4 dry run of
a testhooks build (`mortar --features testhooks` in `<target>/testhooks`,
with `MORTAR_INCR_STATE4=1`); at M6 they judge the update or go, by
ruling.

The report's reason is recorded. A sequence stops at its first failing
step; its sequence file goes to `failed/`, and `--minimize` reduces it
by delta debugging (over steps, then each step's knobs and action) to
the same failing check, writing `tests/l10/regressions/<label>.json` with
the seed and the failure's first line; `--regressions` replays every one
(the nightly run does): a regression fails until what it records is
fixed, and then guards it. A base link mortar refuses by name (`unsupported
feature`) ends the sequence as REFUSED; a sequence whose links all fail
alike is FLAGGED.

**Output.** Per sequence `L10 RESULT ... verdict=PASS|FAIL at=<step>
<check>|REFUSED (...)`, `L10 STATS ... updates=<n>
fallbacks=<reason>:<n>,... failed-links=<n>` (FLAGGED, in one line, when
a passing sequence never updated: today every one), `L10 COST ...
wall= links= load= [inputs= links= snapshot= exit= verify= equiv= run=
oracle=]` (seconds per phase); per edit row `L10 EDIT <row> steps=
reasons= hops=<1>/<2>/<3> [shape=..] [wp207=..]`; `L10 SUMMARY`. Exit
status 1 when a sequence fails.

**Subjects.** `mortar` (the product path), `floor` (`subjects/floor.py`:
the test path's same-size update, WP-061, as the subject: a refusal is
named in the report), or any executable; the self test's deliberately
wrong ones are in `subjects/`. The seam for H-2's faults is the step's
`fault` field, always null here (the runner refuses a non-null one).

**`--self-test`** (the lane's own test, IN-63's last sentence): a
subject that hands back the previous output at a seeded step, and one
that flips one relocated word of an unchanged unit, are each found and
minimized to at most 3 steps; a subject that drops one diagnostic is
found by the exit check; the floor on same-size edits passes, and each
step is an update or a refusal the floor names.

**`--quick`** is the pre-merge smoke, run by `cargo xtask ci` after the
link tests (`QUICK` in `l10.py`); IN-63's budget is 5 minutes of wall
time. Without `--quick` and without a selection, the nightly run: every
project of `PROJECTS` x the configurations of `nightly_configs()` x
seeds 1-3 (3 three-hop) x 30 steps, then the regressions and the self
test. Under `MORTAR_REQUIRE_TOOLS=1` a skipped configuration is an
error. Measured (WP-067, 12 shared cores at a load of 6-15): the
smoke takes 51 s warm, 65 s with an empty object cache (9 s compiling
550 objects); the nightly run 120 sequences, about 10,000 links, 44
minutes at `-j 4`, scratch peak 1.6 GB under `.build/target-*/l10/`.
The oracle and the equivalence check are the largest phases (a quarter
of the time each). `--project protoc` (by name; 6 steps) takes 100 s.

## incr-update

`cargo xtask incr-update [--quick] [--only <scenario>,...] [--target
a64|x64|i386,...] [--compiler gcc|kiln,...] [--subject
mortar|<name>|<path>] [-j <n>] [--keep] [--self-test]` (H-1, WP-223;
`xtask/src/incr_update.rs`, the lane `tests/incr-update/lane.py`, its
README the scenario format). It builds mortar, and the `mortar` crate
with `--features testhooks` into `<target>/testhooks` (the subject:
`MORTAR_INCR_DIRTY=<file>` makes an update write its dirty ranges,
`0x<offset> 0x<length>` a line; built without debug information, a 34 MB
binary in a 161 MB directory), `mortar_incr`'s example `incr_state`
(release, beside mortar: the comparator of `state`'s (d0), passed to
the lane as `--incr-state`), holds `-j` jobserver tokens for the
lane's workers and runs the lane under `<target>/incr-update`.

**Scenarios.** `tests/incr-update/scenarios/<name>/`: a first link in
incremental mode and three updates, hop 1 the edit, hop 2 its inverse,
hop 3 the edit again (a scenario's `hops:` may say otherwise), each
scenario's `proves:` line saying what it proves once updates exist.
They are WP-207 §D's series, WP-211's
`growth_step_moves_unchanged_then_edit_referrer` and
`member_round_shortcut`, IN-75a's warning series (with and without
`--fatal-warnings`), and `tests/incr-review`'s shapes ported to
scenarios (S1-S4, w-meaning's W1, W1b, W6, W8-W11, w-archive's A1-A3;
the scripts themselves drive the floor and stay as they are). Each is
built by GCC and by kiln where the shape has a C++ form (C with
`-fcommon` and the assembly shapes are GCC's; each says why in a `not
kiln:` line), on AArch64 and x86-64 (x86-64 under qemu on an AArch64
host); i386 runs only when `--target i386` asks for it, with the same
scenario files, and an i386-only failure is parked (operator,
2026-11-17).

**A hop.** The inputs are installed in the run's directory `W` (a
changed object as a new inode, an unchanged one untouched) and copied
into `h<i>`; three links of the same inputs: F, the plain full link
(`h<i>/f`); G, the incremental-mode full link (`--incremental=full`,
`h<i>`, IN-63a's link); X, the subject (`--incremental
--incremental-report=h<i>/rep.txt`, `MORTAR_INCR_DIRTY=h<i>/dirty.txt`,
in `W`, whose output and state persist from hop to hop).
`tests/l10/judge.py`'s one `judge_step(tools, x, f, base, new,
hop=Hop(...))` judges X; with `hop=None` it is L10's judge, unchanged.
Its checks, in this order, each `pass`, `FAIL <why>` or `n/a <why>`:

| check | IN-79 | what |
|---|---|---|
| `exit` | (d) | X's exit status equals F's; a failed X leaves the state it found (IN-48); with `--fatal-warnings` the update fails as F fails |
| `stderr` | (d) | X's standard error equals F's line for line, in order, after dropping X's lines that begin with `incremental:` (the report line, which a full link does not print); nothing else is normalised: F links the same file names in its own directory, so the texts agree byte for byte (IN-75a). A failure names the lines dropped, repeated or added and the first differing line |
| `dirty` | IN-75 | every byte in which X's output differs from the previous output lies in a range of `dirty.txt`; the ranges are sorted, disjoint and inside the file. `n/a (full link: <reason>)` for a full link and for the floor; `incremental: update` without `dirty.txt` is a FAIL |
| `elflint` | (b) | `eu-elflint --gnu-ld`: every complaint matches one of "Checks"'s five allow patterns, each held to what it says (an `IRELATIVE` record of `.rela.plt`/`.rel.plt` by its type; `__ehdr_start`; the variant PCS bit only on AArch64; a note of owner `stapsdt`, type 3, in a section named `.note.stapsdt`, ending at the offset named). A sixth pattern is a finding for the supervisor, never an addition. Before `verify`: no corruption was found that elflint refuses and `verify` accepts (WP-223), so the outside tool speaks first |
| `verify` | (b) | `xtask plumb <output> --map <the update's map>` |
| `equiv` | (b) | `xtask plumb --equiv` of X's map and F's |
| `run` | (a) | the program's status and output equal F's |
| `oracle` | (c) | IN-63a between the previous hop's G and this hop's, against X's report; `n/a` without a previous full link or without a dry run's prediction |
| `state` | (d) | until (d) proper (`state_equals_capture_of_its_placement`, IN-78, which needs `P_U` from the map): **(d0)** (WP-267 P-4), `incr_state compare S' G`, `S'` the state X wrote, `G` the state of the hop's own G link (`h<i>/prog.mortar-state`). **Exact** (`pass (d0 exact)`) on an identity hop (X's map and G's are equal line for line outside `arg` and `symblock`) whose previous hop was a full link (not `base_was_update`); on every other update hop equal **outside IN-78's placement rows** (`pass (d0 outside placement: <n> rows excluded)`): the tables `CHUNKS`, `ORDER`, `HOLES`, `PATCHES`, `REDIRECTS`, `REGIONS`, `SYMTAB`, `DEFINED`, `SYM_PLACE`, `SIZES`, `SIZED`, `SEGMENTS`, `LAYOUT`, `OUTSECS`, the columns `SECTIONS`' place, `FILES`' `eh_offset` and `eh_capacity`, `FRAGS`' offset, `META`'s output size, and `SLOTS`' order. A failure prints the rule and the first differing row; the comparator's output is saved to `h<i>/d0-compare.txt`. Without the tool: `n/a ... no incr_state`, never a pass |
| `duty` | (d) | `duty_list_covers_differences` (IN-78) under (d0): every row in which `canon(S)` (a copy of the state taken before X) and `canon(S')` differ is a table of a cause of IN-78's derived table, held as data in `tests/incr-update/duty.py` (a unit test checks it against this spec's table, a name a cell gives only to say the cause does not change it held out by the spec's own words); `STRINGS` held out (a string is its rows'); `S'` stands for `C(U)` where (d0) holds |
| `islands` | (d) | WP-272 (WP-271 review R-2): on every hop with a state (an update or a full link, the first link included), the state's island rows against the OUTPUT's bytes, read with `incr_state rows S' CHUNKS PATCHES REDIRECTS ...`: input-section addresses from the map's `place` records (the verifier judges them), each also equal to the state's `SECTIONS` place; chunk addresses from `CHUNKS` (an island inside an executable section's bytes, equal to the map's `capacity` record). **State to bytes:** each `REDIRECTS` row's word at F (its section's address + offset) is a `b` (a thunk's: `b` or `bl`) to its patch (the chunk's address + the `PATCHES` offset); a 843419 patch (kind 2) names the redirect's own site, its first word is a load/store with an unsigned immediate (W4') and its last `b F + 4`, F from the `PATCHES` row's section and site offset; a patch with no redirect is that form or zeros (a dropped patch, R-6); a thunk (kinds 0, 1) decodes as a64.md §9's three words and branches to the key's `.symtab` address + addend or its `slot plt` record (a local or versioned key is not resolved here: the hop is `n/a (local key: ...)` or `n/a (versioned key: ...)` by name once every other row has passed, never a pass and never a FAIL; WP-276 review, ruling 6 (b)); patches lie inside their island and do not overlap. **Bytes to state:** an island word no `PATCHES` row covers is zero; every `b`/`bl` word of an executable section whose target lies in an island is the word at F of exactly one `REDIRECTS` row (model.md §4.4: every thunk site has a redirect). `pass (<r> redirects, <p> patches (<u> unused, <t> thunks), <i> islands, <b> branches into them)`; `n/a (x86-64: no patch islands)`, `n/a (i386: ...)`, `n/a (no island chunk)` (no `Patches` chunk: a link without `--fix-cortex-a53-843419` and without thunks), `n/a (no island: <n> Patches chunks, all empty)`, `n/a (no incr_state)`; a mismatch is `FAIL` with the row and the word |
| `expect` | §7 | only in a scenario with `expect-full: <reason> (<detail>): <why>`, after hop 0: a hop that may not be an update by rule is that named full link (`pass`) or a fall-back that comes first (`n/a`, e.g. `small-output`); `update` or the reason with another detail word is a FAIL. The `--gc-sections` scenarios say `update-not-implemented (live)` (IN-71a, WP-215 seam 3; WP-238 lifts it): no scenario expects a gc UPDATE |

**What (d0) does not judge** (WP-267 review §6), so what only (d)
proper or another check can catch: `FILES.round` (canon writes 0; the
support property is not checked), `META.compactions`, `build_id_len` and
the other fields `canon` zeroes (the `META` history and binding, the
`STAT` output row), `STAT` (G's inputs are copies, with other inodes and
times), `LEAVES`, the link's directory in `STRINGS` (G links in `h<i>`, X
in `ws`, a name of the same length so no other string moves; the row
must be equal once the directory is replaced), the placement rows above
on every hop that is not exact (three of the four stale-datum defects
were in such rows), and rows seen only as set differences (a table whose
row counts differ is compared as sets: duplicates collapse). `P_U` is not
read (`P_U: decision records not read`), so no decision row is compared
with the update's placement. The run's summary adds `incr-update: state
(d0, WP-267 P-4): exact:<a> outside-placement:<b> n/a:<c> FAIL:<d>`. and `incr-update: islands (WP-272): pass:<a> FAIL:<b> n/a:<c> (<why>:<n>, ...)`. A check that
cannot run because the link failed is `n/a the link failed`. The run's
summary counts the `n/a`s by check:

```
incr-update: <n> scenarios, <m> hops: <p> PASS, <f> FAIL, <k> n/a (<check>:<count> ...); <r> runs, <s> not here (scenario: <a>, tools: <b>); <t>s
```

and `INCR-UPDATE RESULT <scenario>.<compiler>-<target> verdict=PASS|FAIL
at=hop <i>: <check>: <why> reasons=<reason>:<count>,...` a run. A run's
directory is removed when it passes unless `--keep`. Under
`MORTAR_REQUIRE_TOOLS=1` a configuration that cannot run here is an
error.

**`--self-test`.** The judge's units (no link: the dirty-range rule, the
stderr comparison, the five allow patterns, the (d) machinery with a
live comparator, the duty table against the spec, `expect`), then one wrong
subject per check (`tests/incr-update/subjects/`), each a wrapper of the
testhooks mortar wrong at one hop, which must be found by its check at
that hop and by no earlier one: an exit status (`exit`); a warning
dropped, a warning repeated (`stderr`); a byte written outside every
range, a ranges file missing a range, no ranges file (`dirty`); an
output elflint rejects (`elflint`); a word of `.text` flipped, a
`.symtab` local moved (`verify`); a program that cannot run (`run`); a
report that leaves out a change (`oracle`). The `islands` check's own units (in `--self-test` and `--quick`; `lane.py` `island_units`) link `scenarios/islands_843419` (one 843419 site that gets a patch) and judge it clean (`pass`) and with one thing mutated, each a `FAIL`: a `REDIRECTS` offset; that offset and the patch's site offset together; the word at F left unfixed; the site's `SECTIONS` place stale; the patch's words zeroed; an extra `b` into the island; the patch rows lost (the island's words uncovered); without an AArch64 GCC or `incr_state` they are one `SKIP` line by name. The previous hop's output
and map handed back (`prev_output`) is caught by `verify`, not by
`dirty` or `equiv`: the old map describes inputs that changed, and
`verify` runs first (the supervisor's ruling, 2026-11-18).

**`--quick`** is one target (the host's), GCC, five scenarios (`QUICK`
in `lane.py`), and the units; measured 5 s warm (20 hops at `-j 4`, 12
shared cores at a load of about 11; WP-223), so it adds well under the
60 s a `ci --quick` step may add. The gate runs `--quick` after L10's
smoke in its `linktest` step, in `ci --quick` and full `ci` alike (the
supervisor's ruling, 2026-11-18). The full matrix (36 scenarios, 108
runs, 432 hops on AArch64 and x86-64, GCC and kiln) is not a `ci` step:
it runs in the supervisor's batch gate script, `cargo xtask incr-update`
with no option. It takes 164 s warm at `-j 4` on the 12 shared cores of
the AArch64 container at a 1-minute load of about 11, and 112 s at a
load of about 14 (WP-223's record; 432 hops). `--crash` (H-2) will run each update once
per `MORTAR_INCR_CRASH` point (IN-77's list, the points between a
write and its sync included) and then the next link.

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
| jobs | `tests`: `cargo xtask ci --step fmt --step clippy --step test`; `links`: the other steps | the same |
| fan-in check | `check` | `check-x86_64` |

1. **Two lanes.** `ci.yml` calls `ci-lane.yml` with `ubuntu-24.04-arm`
   and with `ubuntu-24.04`; their jobs show up as `arm / tests`,
   `arm / links`, `x86_64 / tests` and `x86_64 / links`. The fan-in jobs
   `check` (AArch64) and `check-x86_64` pass only when both jobs of their
   lane succeeded: they run with `if: always()`, so a failed, cancelled
   or skipped lane fails them rather than counting as passed. GitHub
   documents `needs.<job>.result` (success, failure, cancelled or
   skipped) but not how the jobs of a called workflow add up to it, so
   the lane also passes out each job's own result as an output
   (`jobs.<job>.result`, documented for a reusable workflow's outputs),
   and a fan-in requires the lane's result and both outputs to be
   `success`; an output a lane never set is empty and fails it. They are
   plain jobs, not a matrix, so their names do not change when a lane's
   jobs do.
2. **The whole gate, two jobs per lane.** Each lane runs all of "ci"
   (fmt, clippy, the tests, the link tests on all three architectures,
   the census, the Mason step, the sweep, static-identical) in two jobs
   side by side on its runner, each with the whole set-up, through
   `scripts/env.sh` and with `MORTAR_REQUIRE_TOOLS=1`, as hod and kiln
   split theirs by `--step`: `tests` runs `cargo xtask ci --step fmt
   --step clippy --step test`, and `links` every other step of
   `xtask::ci` (`--step linktest --step census --step mason --step sweep
   --step static-identical`), with the link-test results (rule 5).
   `xtask/tests/hosted_ci.rs` checks that each step of `xtask::ci` is
   run by exactly one of the two, so a new step must be placed. The gate
   was one job until it grew past half an hour: run 44308473975 took 37
   minutes on the AArch64 runner and 41.5 on the x86-64 one, and run
   44300379924's x86-64 lane was cancelled by its limit during the
   sweep. Its steps on x86-64: fmt and clippy 1 minute, `test` 21 (about
   60 test binaries, none over 95 seconds), `linktest` 15, census 1,
   mason under 1, sweep about 4, static-identical about 3, and the set-up
   about 2 per job. The cut is the one that halves it: `test` and the
   cargo checks are one half, the steps that link and run programs the
   other, about 24 and 26 minutes with their set-up; each job's limit is
   40 minutes, half as much again. A lane takes as long as its longer
   job.
3. **The reference environment.** Each job runs in
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
5. **Results.** After the `links` job's steps, whether they passed or
   not (unless the run was cancelled), `scripts/ci-summary.py` writes the link tests' table
   of outcomes per linker and architecture, and every record whose
   outcome fails a run, to the job summary, from
   `$CARGO_TARGET_DIR/linktest/results.tsv` ("Results and exit status";
   removed before the gate, so a restored cache cannot show an older
   run's), and the file is uploaded as the artifact
   `linktest-results-<cache key>` (kept 14 days). No results file means
   the job stopped before its link-test step. The `tests` job has
   neither.
6. **Triggers.** `ci.yml` runs on a `push` to `main`, on every
   `pull_request` and on `merge_group` (the merge queue), always the plain
   gate; and by hand (`workflow_dispatch`: Actions, "ci", "Run workflow",
   on any branch), with the input `no_fail_fast` for a diagnosis run
   (`cargo xtask ci --no-fail-fast`, "ci"). The repository and its Actions
   logs are public: nothing the gate prints may name the machine it was
   developed on (paths, user names).
7. **Caches.** `Swatinem/rust-cache`, one `shared-key` per lane and
   job: `ci-arm-tests`, `ci-arm-links`, `ci-x86_64-tests`,
   `ci-x86_64-links` (rust-cache adds the Rust version, the hashes of
   `Cargo.lock` and the manifests, and the job's environment to the key):
   `~/.cargo` and the builds of the dependencies, not of the workspace
   crates (a fresh checkout gives every source a new mtime, so cargo
   rebuilds them anyway). Each job saves its own, also when it failed.
   Not one key for both: an Actions cache entry is never overwritten,
   so of two jobs saving one key only the first to finish writes it; and
   with one saver the entry holds that job's dependency builds (`tests`:
   clippy's check builds and `cargo test --workspace`'s dev builds;
   `links`: xtask's dev build), which serve the other job only as far as
   they happen to include its own. A key each restores exactly what that
   job built, for one more entry of some tens of MB per lane. With one third-party dependency tree (`signal-hook`, for
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
11. **Time and cost.** A run is six jobs: a `tests` and a `links` job
    per lane, and two fan-in jobs of a few seconds. The gate in one job
    took about 16 minutes on `ubuntu-24.04-arm` and about 19 on
    `ubuntu-24.04` when it was first measured
    (`spec/landed/harness/WP-015.md`), and 37 and 41.5 in run
    44308473975 (rule 2), of which the set-up (packages, Rust, cache) is
    2 to 3 minutes; split, each job is expected to take about half of
    that, plus its set-up. Standard runners are free for a public
    repository.

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
| `kind` | all | `sources`, `synthetic`, `cmake`, `cargo`, `ninja`, `planned` |
| `about` | all | one sentence for the report |
| `sources` | sources | files of `bench/workloads/src/` |
| `shape`, `params` | synthetic | the generator's shape and its two parameters ("Generated workloads") |
| `group` | synthetic | `yes`: the link puts every input after `main.o` in one `-Wl,--start-group` … `-Wl,--end-group`; default `no` |
| `url`, `sha256`, `root` | cmake | the source archive, its SHA-256, the directory in it with the top `CMakeLists.txt` |
| `options`, `prebuild`, `target` | cmake | cmake `-D` options; make targets built first without capture; the make target whose link is the workload |
| `rev`, `package`, `bin`, `rustc_args` | cargo | a commit of this repository; `cargo rustc -p <package> --bin <bin> -- <rustc_args>` |
| `script`, `sha256`, `root`, `tree`, `target` | ninja | the script of this checkout that makes the build tree, and its arguments after `<work>`; the SHA-256 of the source archive it unpacks and the directory in it (`$S`); the directory under `<work>/workloads` that holds the tree and the captured inputs, shared by the workloads of one tree; the Ninja output whose link is the workload ("Large workloads") |
| `driver`, `cflags`, `ldflags` | sources, synthetic, cmake (no `ldflags`), ninja (`driver` only, for the key) | the compiler driver on `PATH`, compiler flags, driver flags of the link |
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
| `synthetic-line-1000`, `synthetic-line-3000` | on-request | 1000 / 3000 static archives of 25 members (the `line` shape), on one line in dependency order, `-static`: what the count of archives alone costs (WP-081) |
| `synthetic-line-1000-group`, `synthetic-line-3000-group` | on-request | the same with the archives in one `--start-group` … `--end-group` (`group = yes`) |
| `cxx-large-g` | on-request | clang 19.1.7, `RelWithDebInfo` (`-O2 -g`), AArch64 target only, linked `-static`: 88 archives, 2.4 GB of input, some 1.1 GB of output ("Large workloads"; record `spec/landed/harness/WP-081.md`) |
| `cxx-large-g-opt` | on-request | `opt` of the same tree: 52 of the same archives, some 530 MB of output |
| `cxx-large-split` | on-request | `cxx-large-g` compiled with `-gsplit-dwarf` (`LLVM_USE_SPLIT_DWARF`): the `.dwo` files stay beside the objects and are not linked |
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
without arguments (their `main` exits 0) except `synthetic-objects*`
and `synthetic-line-*`, which are not run: each of their functions (each
needed member's first function) calls two further on, so a run makes
exponentially many calls.

### Planned workloads

`kind = planned` sections are specified and not built: `--list` and the
report name them with what they need. Today: `libcxx-test` (one libc++
test program as the L8 lane links it; needs kiln and hod and the libc++
build), `cxx-large` (`cxx-large-g`'s program without debug information:
needs a third build of the tree),
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
`rustc --version`; the bytes of the `ninja` kind's script and `ninja
--version`. A `ninja` workload is never rebuilt by the bench ("Large
workloads"). A workload is ready when its stamp's key is the current
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
  unpacked with `tar -xf … --no-same-owner` (which reads gzip, xz and
  bzip2 alike; the code is shared with "realbuild"). **Members `tar`
  is not given**: the archive is listed first (`tar --numeric-owner
  --quoting-style=literal -tvf`), and two classes of members are
  excluded by name (`--anchored --no-wildcards --exclude=<member>`).
  (1) A symbolic link whose target is absolute or has a `..` component:
  GNU tar writes a mode-000 placeholder file in its place and swaps the
  link in when the archive is done; on a shared mount a file without
  owner read permission can be created but never examined, changed or
  removed again, as root too, so the swap fails and the placeholder
  stays and breaks whatever walks the tree (LLVM's archive has five such
  links). The bench makes these links itself after `tar` has run, each
  only when its name is relative with plain components, every directory
  above it is a directory of the unpacked tree and not a link, and
  nothing is at its place; otherwise the log says `not unpacked (a link
  outside what was unpacked): <member>`. (2) A regular file or a hard
  link whose mode has no owner read permission, a directory without
  owner read or search permission (with everything below it), a device
  and a FIFO: not unpacked, each with a line `not unpacked (unreadable
  by its owner, or not a file): <member>` in the log. A listing line
  that is not understood, or more than 4096 such members, is an error
  and nothing is unpacked. A script that unpacks an archive itself
  ("Large workloads") excludes the links of class (1) the same way.
  Then `cmake -S
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

### Large workloads

A `ninja` workload (WP-081) is a link of a build too long for a bench
run: LLVM/clang takes hours to compile on the shared host. Its tree is
made ONCE, outside the bench, by the section's `script`; the bench
captures the link of `target` from the tree and keeps what that link
reads, not the tree.

- **The tree**: `<script> <work> <args>` makes `<work>/workloads/<tree>/build`
  (a Ninja tree; its archives are thin while it is built, half the disk).
  It fetches the pinned archive into `<work>/downloads/`, checks its
  SHA-256 and unpacks it once into `<work>/src/<first 16 digits>/`, as the
  `cmake` kind does. It compiles every input of the large links and links
  none of them. Ninja 1.12 is not a jobserver client: the script takes a
  fixed `-j` (at most 6 on the shared host, 4 while other packages wait for
  the load) and runs under `nice -n 10`.
- **The capture** (`--prepare --workload <name>`, or any run naming the
  workload, when `<work>/workloads/<name>/response.txt` does not exist):
  `ninja -C <build> -t commands -s <target>` gives the link's command; it
  runs with `/bin/sh -c` in the tree with the recording `ld` of
  "Capture" (`COMPILER_PATH`; the wrapper exits 0 and writes nothing), and
  the one record whose output matches `capture` is the link. The link
  must have run in the tree. Each argument that names a file of the tree
  (relative to it, or absolute under it; also after `-L` and after
  `--<option>=`) is copied to `<work>/workloads/<tree>/inputs/` under the
  same relative path and named by it; a thin archive is written as the
  regular archive of the same members in the same order (`ar Dqc`, then
  `ranlib -D`: LLVM's own archive rule); a directory of the tree after
  `-L`, or the directory of a file a `--<option>=` names in the tree (the
  `--dependency-file` CMake passes), is made there. Arguments outside the
  tree stay as they are. `response.txt` gets the relocated arguments, and
  the link runs in `inputs/` (the stamp's `cwd`). A file already in
  `inputs/` (by the same size, or a regular archive already written) is
  not copied again: the workloads of one tree share their inputs.
- **The census** is "Census"'s, with `$W` standing for the tree's
  directory.
- **Never rebuilt.** A prepared `ninja` workload whose key changed, or
  whose files changed, is counted again; it is never deleted or rebuilt by
  the bench. Without `response.txt` and without a tree the preparation is
  an error naming the script to run. To rebuild: delete
  `<work>/workloads/<tree>` and the workloads' directories, run the script,
  prepare.
- **After the capture** the tree is deleted by hand
  (`rm -rf <work>/workloads/<tree>/build`, a literal path); the cache keeps
  `inputs/` and each workload's `response.txt` and `stamp`.

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
  map names; the erratum option is the map's line, `spec/plumb.md`
  §3.2a). A child, so that what
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
| `line (a, m)` | `a` archives of `m` members of 64 functions and a table; seven members in eight are needed: function 0 of each calls the same member of the next archive and of one archive further on, never backwards, so one pass over the line in order loads them all | the count of archives: opening, symbol-table scans, the order of extraction |
| `library (m, k)` | one archive of `m` members of eight functions that call nothing; `main` calls every `k`-th member, the first included | the search of a big archive of which little is needed |

Every shape has a `main` that calls into every unit (for `archives`, into
the needed members of the first; for `line`, into the needed members of
the first archive; for `library`, into its needed
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
`xtask/src/bench/tree.rs`: `arguments_are_relocated`.
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

## realbuild

`cargo xtask realbuild` is layer L9's open-source lane (`docs/VALIDATION.md`;
WP-100): real projects built by their own build systems (CMake, autotools
with libtool, Kbuild, hand-written Makefiles, rustc) with mortar as the
linker, their own tests run, and every link line counted: linked
correctly, refused by name, or a defect. It is an **off-gate lane**: it
fetches from the network and takes hours, so `cargo xtask ci` runs only
its unit tests, and the hosted lanes never run it; its evidence is the
checked-in baseline and the report it generates.

```
cargo xtask realbuild [--project <p>]... [--row gcc|clang]... [-j <n>] [--mortar <path>]
                      [--work <dir>] [--keep] [--keep-going] [--update-baseline]
cargo xtask realbuild --report
cargo xtask realbuild verify <output> <map> <argv file>
```

### The projects

`tests/realbuild/manifest`, in the bench manifest's format ("Workloads":
`[name]`, then `key = value`; the parser is shared): `version`, `url`,
`sha256`, `root` (the archive's directory holding the sources),
`licence`, `why` (one sentence: what the project covers), `rows` (`gcc`,
`clang`), optionally `extra = <url> <sha256> <root>` (a second archive:
a test suite shipped apart, a dependency's sources) and `tests` (which
tests, when not all, and why). An unknown key, a missing one, a digest
that is not 64 lower-case hexadecimal digits and an unknown row are
errors naming the line. Archives are fetched and unpacked by the bench's
code ("Preparation", `fetch_archive`: `curl`, `sha256sum`, `tar -xf`)
into `<work>/downloads` and `<work>/src/<first 16 digits>`; nothing is
committed and no project is patched (a configuration choice, such as
turning one applet off, is not a patch; a project that does not build
unpatched is replaced and named in the record).

A project's build is `tests/realbuild/projects/<name>.sh`, which
defines `rb_build` and `rb_test` and is run by `tests/realbuild/run.sh
<script> build|test` with `tests/realbuild/lib.sh` sourced. It runs the
project's own build system, and passes no linker: the wrapper is found as
`ld`. Its environment: `RB_SRC` (a fresh copy of the sources), `RB_EXTRA`
(of the extra archive), `RB_BUILD` (an empty directory, the current one),
`RB_ROW`, `CC`, `CXX`, `FC`, `F77` (GCC's `gcc`, `g++`, `gfortran`; for
`clang`, `clang`, `clang++` (22, else 19) and `gfortran`), `CFLAGS`,
`CXXFLAGS`, `FFLAGS`, `FCFLAGS` `-g -O2`, `RB_JOBS` and `RB_TEST_JOBS`
(the lane's `-j`, 1 to 6, default 3: rustc's code generation units and
ctest's `-j`), `RB_RESULTS` (the tests'
results, `<name> TAB PASS|FAIL|SKIP` per line, written by the library's
`rb_run`, `rb_ctest` (ctest's JUnit file) and `rb_automake` (automake's
`.trs` files); the word is the line's last field, so a name may hold a
tab, and a name's control characters are read as spaces), `LC_ALL=C`, `TMPDIR` inside the build's directory, and
`MAKEFLAGS=-j<n>` in place of the machine's pool: make is the
jobserver of its own `n` slots for all it starts, and no script passes
`-j` to it; CMake uses the Unix Makefiles generator. A fixed count rather
than the pool because the shared host caps this lane at 3 jobs under
`nice -n 10` (supervisor, WP-100): the pool's free tokens can be many
more. Each step runs under `nice -n 10`. The script's
`rb_configure <command>` marks the links its command makes (autoconf's,
CMake's and hand-written checks) as the `configure` phase. Each step is a
child of `process::run` (one token; 2 hours and 8 GiB of address space),
its output appended to the build's `log`. Before each build the lane
waits until the 1-minute load average is under 10.

### Rows and modes

A row is a project with one compiler family: `gcc` (the control linker is
GNU ld, `/usr/bin/ld.bfd`) or `clang` (the control is lld, `ld.lld`).
Fortran is gfortran's in both. Each row is built twice, each time from a
fresh copy in `<work>/build/<project>-<row>-<mode>`: in mode `control`
the wrapper hands every line to the control; in mode `mortar` it links
every line with mortar. The directory is removed when its results are
recorded (`--keep` keeps it), and the unpacked sources when the project's
rows are done: one build tree on disk at a time.

### The wrapper

`tests/realbuild/ld` is installed as `ld`, `ld.bfd`, `ld.lld`, `ld.gold`
and `<triple>-ld` in `<build>/wrap`, which is first on `COMPILER_PATH`
(where gcc, collect2 and clang look for the linker first) and on `PATH`
(where a build that runs `ld` itself, such as Kbuild's `ld -r`, finds
it). A run that names no output (`-v`, `--version`, `--help`,
`--verbose`: a build asking which linker it has) is a query: the control
answers it, and it is only counted. Every other run is a record, a
directory `<build>/records/link.XXXXXXXX` with the working directory and
the arguments (each ended by a NUL), copies of the response files it
names, the phase, the start time, and:

- mode `control`: the control links the line; its status and standard
  error.
- mode `mortar`: the control links the line to a scratch output, which is
  removed (its status is the oracle: whether the line links); then mortar
  links the line with `--mortar-map=<record>/map`. When mortar's status
  is 0, `xtask realbuild verify <output> <map> <argv>` judges the output
  with the line, and the build goes on with mortar's output. Otherwise the
  control links the line for the build (the fallback, recorded), so one
  refusal does not hide the rest of the project; the build sees the
  control's diagnostics and status.
  An output that exists and is not a regular file (`-o /dev/null`,
  Kbuild's and configure's probes of whether a line links) is not one the
  verifier can read: mortar links such a line to `<record>/mortar.out`,
  which is judged and then removed, and the build's output is not
  touched.

`realbuild verify` expands the line's `@file` words (GNU ld's quoting,
the bench's `parse_response`), runs the verifier (`plumb::verify_link_line`,
the inputs read from the working directory, stage 3 with the line, and
`--fix-cortex-a53-843419` as the line gives it: the last of it and
`--no-fix-cortex-a53-843419`, so that erratum 843419 sites are judged
whenever the option was given, not only when the output has a fix) and
prints `plumb: clean` (status 0), or `plumb: <n> findings: <rule> x<k>,
...` and the report's first 20 lines (status 1), or `plumb: error: <what>` (status
2: the verifier could not judge).

### Outcomes

A line's id is its output's path relative to the build's own directory
(`build/` out of tree, `src/` for the sources' copy, `extra/` for the
second archive's), with each component a tool named at random
written `*` (`cc` and six letters or digits, `rustc` and six, `tmp.` and
ten, `TryCompile-` and six, `cmTC_` and five), `configure/` before it for
the configure phase, and `#<n>` for its n-th repetition in the order the
links started. A **probe** is a line of the configure phase, or one whose
id names `conftest`, `CMakeScratch`, `CMakeTmp`, `TryCompile`, `cmTC_`,
or `tmp/`. The output's kind is `shared` (`-shared`), `relocatable`
(`-r`, `-i`, `-Ur`), `static-pie` (`-static` or `--no-dynamic-linker`
with `-pie`), `static`, `pie` or `exec` (the last of `-pie`, `-no-pie`
wins). A line of mode `mortar` is:

| outcome | when | a defect |
|---|---|---|
| `linked` | mortar's status 0, the control's 0, the verifier clean | no |
| `refused <noun>: <name>; ...` | mortar's status is not 0 and its standard error names refusals (`<prog>: error: [<place>: ]unsupported <noun>: <name>`, `spec/diag.md` §4; `feature: linking`, the consequence of a refused option, is not counted); the control links the line | no |
| `both-fail` | neither links it (mostly probes of what is not there) | no |
| `finding <report>` | mortar linked, the verifier found something | yes |
| `verifier-error <what>` | the verifier could not judge | yes |
| `links-control-cannot` | mortar links a line the control does not | yes |
| `crash <status>: <line>` | mortar ended by a signal, with a status of 128 or more, or a panic | yes |
| `error <status>: <line>` | mortar failed with no refusal, the control links it | yes |
| `incomplete` | the record lacks a status (the wrapper was cut short) | yes |

A refused name with a path after `=` is counted without it
(`--version-script=<file>`). A **lost test** passes with the control and
not with mortar. The lane prints each row's counts, every defect, and
`STOP:` when a row has a lost test and no defect line: a silent mislink
that both mortar and its verifier missed. It then stops the survey
(`--keep-going` goes on), as WP-100's stop condition says.

### Results

Each mode leaves `<work>/results/<project>.<row>.<mode>/` (`log`, `tests`,
and `lines`: every line's id, kind, outcome, directory and words, with
mortar's first diagnostics and the verifier's report where it did not
link cleanly), and each row `<work>/results/<project>.<row>.tsv`. The
checked-in files, which `--update-baseline` rewrites for the rows the run
has and keeps for the others:

- `tests/realbuild/baseline.tsv`: `# mortar-realbuild-baseline 1`, then
  `project TAB row TAB line TAB kind TAB outcome`, the mortar mode's lines;
- `tests/realbuild/tests.tsv`: `project TAB row TAB test TAB control TAB
  mortar`;
- `tests/realbuild/runs.tsv`: per mode, the wall time, the build tree's
  peak size (`du -sk` after each step), the lines and whether it built,
  and mortar's commit.

A run compares its rows with the baseline's for the same project and row:
**regressed** (linked before and not now, or a defect now that was not
one), **fixed** (linked now and not before), **changed** (another
outcome, such as another refusal), **new** and **lost** lines; exit status
1 when one regressed or the run stopped, 3 for an error of the harness
(a fetch, a missing tool), 2 for a usage error, else 0.

### The ranking

`--report` writes `docs/status/realbuild.md` from the manifest and the
three files, measuring nothing: the projects (version, licence, why,
URL and digest), the outcome table per project and row (lines, linked,
refused, neither, defects, probes, tests passed with each linker, lost
tests), the lines by kind of output, **the ranking** (every refused
name with the lines it is refused on, the projects, the **unlock** count:
the lines on which it is the only refusal, an upper bound because mortar
stops at the first phase that refuses), the lines by their whole set of
refusals, the defects, the tests that differ, and the runs.

### One project alone

`cargo xtask realbuild --project zlib --row gcc` (with `--keep` to keep
the build trees for a look, `--mortar <binary>` for a mortar built
elsewhere). The work directory is `real-builds` beside the checkout's
target directory (`--work` to change it); without `--keep` its peak size
is the largest single build tree (each mode's tree is removed before the
next starts) plus the downloads, not the sum.

### Tests

`xtask/src/realbuild.rs`: `the_erratum_option_is_read_from_the_line`, `a_test_name_with_a_tab_is_one_field`, `arguments_parse`,
`the_manifest_parses_and_names_its_errors`,
`the_checked_in_manifest_parses_and_each_project_has_a_script`.
`xtask/src/realbuild/outcome.rs`: `refusals_are_read_by_noun`,
`kinds_follow_the_line`, `outcomes_follow_mortar_the_control_and_the_verifier`,
`ids_are_relative_and_random_names_starred`, `response_files_are_expanded`,
`the_baseline_round_trips_and_compares`. `xtask/src/realbuild/report.rs`:
`the_ranking_counts_lines_projects_and_unlocks`,
`tests_and_runs_round_trip_through_the_report`. The builds themselves are
exercised by running the lane; its evidence is the baseline and
`spec/landed/harness/WP-100.md`.
