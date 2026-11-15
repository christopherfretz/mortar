# WIP: WP-051 (dynamic linktest harness)

## Done
- Harness (committed): fixtures, shlib, env, then/count/allow, check elflint/ldd/gdb/lldb, Raw::Skip, per-test xfails, stage-3 probe; spec/harness.md documented; unit tests.
- 80 dyn cases under tests/link/dyn (kinds, shlib, dynsym, dynamic, reloc, layout, tools, census), 6 fixtures; xfails grouped by package. Full dyn run both arches: 0 FAIL/ORACLE/ERROR/XPASS, 31.8 s at -j 6.
- Merged main once (i386 run-wide xfail removed).

## Next
1. Gate `MORTAR_REQUIRE_TOOLS=1 cargo xtask ci --quick` (running), measure quick linktest time vs before (baseline ~45 s for 36 tests).
2. spec/landed/dyn/WP-051.md (findings from the case authors: in the supervisor's notes).
3. Delete WIP.md; rm /tmp/mortar-wp051-137416.
