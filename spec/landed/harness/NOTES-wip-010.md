# WP-010 work in progress: design notes for the next implementer

Handover notes, not a spec. Delete this file when `spec/harness.md` "Link
tests" and `spec/landed/harness/WP-010.md` are written.

## State

Written, NEVER COMPILED (no cargo command was run on this branch yet):

- `xtask/src/linktest/pattern.rs`: the pattern matcher and its tests.
- `xtask/src/linktest/target.rs`: `Arch`, `Linker`, `Family`, `Feature`,
  `ArchTools::find`, `CommonTools::find`, tests. (`no_fortran` in its
  tests is written clumsily; simplify.)
- `xtask/src/linktest/format.rs`: the parser (`parse(text) -> Result<Test,
  Vec<Error>>`). Complete but has NO unit tests yet. It was patched by a
  script after being written: read it top to bottom before compiling.
- `xtask/src/linktest/outcome.rs`: `Outcome`, `Raw`, `marked`,
  `self_check`, table-driven tests.
- `xtask/src/linktest/report.rs`: `Record`, `Report` (summary, table,
  TSV, exit status), tests.

Not started: `xtask/src/linktest.rs` (module root: `Config`, argument
parsing, test collection, building mortar, the worker pool),
`linktest/engine.rs` (running one test on one architecture), `pub mod
linktest` in `xtask/src/lib.rs`, the `linktest` command in `main.rs`, the
`ci` step, the spec text, the seed corpus (`tests/link/` is empty), the
end-to-end test, `spec/landed/harness/WP-010.md`, `spec/README.md`.

## The format (as `format.rs` implements it)

File: `tests/link/<area>/<name>.test`. Three parts, in this order:

1. Directives, `#@ <key>: <value>`, before the first step:
   `spec: <file>.md <section>` (one or more, required), `arch:` (once;
   default all three), `requires:` (once; words `g++ gfortran clang
   yaml2obj llvm-readelf`; must name EXACTLY the tools the steps use
   beyond `gcc`, binutils and the linkers: both a missing and an unused
   word are errors), `xfail(<quals>): <reason>` and `skip(<quals>):
   <reason>` (at least one linker in the qualifiers), `timeout: <s>`
   (1..=3600) and `mem: <MiB>` (16..=65536): limits of each link and run
   step.
