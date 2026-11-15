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
- Next: cut tests/identical/baseline.on-x86_64.tsv from the x86-64
  lane's log; the next diagnosis run's failures; two green plain runs;
  then the triggers and spec/harness.md "Hosted CI".
