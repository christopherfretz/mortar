# WP-015 (hosted CI green on both lanes): work in progress

Deleted in the final commit.

- Done: scripts/ci-tools.sh lists (TOOLS, TRIPLE_TOOLS, LLVM_TOOLS,
  SETUP_TOOLS, FALLBACK_TOOLS) and the debuggers, elfutils, file, i686
  gfortran, pins (LLDB, gdb, elfutils, qemu); the tool guard
  (mortar_testutil::ci_tools, find_listed_tool; the link-test tables'
  test); named skips (mortar_testutil::named_skip, listed by xtask ci);
  `cargo xtask ci --no-fail-fast` and the by-hand `no_fail_fast` input.
- Next: the diagnosis run's failures on both lanes; the tests that skip
  silently off an AArch64 host (std::env::consts::ARCH checks) made
  host-independent.
