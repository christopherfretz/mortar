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

Every layer runs on AArch64 natively and on x86-64 and 32-bit x86 under
`qemu-x86_64` and `qemu-i386` (the host is AArch64); the hosted lanes run
each architecture natively (32-bit x86 on the x86-64 runner). The layers
that need kiln and hod (L7, L8) cover the two 64-bit architectures.

### Where mortar writes an output, and who judges it

L3's promise is that no harness produces a mortar output the verifier
has not judged. The audit of WP-035 (`spec/landed/harness/WP-035.md`);
a row that changes must change here. Refusal tests (deliberately bad
inputs, `exit: fail`) write no output and are not holes.

| where | mortar's outputs | the verifier |
|---|---|---|
| `cargo xtask linktest` (L2/L3) | every link step of every test, per architecture | every step that is to succeed writes `--mortar-map` and is judged (`spec/harness.md` "L3"); outputs outside the verifier's scope are counted by reason in the summary (`machine`: i386; `pie-or-shared`, `dynamic`, `relocatable`). `--no-plumb` turns it off by hand; no gate passes it. |
| `cargo xtask bench` | one untimed link per workload, then the timed ones | judged before timing (`spec/harness.md` "Verification"); the timed bytes must hash to the verified bytes; `--no-verify` by hand only, never for a baseline. A verified output whose workload has a `run` line is then run once, as is GNU ld's output of the same link, and the two programs' stdout and exit status must agree (`spec/harness.md` "Running the outputs"); a difference fails the bench like a finding. Since WP-036 mortar links 12 of the 23 workloads, every one of them clean and, where run, the same as GNU ld's; the 11 others are refused by name (10 are `-pie` or dynamic; `rust-self-static` because rustc passes a trailing `-Bdynamic`). |
| `crates/mortar/tests/pipeline.rs` | every link through `mortar()` | `mortar()` judges every output it writes; the determinism runs judge the first output and compare the 27 others byte for byte (`mortar_unverified`). |
| `crates/mortar/tests/x64.rs` | `holds`, the gc size pairs and the determinism runs | `holds` and the size pairs judge; the determinism test judges the first output, the others must equal it. |
| `crates/mortar/tests/gc.rs` | every `--gc-sections` link through `mortar()` | `mortar()` judges every output it writes; the determinism runs judge the first and compare the others byte for byte (`mortar_unverified`). |
| `crates/mortar/tests/image.rs` | the library's `image::write` | every written image is judged (`plumb`), `i40_compares_with_gnu_ld` included; the I-65 tests write none. |
| `crates/mortar/tests/deferred.rs` | the tombstone test's writes | the first output of each case is judged (with the map rendered from the link); the others must equal it. The other tests write no output. |
| `crates/mortar/tests/options.rs` | the option audit's links (`Cx::mortar`) | every output is judged; the one exception is `--build-id=0xHEX` (a false B1 until mortar's map states the style `given`, `spec/plumb.md` §6.7), checked to be exactly B1 so that it fails when the map catches up. |
| `crates/mortar/tests/sysroot.rs` | the sysroot links | every output is judged. |
| `crates/mortar/tests/debuggers.rs` | the debugger pairs | `link` judges every output. |
| `crates/mortar/tests/cli.rs`, `refusals.rs` | none (failed links, `--version`) | — |
| `crates/mortar/tests/front_diff.rs` | none (the front's layout, compared; no image) | — |
| `cargo xtask mason programs` | every program × variant | judged (`--mortar-map`; the lane wrote `-Map`, a GNU text map since WP-027e, which the verifier could not read). |
| `cargo xtask mason libcxx` | every libc++ test link | only with `--plumb` (a cost choice: thousands of links); the counts are in the report. The shim now writes `--mortar-map`. |
| `cargo xtask mason verify` | — | the shim's verifier. |
| byte-identity checks of the performance packages (WP-032*) | ad hoc scripts, not in the repository | not judged. Their outputs are the bench's workloads or static twins of them: a performance package states the verifier's verdict on each output it hashes, or measures through `cargo xtask bench`. |

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

A change fails the bench gate when mortar's time grows by more than 35% or
its memory by more than 4% against the recorded per-machine baseline in
`bench/baselines`. The margins were 5% and 2%; WP-036 measured the
shared host's run-to-run noise (up to 28% on the links the gate judges
steady, 2.6% of memory) and set them above it (`spec/harness.md`
"Noise"). Each mortar link is also compared against the fastest and the
smallest other linker in `docs/status/perf.md`. The release gate also compares against mold, lld and
GNU ld on the same machine and records the ratios in
`docs/status/bench.md`. Measurements are taken with the jobserver pool
idle, or marked as noisy.

## Gates

- **`cargo xtask ci --quick`**: what an implementer runs before handing
  back: fmt, clippy with warnings denied, unit tests, the link tests of the
  touched areas.
- **`cargo xtask ci`**: what runs before a merge: everything in the "quick"
  and "full" rows, on every architecture.
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
