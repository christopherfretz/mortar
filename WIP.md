# WIP — WP-249 step 2 (I-47a in the linker)

Brief: /Volumes/AgentWork/mortar-helper/briefs/wp-249-step2.md. Step 1 is done and ruled (record
spec/landed/image/WP-249.md §1-§6). Scratch: /Volumes/AgentWork/mortar-helper/.build/target-wp-143/wp249/s2/.

Done (compiled, tested, fmt + clippy clean):
- crates/mortar_image/src/link.rs: `LinkTables::kept_copy_value` (private), called from
  `sym_value_of`'s Local arm when `facts.dead`.
- crates/mortar/tests/macro_import.rs: 6 tests pass (branches a64/x64/i386, GCC -g3 vs GNU ld
  x2, x86-64 gdb via qemu -g).
- tests/link/debug/macro-import-{c,cxx,gc,debuggers,fallback,hand}.test: pass with L3 off;
  before-binary fails all but fallback.
- protoc -g timing: before 237.1 ms, after 235.0 ms (15 interleaved), outputs byte-identical.

Next: full gate in background (s2/gate.sh -> s2/gate-summary.log, final line "GATE DONE"),
then the record's "Step 2", scratch cleanup, delete this file, hand back.

Findings for the supervisor:
- NEEDS RULING: with L3, the reference linkers' outputs of the -g3 lane tests get M2/M3 map
  findings (verifier attributes same-named .debug_macro by name, plumb §3.3) -> ORACLE.
- mortar's L3 findings are R1 (expected until WP-250).
- `.gcc_except_table` / dead-FDE fields into a non-alloc dropped group member would now get the
  redirect instead of 0; no producer.
- Merge / .eh_frame kinds are not redirected: falls back to the tombstone.
