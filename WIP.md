# WIP: WP-228 (G-1) part B2, branch `wp/228-incr-debug-b`

Rulings: `/Volumes/AgentWork/mortar-helper/briefs/wp-228-rulings.md`, last section "After B1".

Done (commits): seam 8a 1144efcd; R-1 f7975165; `update/debug.rs` +
`tests/update_debug.rs` 1324c248 (23 decision tests, green); record
part B2 1e9781e4 (`spec/landed/incr/WP-228.md` "Step 2, part B2").

Gate: script `.build/target-wp-154/wp228/b2/gate.sh`, logs in `wp228/b2/gate/`.

NOT done (next agent, in this order):
1. The test-only writer under `crates/mortar_incr/tests/` that applies a
   `DebugPlan` to a copy of a full link's output (relocate the changed
   file's debug sections at their new places, `neutral::unit` for each
   hole, appended strings, aranges), then every named test of the record
   B2.2 compared with a full link via gdb, lldb, `llvm-dwarfdump --verify`,
   readelf (and `cargo xtask plumb` once WP-253 is picked).
2. WP-229 §8: list what G-1 takes over.
3. Rulings on record B2.3 (the mortar_incr -> mortar_layout edge etc.).
