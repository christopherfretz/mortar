# WIP: WP-155 (D-25's call-only cell)

State: measured (scratch `.build/target-wp-140/wp155/m/`: `meas.sh`, `table-before.txt`,
`table-after.txt`, `crt.sh`). Code: `crates/mortar_scan/src/weak.rs` `WeakUses::verdict`
(a branch alone -> WeakDyn; branch + unfillable stays Weak0 in Exec), unit test
`d25_verdicts`, `crates/mortar_scan/tests/dynamic.rs` `d25_weak_classes` updated.
Default C/C++ links (-pie, -no-pie, static-pie, both arches) byte-identical before/after.

WAITING for the supervisor's ruling on the mixed cell (branch + unfillable use in Exec):
GNU ld and lld disagree on x86-64 (GNU ld: Weak0, call 0, SIGSEGV even with LD_PRELOAD;
lld: JUMP_SLOT, the preloaded definition is called, the test reads 0); on AArch64 both
write the JUMP_SLOT.

Next after the ruling: the spec (D-25 "Then:" + closing Weak0 sentence, D-11 wording,
§13.1 row, a64 §7.1, x64 X-57), link tests under tests/link/dyn/reloc, the
weak-class-table branch row, static-identical, census, the lane, the record WP-155.md.
