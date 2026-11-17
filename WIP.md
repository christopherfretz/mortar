# WIP: WP-126 (C++ in -shared)

Brief: /Volumes/AgentWork/mortar-helper/briefs/wp-126.md. Scratch: .build/target-wp-145/wp126/
(lt.sh runs the lane on given tests; rb.sh runs realbuild with --work wp126/rb;
scripts/cmpso.py compares .dynsym/NEEDED/SONAME of kept outputs).

Done: 7 lane tests tests/link/dyn/cxx/shared-*.test, all PASS (mold x86_64 skip in
static-libstdcxx: verifier S9 on mold's .tbss-only PT_TLS, to report). Export sets: mortar ==
GNU ld on every library of the lane tests but GNU ld's version-node symbol (D-110 item 3) and
section symbols (cmpso-lane.txt). Manifest row googletest-shared + projects/googletest-shared.sh.
Refusal found: `g++ -shared plain.o -lgcc_s -Wl,--no-as-needed` -> NeededOrderUnknown (script
mention's as-needed state not recorded; mortar_input+mortar_core+mortar_resolve: report).

Running: realbuild googletest-shared gcc --keep (then clang --keep; then zlib, lua, googletest
default, no keep). Do NOT --update-baseline (rewrites the report); dated section by hand.
Next: compare gtest .so control vs mortar; record spec/landed/dyn/WP-126.md; static-identical
--update for the new tests; gate + lane on final tree.
