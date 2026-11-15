# WP-015 (hosted CI green on both lanes): work in progress

Deleted in the final commit.

- Done: scripts/ci-tools.sh lists (TOOLS, TRIPLE_TOOLS, LLVM_TOOLS,
  SETUP_TOOLS, FALLBACK_TOOLS), Debian's LLVM 19 (clang-19, lld, lldb)
  and the harness's coreutils/procps names; pins (LLDB/clang-19/ld.lld,
  gdb, elfutils, qemu); the tool guard (mortar_testutil::ci_tools,
  find_listed_tool; the link-test tables' test); named skips
  (mortar_testutil::named_skip, listed by xtask ci); `cargo xtask ci
  --no-fail-fast` and the by-hand `no_fail_fast` input;
  mortar_testutil::target and the AArch64-subject tests on every host;
  static-identical: one baseline per host (baseline.on-<host>.tsv; a
  failed check prints the run's rows); tests/link/anomalies `cross`
  (A13 of mold is the cross x86-64 C runtime's).
- Done (x86-64 lane): its static-identical baseline; named skips
  through MORTAR_NAMED_SKIPS (xtask's own gate test records into a
  directory of its own: it had emptied the outer gate's); `exit:
  no-start`; `.note.package` (front_diff); rustc's rust-lld and
  static-pie (census); gc's COMDAT keepers tolerated on the cross
  libstdc++ only; plumb corpus allows lld's `__CortexA53843419_*`.
- Merged main 2a31ada (the M2 train; the second of three merges).
  front_diff's `.eh_frame` mapping-symbol bound: 4 on the native
  runtime, 16 on the cross one with `.eh_frame` size and CIE/FDE counts
  lld's (ruling (a); cause suspected, not proven).
- After the merge: main's new host-assuming tests on their subject's
  tools (shlib_oracle and its scripts, scan dynamic, a64 dynamic_ref,
  debug_gc, synth dynsym verneed); baseline.on-x86_64.tsv gains 030d's
  15 rows (from run 43743521473). Stage 3 on x86-64: 0 FAIL.
- Open: two green plain runs; the triggers and spec/harness.md
  "Hosted CI"; the landed record.
