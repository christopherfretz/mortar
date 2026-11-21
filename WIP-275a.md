# WIP-275a: state VERSION 13 (`CHUNK_UNITS`, `MERGE_DEFS`)

Brief: `/Volumes/AgentWork/mortar-helper/briefs/p-275a.md`. Branch
`wp/275a-format` from int/batch-27 15c66f4c. Target dir
`.build/target-wp-125`, scratch `.build/target-wp-125/w275a`.

## Done
1. Format (commit "P-275a step 1"): kinds 58/59, `ChunkUnitRow` (22 B),
   `MergeDefRow` (49 B), `unit_kind`, VERSION 13, TABLES 40, `name()`,
   COLUMNS, MemoryState/StatePatch fields, encode/decode/check_sorted,
   apply/plan_in_place, normalize (tombs dropped, files mapped, `local`
   checked against the file's range, seq renumbered k<<32 per chunk),
   canon (both tables sorted after normalize: seq-blind).
   Tests: `state5_columns::chunk_units_and_merge_defs_survive_the_patch_and_the_compaction`,
   `state5_columns::incr_state_canon_is_seq_blind_and_a_reorder_differs`.

## Next
2. capture.rs `chunk_unit_rows`, `chunk_units_check`, `merge_def_rows`
   (written, compiles; to commit with step 2).
3. `incr_state check`; judge.py `D0_PLACEMENT_TABLES` + `units` column.
4. `state_producers` tests (a64, x64).
5. Cost (protoc), gates, record section in `spec/landed/incr/WP-275.md`.

## Resume
`( cd <worktree> && unset CARGO_TARGET_DIR && source scripts/env.sh &&
export CARGO_TARGET_DIR=/Volumes/AgentWork/mortar-helper/.build/target-wp-125 && cargo test -p mortar_incr --test state5_columns )`
