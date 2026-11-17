# WP-127 part 1 (x86-64 shared objects) — work in progress

Brief: `/Volumes/AgentWork/mortar-helper/briefs/wp-127.md`.

## Done
- Read the briefs and the spec (x64 X-59..X-62, dyn D-100..D-122, WP-110 §D/§G, WP-123).
- Step 1 corpus: `.build/target-wp-127/x62/build.sh` (gcc, clang19, clang22, kiln/llc
  at O0..Os; kiln/hod at O0; -fPIC/-fPIE/-fno-pic; lua, zlib, gtest, the cases),
  `analyse.sh` (x86gt.py per producer x level x model; s1d.py --x86 over -fPIC).

## Next
- Read the measurement (out/TABLE.txt, out/CODEPC.count).
- arch_status accepts x86-64 Shared; weak_branch only in executables (X-59);
  bound.rs x86-64 scope (X-62) + view.rs gate; tests.

## Open questions
- none yet
