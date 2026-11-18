# WIP — WP-249 step 2 (I-47a in the linker)

Brief: /Volumes/AgentWork/mortar-helper/briefs/wp-249-step2.md. Step 1 is done and ruled (record
spec/landed/image/WP-249.md §1-§6). Scratch: /Volumes/AgentWork/mortar-helper/.build/target-wp-143/wp249/s2/.

Done (uncompiled until noted):
- crates/mortar_image/src/link.rs: `LinkTables::kept_copy_value` (private), called from
  `sym_value_of`'s Local arm when `facts.dead`.
- crates/mortar/tests/macro_import.rs: branch tests (a64, x64, i386), GCC -g3 vs GNU ld
  (units by contents), --incremental=full equality, x86-64 gdb via qemu -g.
- tests/link/debug/macro-import-{c,cxx,gc,fallback,hand}.test.

Next: build, run the unit tests, the lane on tests/link/debug, then static-identical,
the protoc -g timing, the full gate, the record's "Step 2".

Findings for the supervisor (so far):
- `.gcc_except_table` / dead-FDE fields into a non-alloc dropped group member would now get the
  redirect instead of 0 (the helper does not see the referring section); no producer.
- Merge / .eh_frame kinds are not redirected (value is not K + offset): falls back.
