# WIP: WP-228 (G-1) part B3, branch `wp/228-incr-debug-b`

Rulings: `/Volumes/AgentWork/mortar-helper/briefs/wp-228-rulings.md`, last section "After B2" (B3 items 1-6).
Scratch: `/Volumes/AgentWork/mortar-helper/.build/target-wp-154/wp228/b3` (`t.sh <log> <cargo args>` runs cargo in this worktree, waits for load, ends `T-END rc=`; logs `bytes-run1.log`, `bytes-run2.log`).

## Done (commits)
- 4058725e B2-1: edge mortar_incr -> mortar_layout; `slack_of` = `slack::neutral` + `Slack::INCREMENTAL.allotment`; mirror + `classes_mirror_slack` deleted; `forms_agree_with_slack` (unit) holds debug.rs's six minima = slack's.
- 2dae6980 B2-6: `split::units` refuses a 64-bit unit (`debug-unit-kind`, "a 64-bit DWARF unit"); test `dwarf64_unit_in_changed_file_is_debug_unit_kind` (tests/update_debug.rs; real gcc/clang objects, -gdwarf-4/5, -gdwarf64 vs -gdwarf32, both targets).
- 52f1a1ed + aa32ab7a item 2+3 (part): `crates/mortar/tests/update_debug_bytes.rs`, the test-only writer (module doc says how) and 3 named tests, green (36 judged outputs: 3 edits x 8 configs {aarch64,x86_64}x{gcc,clang}x{dwarf4,5} x {emulated DEBUG_STRS rows, no rows}; no-rows on DWARF 5 and some others = `debug-compaction` (string tail, size/32) and skipped by design):
  `in_place_line_edit_reads_as_the_full_link`, `appended_string_reads_as_the_full_link`, `tail_and_dead_range_read_as_the_full_link`.
  Judges: gdb + lldb transcripts equal to F (AArch64 run; x86-64 static questions only), `llvm-dwarfdump --verify`, readelf no new warning; IN-54 `aranges()` == the image's own `.debug_aranges`.

## Findings so far (for the hand-back)
1. `split::classify_object` (G-2) refuses `.debug_types` and COMDAT `.debug_info` (type units) as `debug-unit-kind`, so `place`'s R-5 (a)-(c) is unreachable through the join: a ruling is needed on which wins.
2. The merged strings' tail reserve is `size/32` (chunk.rs `debug_tail`): until WP-247's `DEBUG_STRS` rows exist every string of a changed file is appended (misses), which overflows the tail on small programs (`debug-compaction`); with rows only new strings are appended.
3. `.debug_abbrev`: GCC's per-CU abbrev contribution is often exactly a class size (remainder 0, no slack): any new abbreviation sends it to the tail = `debug-compaction` (B2-3). Seen when b.c gained `long` members on one line.

## Next (in order)
1. Item 3 rest: C++ (g++/clang++, `-fdebug-types-section` group test: R-5 a, note finding 1), Fortran, kiln/hod once, a dead-range REUSE (two changed files: b grows to the tail, c then fits b's old allotment), removed contributions. Add as `Edit`s/scenarios in update_debug_bytes.rs; `MORTAR_WP228_KEEP=<dir>` keeps outputs for `cargo xtask plumb` (after WP-253's range is picked).
2. B2-3's count on protoc -g objects (body edit, added function, added local type): does `.debug_abbrev` fit its allotment/dead range.
3. Item 4: time `place`/`strings`/`aranges` on protoc -g state (record §B2.7 terms; propose running sums in META/OUTSECS).
4. Item 5: WP-229 §8 list. Item 6: R-8's sentences + IN-57 sentence into spec/incr.md (one commit), record part B3, whole gate (`wp228/b2/gate.sh` as model), delete this file.
