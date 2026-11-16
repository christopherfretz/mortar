# Roadmap

Tracks run in parallel inside a milestone once the interfaces they share
are fixed. Work packages are rows in `docs/BACKLOG.md`. The milestone
numbers are names, kept stable because the specs and the backlog cite them;
the order of work is the one below.

## Order of work (operator, 2026-11-15)

Correctness and the ability to link big, real codebases come first,
together with incremental linking; optimisation starts when that is
minimally mature. Incremental linking and diagnostics (the metadata, the
one-definition rule, what tuck needs) are what distinguishes mortar; parity
with mold is wanted but is not what makes mortar worth using.

1. **M2**, then **M3**: dynamic executables, then the libc++ suite with
   kiln and hod.
2. **M4** (the breadth big codebases need: shared-object output, linker
   scripts, the open-source lanes), **M7** (incremental linking) and **M6**
   (metadata and the one-definition rule) side by side. The designs of M7
   and M6 (`spec/incr.md`, `spec/meta.md`, the requests to kiln and hod)
   start during M2, because they shape what the other projects emit and
   what the linker must keep.
3. **M5**, the fastest: after the above is minimally mature.

Until M5, speed and memory are guarded, not pursued: the bench records
every release and a regression needs a reason, no package is cut for speed
alone, and no `unsafe` is proposed (file mapping included: measured in
November 2026 at about a third of the distance to mold on the static
bench link, it waits for M5). What does not wait is
design: nothing may be built that makes a fast or an incremental linker
impossible later (layout is data, the image a pure function of it,
deterministic output, the pipeline's parallel shape).

**The reference configuration** (operator, 2026-11-15): the codebase mortar
is aimed at links very large STATIC executables (nearly everything is
linked statically): typically hundreds of megabytes, rarely gigabytes; link
lines of hundreds to low thousands of static archives; debug information
always on, split DWARF (`-gsplit-dwarf`) acceptable; the output's file
system supports reflink. Designs, benches and the go/no-go of incremental
linking are sized for that: every step of a link or of an incremental
update that is proportional to output size or to the number of inputs is
named and costed at that scale, and split DWARF is a first-class
configuration of every lane (WP-080), with a large real static link in
the bench (WP-081).

The first long-term goal is **M3**: the full libc++ test suite, compiled by
kiln and hod and linked by mortar, passing wherever it passes with the
control linker.

## Tracks

| track | area | crates |
|---|---|---|
| T-H | Harness: CI, link tests, differential runs, external suites, bench, jobserver | `xtask`, `mortar_testutil` |
| T-ELF | File formats: ELF, archives | `mortar_elf` |
| T-CLI | Command line, linker scripts, version scripts, diagnostics | `mortar_cli`, `mortar_script`, `mortar_diag` |
| T-RES | The model, inputs, symbol resolution, liveness, merging | `mortar_core`, `mortar_input`, `mortar_resolve`, `mortar_live`, `mortar_merge` |
| T-A64 | AArch64: relocations, relaxations, PLT/GOT, TLS, thunks | `mortar_arch`, `mortar_a64` |
| T-X64 | x86-64: the same | `mortar_x64` |
| T-X86 | 32-bit x86: the same, on ELF32 with REL relocations | `mortar_x86` |
| T-OUT | Synthetic sections, layout, image, debug sections | `mortar_synth`, `mortar_layout`, `mortar_image` |
| T-V | The independent verifier | `mortar_plumb` |
| T-META | Mason metadata, one-definition rule | `mortar_meta`, `mortar_odr` |
| T-INC | Incremental linking | `mortar_incr` |
| T-PERF | Speed and memory | all |

T-V is staffed separately from T-A64, T-X64, T-X86 and T-OUT by rule: the verifier
is written from the specs, not from the linker.

## M0: foundations

The repository, the gate, the harness and the contracts that let the tracks
run in parallel.

- Workspace, lints, `scripts/env.sh`, the jobserver pool (done: WP-001).
- `spec/model.md`: the ids and the `Plan`/`Inputs`/`Resolution`/`Layout`
  tables. Design only, reviewed before T-RES and T-OUT start.
- `mortar_elf` readers with the L1 corpus; `mortar_diag`.
- The link-test format and runner with the reference-linker self-check, so
  that tests exist and are proven right before the code they test.
- `mortar_cli`: the option table from the link lines the drivers really
  produce (GCC, Clang, gfortran, rustc, kiln), each option with a status.
- The verifier's first stage, developed against the reference linkers'
  outputs alone.
- Bench harness and the first baselines of mold, lld and GNU ld.

**Done when:** `cargo xtask ci` runs link tests against the three reference
linkers, the verifier checks their outputs for the M1 relocation set, and
mortar refuses every link by name.

## M1: static executables (AArch64)

The whole pipeline, in its final shape, on the smallest useful case.

- `-static` non-PIE: objects and archives, symbol resolution with archive
  extraction, COMDAT, common symbols, default layout, the AArch64 static
  relocations, `.eh_frame` and `.eh_frame_hdr`, IRELATIVE for the IFUNCs of
  a static glibc, static TLS, `.init_array`, `.symtab`, build id, map file.
- The AArch64 erratum 843419 fix: Debian's and Ubuntu's GCC pass
  `--fix-cortex-a53-843419` on every link, so the option is honored (the
  scan for the sequence and the patches, `spec/model.md` §4.4) or the
  link is refused; it is never accepted without effect.
- A static C hello-world, then static C++ with exceptions, link and run.
- `--time-phases`; determinism test; first mortar rows in the bench.
- x86-64 and i386: the static-pie rows of the architecture specs are
  specified and tagged now, and `mortar_x64`/`mortar_x86` implement and
  unit-test them (`spec/x64.md` X-9, X-15, X-56, §10; `spec/x86.md`
  X86-27, X86-50, X86-51, §11); the end-to-end static-pie link test is an
  M2 gate, with the generic `.dynamic`/`RELATIVE` work.

**Done when:** the static subset of the link tests passes with the verifier
clean, and static C and C++ test programs behave as with every reference.

## M2: dynamic executables

What every default link line of GCC, Clang and kiln needs.

- Shared objects as inputs: `.dynsym`, versions (`verdef`/`verneed`),
  `DT_NEEDED`, `--as-needed`; implicit linker scripts (`libc.so`).
- PIE and non-PIE dynamic executables: `.interp`, `.dynamic`, `.dynsym`,
  `.gnu.hash`, GOT, PLT, dynamic relocations, copy relocations, canonical
  PLTs, RELRO, `.note.gnu.property`.
- TLS: descriptors, initial-exec, local-exec, and the relaxations between
  them; GOT relaxations.
- `--gc-sections`, mergeable sections, `--build-id`, `-z` options in use.
- Debug sections as data: relocation, tombstones, string merging.
- kiln's executing corpus and hod's link matrix with mortar (L7).

**Done when:** the default link lines of `gcc`, `g++`, `clang++`,
`gfortran` and `kiln++` work for the test corpus on AArch64 with the
verifier clean, and kiln's corpus passes with mortar.

Spec: `spec/dyn.md`; the package cut: `spec/landed/dyn/WP-040.md`
(WP-040). Shared objects are inputs only: producing them (`-shared`,
verdefs), RELR, i386 dynamic links and the IBT/PAC PLTs are M4. Joined in
the order: C PIE on AArch64, C++, x86-64 (under qemu), the options.
`--gc-sections` is WP-031.

## M3: libc++ with kiln and hod

- The libc++ lane: kiln and hod's sweep with mortar as the linker, strict
  against the control, without and with `-g`.
- gdb and lldb session tests (L6).
- Range thunks; everything the sweep and the session tests find.
- Start-up and small-link speed, since this lane is nine thousand links.

**Done when:** libc++ strict passes with and without `-g`; the time of the
link step over the suite is recorded against GNU ld, lld and mold.

## M4: completeness

Parallel tracks, each closing a class of real-world links.

- x86-64 to parity with AArch64 (starts in M1 as soon as `mortar_arch` is
  fixed; listed here because this is where it must be complete).
- 32-bit x86 to the same parity: ELF32 output, REL relocations with their
  implicit addends, the i386 TLS models, GOT-relative addressing and its
  relaxations, non-PIC executables with copy relocations and canonical
  PLTs. Inputs from GCC and Clang (kiln and hod do not target it); the
  class-generic readers, tables and writers it needs are in place from M0
  (`docs/ARCHITECTURE.md`, "Scope").
- Shared-object output: symbol visibility and export rules, version scripts,
  `-Bsymbolic*`, `--dynamic-list`, `DT_SONAME`, `DT_RUNPATH`. Designed by
  WP-110 (`spec/dyn.md` §17; the cut in `spec/landed/dyn/WP-110.md` §D):
  first joined target a C library with a soname on AArch64 (the default
  driver line and Python's `LDSHARED` line, `-Bsymbolic-functions`
  included), loaded by a GNU-ld-linked and a mortar-linked host,
  verified; then versions (rustc's `cdylib` line), C++, x86-64, the
  option tail. The cut: a docs step 0 and a code step 0 (the shared
  types), then six parallel worktrees with disjoint files (the `Shared`
  front/resolve/scan, synth/layout/image, the harness for library
  outputs, stage 3 for `Shared`, the executables' refusal of copies of
  locally bound writable data, the version-script parser); the first
  target is gated on their join, stage 3 and the harness. Non-PIC code
  that binds an exported datum is refused at the address level
  (`non-pic-in-shared-object`); the one form no rule sees yet (GCC
  AArch64 section anchor plus an instruction immediate) is an open
  question with a known-gap test.
- Linker scripts: `SECTIONS`, `MEMORY`, `PHDRS`, expressions, `PROVIDE`,
  `KEEP`, `INSERT`.
- `-r`, `-static-pie` (x86-64's rows from M1 and M2, above), `--wrap`, `--defsym`, `--exclude-libs`,
  `--start-lib`, `--push-state`, RELR, compressed debug sections,
  `.gdb_index`, `.debug_names`, identical code folding, `--emit-relocs`.
- External suites (L9): mold's and lld's tests with recorded pass lists.
- Open-source builds with GCC, Clang and gfortran objects; a Fortran lane
  (LAPACK's tests); mortar and Rust test binaries linked by mortar; musl.

**Done when:** the open-source lanes build and pass their own tests on the
three architectures, and each unsupported GNU ld feature is refused by name with
a row in `docs/status/support.md`.

## M5: the fastest

Design for speed is in the architecture from M0; this milestone is where it
is measured against the competition and won. It comes after M4, M6 and M7
are minimally mature ("Order of work").

- Parallel everything, the concurrent symbol table, memory layout of the
  tables, output writing; the decision on file mapping (`unsafe`, by
  measurement).
- Bench gate enabled; ratios against mold, lld and GNU ld recorded per
  release.

**Done when:** mortar is at least as fast as mold and uses no more memory
on every workload of the bench set, or the difference is explained and
accepted in `docs/status/bench.md`.

## M6: metadata and the one-definition rule

Needs the kiln and hod projects (see "Cross-project").

- `spec/meta.md`: the Mason metadata container, agreed across projects.
- Static checks in `mortar_odr`; the weaker checks for foreign objects.
- The loadable table and the load-time hook for shared objects.
- The violation corpus (L11).

**Done when:** every violation kind in `spec/odr.md` is diagnosed across
objects, archives and shared objects, with no false positive on libc++ and
the open-source lanes.

## M7: incremental linking

A launch feature, not an extra: it is worked on beside M4, ahead of M5
("Order of work"). It builds on what M1 fixed (layout is data, the image a
pure function of it, deterministic output) and benefits from M6's metadata
(per-section content hashes from kiln and hod); it must be correct before
it is fast, so the verifier judges every incrementally produced output as
it judges a full link, and the output of an incremental link is compared
with the full link of the same inputs (L10).

- `spec/incr.md`: the design, written during M2 (what is kept between
  links, how a change is detected, what an edit may move, when the link
  falls back, what kiln and hod hand over).

- Link state format; change detection; delta resolution.
- Layout with slack; in-place patching; moving a section and patching its
  referrers; fall back to a full link, with the reason.
- The incremental validation layer (L10) and bench cases.

**Done when:** L10 passes over long random edit sequences on the
open-source lanes, and the incremental bench cases are recorded.

## M8: 1.0

Release gate green (`docs/VALIDATION.md`), documentation for users,
packaging, a stability statement for the command line and the metadata
format.

## Cross-project

Requests to the other Mason projects, raised as early as their design
allows because they have their own queues:

| request | to | needed by |
|---|---|---|
| A `-fuse-ld=mortar` spelling and discovery of the `mortar` executable in kiln's driver; later, kiln running mortar directly | kiln | M2 (convenience), M5 (start-up time) |
| mortar as a linker in hod's link matrix and in both projects' libc++ sweeps | hod, kiln | M2, M3 |
| The metadata container: kiln emits records, hod carries them into a discardable section | kiln, hod | M6 |
| One-definition-rule records: per-entity definition hashes and locations | kiln | M6 |
| Per-section content hashes and `-ffunction-sections`-style granularity for incremental links | kiln, hod | M7 (design during M2) |
| What tuck needs the linker to keep or index | tuck | after M3 |
