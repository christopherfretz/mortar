# WIP WP-225 gc-lift (branch `wp/225-gc-lift`, base 026f1b67)

## Done
- b7ed86e2 step 1 (M2b2b-1): `file_scan.rs` reads the row's path/member
  through `ChangedFiles::string`; `scan_front` removed (its call in
  `placement` removed too: 7 lines of M2b-2c's function).
- fd187d40 steps 2-5: record "gc-lift" in `spec/landed/incr/WP-225.md`;
  `run_with`'s `(live)` refusal removed; `live_judged` in `update/mod.rs`
  (live::live vs the front's mark, FINDING on disagreement); the update's
  `META.live_hash` written in `reference_of` (FINDING gc-1); F-10 rounds
  judged in `changed.rs` `u5_judged` (rounds-only goes on, flips
  `(resolve)` by name); `tests/update_gc.rs` (3 tests, both targets).
- 16997e40: lane scenario `gc_edit_keeps_liveness` (updates taken);
  `expect-full` of `gc_section_dies_then_revives`, `w8_gc_liveness_flip`
  now `(layout)`; `live_update.rs` `a_gc_update_falls_back_end_to_end`
  follows the lift; identity record by table.
- Parked (NOT in the tree, copies in the scratch `parked-scenarios/`):
  `gc_comdat_last_use_dies`, `gc_eh_owner_dies`: the dry-run oracle
  misses an `.eh_frame` piece group's hole on hop 1 (AArch64), on the
  BASE update code too (FINDING gc-3).

## Next / how to resume
- Gate script: `.build/target-wp-140/scratch-gc/gate.sh` (summary in
  `gate-summary.txt`); timing: `timing.sh` (`timing.txt`); the base
  build tree `scratch-gc/base`, `scratch-gc/base-target` is deleted
  after the timing.
- Findings counts: `python3 scratch-gc/cat.py <findings file>`.