2. Steps: a line that starts in column 1. `cc`, `cxx`, `fc`, `as` (the C
   driver with `-c`, so `.S` is preprocessed), `yaml2obj`, `ar` build
   inputs and must all precede the first `link`/`run`/`check`.
   `link <ld|gcc|g++|gfortran|clang|clang++> <args>`, `run <program>
   [args]`, `check <readelf|nm|objdump|llvm-readelf|text> <args>` run per
   linker. Indented lines under a step are its expectations:
   link: `exit: 0|fail`, `diag: <pattern>`, `no-diag: <pattern>` (over
   the lines of the link's stderr); run: `exit: N|signal N`, `stdout:
   <line>` (exact, repeatable; default: prints nothing), `output:
   reference` (the references must agree, mortar must print the same);
   check: `has: <pattern>`, `not: <pattern>` (at least one).
   Other `#` lines are comments.
3. Embedded files: `#--- <relative name>` starts one; it runs verbatim to
   the next `#---` line or the end. (LLVM's `split-file` marker; `---`
   alone would collide with `yaml2obj` documents.)

Qualifiers, in parentheses after a keyword, are words of three kinds:
compiler family (`gcc` default, `clang`) on compile steps only; linker
(`bfd lld mold mortar`) and architecture on link/run/check steps, on
expectations and on `xfail`/`skip`. Words of one kind are alternatives,
kinds are conjoined. `cc(clang,i386) ...`, `has(bfd,lld): ...`.

Words are split on blanks; `"..."` keeps blanks (`\"`, `\\`).
Substitutions `%{arch} %{triple} %{emulation} %{dynamic-linker}
%{sysroot} %{libdir} %{elf-class} %{machine} %{linker}`, `%%` is `%`;
in step arguments and expectation values, not in embedded files;
`%{linker}` not in build steps; values are escaped inside patterns.

Patterns (per line, unanchored substring): a run of blanks matches one or
more blanks; `*` any text; `^` first / `$` last anchor; `\` before `\ * ^
$` only.

Parse-time rules worth knowing: an `exit: fail` link needs a `diag` that
applies to mortar on every architecture; at most one marker per (linker,
arch); every architecture keeps at least one reference linker without a
marker; a scope may only name architectures of the test's `arch`.

## Runner decisions (not yet in code)

- Matrix unit = (test, architecture); one worker thread runs it start to
  finish; `-j` threads (default min(cores, 8), max 16), every child via
  `xtask::process::run` (jobserver token + limits).
- Per unit: write embedded files to `<work>/<NNNN>-<name>/<arch>/inputs`,
  run build steps there once; for each linker COPY `inputs` to
  `<arch>/<linker>/` (a copy, not symlinks: a linker that damages its
  inputs must not reach the others; thin archives keep working because
  members are copied beside the archive) and run link/run/check there.
  References first; mortar only if the self-check holds.
- Driver links: gcc family gets `-B<dir>` where `<dir>/ld` is a wrapper
  script (gcc cannot be told a linker path; `-fuse-ld=` takes fixed
  names); clang gets `--target=<triple> --ld-path=<dir>/ld` (names the
  linker and nothing else; `-B` would also move clang's search for the
  GCC installation). The wrapper is not a symlink: it runs the linker,
  appends its exit status to `$MORTAR_LINKTEST_STATUS`, exits with it.
  No status file after a driver link = the driver never ran the linker =
  ERROR. Verified by hand: `-B` and `--ld-path` work for bfd/lld/mold on
  all three architectures, static C and C++.
- Expected-failure link: exit non-zero, never a signal (driver form:
  wrapper status above 127 counts as a signal); mortar must leave no
  output file (`-o X`, `-oX`, `--output[=]X`, else `a.out`); a limit is
  KILLED, not a met expectation.
- Limits: build/check tools 120 s, 4 GiB; link 60 s, 4 GiB; run 10 s,
  1 GiB; emulated run 6x the time. TRAP (measured): `qemu-i386` reserves
  the guest's whole 4 GiB, which counts against `ulimit -v`: under 4 GiB
  it dies with "Unable to find a guest_base"; 5 GiB works. An emulated
  i386 run needs its limit plus 4 GiB. `qemu-x86_64` runs in 1 GiB.
- Before a native run, check the program exists and starts with
  `\x7fELF` (FAIL otherwise): `process::run`'s `/bin/sh` wrapper (dash)
  re-runs a file the kernel refuses (ENOEXEC) as a shell script.
- Cross tools are `<triple>-<tool>` for gcc, g++, gfortran, ld.bfd, ar,
  nm, objdump, readelf (native `nm`/`ar` do not know x86 objects: `ar`
  would silently write no symbol index). `i686-linux-gnu-gfortran` is not
  installed: the Fortran seed is `arch: aarch64 x86_64`.
- Records: one per (test, arch, linker); a linker-less record per (test,
  arch) for SKIP (requirement missing), ERROR/KILLED (inputs not built),
  ORACLE; one record without arch for a file that does not parse. On
  ORACLE the references keep their own records (FAIL, KILLED, ...) and
  mortar has none. `--linker` without any reference: mortar is judged
  unproven, with a run-level note (`Verdict::Unchecked`); references in
  the run but none that passes the test (`Verdict::Unproven`): mortar
  SKIP. An `output: reference` test cannot judge mortar without a
  reference: SKIP.
- A reference whose output legitimately differs under `output:
  reference` is left out with `skip(...)`, not `xfail(...)` (it cannot
  fail a step, so `xfail` would be XPASS).
- Exit status: 1 = FAIL, XPASS or KILLED; 3 = only ERROR/ORACLE or a
  missing required tool; 2 = usage; 0 otherwise (hod's convention).
- Missing tools: an architecture whose baseline tools are missing is left
  out with a note (no records); a missing linker leaves the matrix with a
  note; under `MORTAR_REQUIRE_TOOLS=1` each is an `error:` line and exit
  status 3, and the run goes on with what there is.
- Extra options planned beyond the brief: `--mortar <binary>` (the
  end-to-end test's fake linker; default builds `cargo build --package
  mortar --bin mortar` through `process::run_step`), `-v`.
- L3 hook: one stub called after every successful link, marked `L3
  (WP-028)`.
- `ci`: `linktest` is a fourth step run in-process after `test`;
  `xtask/tests/jobserver.rs`
  `ci_runs_its_cargo_steps_in_order_and_stops_at_a_failure` runs `ci
  --quick` with a stand-in cargo and expects exactly three steps: give
  its "pass" case explicit `--step fmt --step clippy --step test`, and
  update `ci.rs` tests `steps_are_selected_by_name` and
  `the_steps_are_fmt_clippy_and_test`.

## Reference-linker differences seen so far

None recorded yet: only static hello (C) and a thrown-and-caught C++
exception were linked by hand, and bfd, lld and mold agreed on all three
architectures. What bfd/lld/mold leave behind after a failed link was
not measured.
