# Validation

A linker's output is trusted without inspection, so what "tested" means is
the most important design in this repository. This document says how
correctness, speed and memory are established, which checks gate a change
and which gate a release.

Status: the plan. `cargo xtask ci` runs fmt, clippy and the unit tests
today; each layer below names the milestone that brings it
(`docs/ROADMAP.md`).

## Oracles

mortar is never its own judge where an external judge exists.

| oracle | what it judges |
|---|---|
| GNU ld 2.44, lld 19 and 22, mold 2.37 | Whether a link should succeed, what the linked program does, which symbols are exported, which libraries are needed. Three implementations: where they agree, that is the behavior; where they differ, the spec records the difference and the choice. |
| `mortar_plumb` | Every relocated field and the structural rules of an output (`docs/ARCHITECTURE.md`, "The verifier"). Itself judged by running it on the reference linkers' outputs. |
| `readelf`, `llvm-readobj`, `llvm-dwarfdump --verify`, `eu-elflint` | Structure as other tools read it. |
| The loader and the program | A linked program runs and prints what it prints when linked by a reference linker. |
| gdb 16, lldb 19 | The same debug session gives the same transcript on a mortar-linked and a reference-linked program. |
| Test suites of other projects | mold's and lld's ELF link tests, libc++'s tests, the test suites of the open-source projects mortar links. Fetched and pinned, never vendored. |

## Layers

| layer | what | gate | from |
|---|---|---|---|
| L0 | Unit tests of each crate, each naming its spec section. | quick | M0 |
| L1 | **Readers and writers.** ELF, archive and script parsers against `readelf`/`llvm-readobj`/`ar` on a corpus of real files; malformed-input tests (truncated, overlapping, absurd sizes): a diagnostic, never a panic. | quick | M0 |
| L2 | **Link tests** (`tests/link/`): one directory or file per case with a directive header: how to build the inputs (assembler, C, C++, Fortran, `yaml2obj`, kiln, hod), the link line, and what to check (exit status, diagnostics, `readelf` patterns, the program's output). The harness runs each case with every reference linker first (the self-check: a test that the references fail is a wrong test) and then with mortar. | quick: the touched directories; full: all | M0 |
| L3 | **The verifier on everything.** Every output the harness produces, by any linker, goes through `mortar_plumb`. A finding on mortar's output fails the test; a finding on a reference linker's output is a bug in the verifier or a spec question, and blocks too. | full | M1 |
| L4 | **Differential links** (`xtask linkdiff`): the same inputs through mortar and the references; compare behavior (run output, exit status), the dynamic interface (`DT_NEEDED`, exported and imported symbols with versions, `DT_*` flags), and the semantic content of symbol tables. Bytes are not compared: layout is each linker's own. | full | M1 |
| L5 | **Resolution fuzzing** (`xtask linksmith`): generated sets of objects, archives and shared objects with random definitions, references, bindings, visibilities, COMDAT groups, common symbols and command-line orders; mortar must choose the definitions the references choose, or fail when they fail. Malformed-input fuzzing of every reader. | nightly | M2 |
| L6 | **Debuggers.** Scripted gdb and lldb sessions (breakpoints, backtraces through C++ exceptions and signal frames, locals, types) on programs linked by mortar and by a reference; equal transcripts. `llvm-dwarfdump --verify` on every `-g` output. | full | M3 |
| L7 | **kiln's and hod's tests with mortar as the linker.** kiln's executing corpus and hod's `irdiff` link matrix with mortar added as a fourth linker. | pre-merge lane | M2 |
| L8 | **libc++.** The libc++ test suite (the tag kiln and hod pin) built by kiln and hod, linked by mortar: no test may pass with the control linker and not with mortar ("strict"), with and without `-g`. | release | M3 |
| L9 | **Other people's tests and programs.** mold's and lld's ELF test suites run against mortar with a recorded pass list; open-source projects built with GCC, Clang, gfortran and kiln, linked by mortar, running their own test suites; mortar and the Rust toolchain's test binaries linked by mortar. | release; tracked | M4 |
| L10 | **Incremental.** Random edit sequences over a project (change a function body, add or remove a symbol, grow data, reorder inputs, replace an archive member); after each step: the verifier accepts, the program behaves as a from-scratch link's does, and a full link given the incremental layout gives the same bytes. | full once M7 lands | M7 |
| L11 | **One-definition rule.** A corpus of violations (each kind the spec lists, across objects, archives and shared objects, at load time) that must be diagnosed, and of legal programs that must not be. | full once M6 lands | M6 |

Every layer runs on AArch64 natively and on x86-64 under `qemu-x86_64`
(the host is AArch64); the hosted lanes run each architecture natively.

## Determinism

The full gate links a set of cases with `--threads=1` and with many, and
twice with the same settings: the outputs are byte-identical.

## Speed and memory

`xtask bench` links a fixed set of workloads and records, per linker and
workload, the median wall-clock time of several runs and the peak resident
memory (the kernel's figure for the process, as GNU `time` reports it):

- small (a hello-world against libstdc++), where start-up dominates;
- one libc++ test (the unit of the libc++ lane, linked nine thousand times);
- medium and large C++ programs with and without debug information (a
  large open-source C++ program's objects, captured once and pinned);
- mortar itself, as rustc invokes the linker;
- generated extremes (a million symbols, a hundred thousand sections, one
  huge string table);
- from M7: the incremental cases (one function changed; one object changed;
  an archive replaced).

A change fails the bench gate when mortar's time grows by more than 5% or
its memory by more than 2% against the recorded per-machine baseline in
`bench/baselines`. The release gate also compares against mold, lld and
GNU ld on the same machine and records the ratios in
`docs/status/bench.md`. Measurements are taken with the jobserver pool
idle, or marked as noisy.

## Gates

- **`cargo xtask ci --quick`**: what an implementer runs before handing
  back: fmt, clippy with warnings denied, unit tests, the link tests of the
  touched areas.
- **`cargo xtask ci`**: what runs before a merge: everything in the "quick"
  and "full" rows, on both architectures.
- **Nightly / off-gate lanes**: fuzzing, external suites, open-source
  builds, bench history. A regression opens a backlog row.
- **Release gate**: all of the above green; libc++ strict with and without
  `-g`; the external-suite pass lists not shrunk; bench within thresholds
  and the ratios against the reference linkers recorded; the verifier run
  on the release-gate links with no finding.

## Resource limits

Every process the harness starts holds a token of the machine's jobserver
pool and runs under a time limit and an address-space limit; a process that
exceeds a limit is reported as killed, which no expected-failure marker
hides. See `spec/harness.md` (from WP-003).

## Test conventions

- A test names the spec section it checks.
- A gap is marked expected-fail with a reason (`xfail(mortar): ...`), never
  left failing and never deleted; an expected failure that passes fails the
  gate until the marker is removed.
- A test of a refusal checks the name mortar gives the refused construct.
- Inputs built by a compiler are built at test time by a pinned tool, or
  checked in as `yaml2obj` text; binary objects are not checked in.
