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

2. Capture (commit "P-275a step 2"): capture.rs `chunk_unit_rows`,
   `chunk_units_check`, `merge_def_rows`; called from `rows5` (MERGE_DEFS,
   with the section's capture id) and `state5_rows` (CHUNK_UNITS; `local`).
3. Judges (commit "P-275a step 3"): `incr_state check <s>` (exit 1 names
   the first difference); judge.py `CHUNK_UNITS` in `D0_PLACEMENT_TABLES`,
   the `units` column (`_units`, every hop with a state).
4. Tests (commit "P-275a step 4"): `state_producers::the_chunk_units_are_the_derived_index`
   (geometric reference, CHUNKS.members under is_clip_member, VERSION 12
   refused by an update), `state_producers::merge_defs_are_the_full_links_pieces`
   (5 definitions vs `named_symbol`), a64 + x64: pass.

5. Lane at 6c38aa4e: 7 FAIL, duty only (CHUNK_UNITS not named by IN-78);
   PROPOSED 72c81598 holds it out (judge.py DUTY_BY_UNITS), for a ruling.
   Cost measured (record section "P-275a" f); record written.

## Next
6. The hand-back gate set on the final tree (scratch `w275a/gate.sh`),
   then the gate table into the record (e, and a gate line) and hand back.

## Resume
Gates: `/Volumes/AgentWork/mortar-helper/.build/target-wp-125/w275a/gate.sh`
(summary in `w275a/gate/summary.txt`).
