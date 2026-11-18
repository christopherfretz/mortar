# WP-247 WIP (delete in the final commit)

Brief: /Volumes/AgentWork/mortar-helper/briefs/wp-247.md. Scratch: .build/target-wp-157/wp247 (build.sh <log> <cargo args>).

## Done
- Item 1 format side: DEBUG_STRS (57, KeyKeyed, 13) in TABLES, Rec/Keyed (DUPLICATES), reader
  `debug_str`, encode/normalize (`compact::debug_strs_whole`)/canon, capture `debug_str_rows`
  (capture.rs, one call line in rows5), MemoryState field. Tests: state5.rs `debug_strs_*` (3),
  state5_capture.rs `debug_strs_name_the_output_strings`. duty.py JUDGED_BY_OUTPUT +
  `debug_strs_check` + selftest `debug-strs-*`.

## Next
- Item 1: seam 8a cherry-picked (d148c1e0) and applied (plan_in_place, compact::apply; deletes refused;
  test debug_strs_patch_puts_rows). LEFT: measurement on protoc -g (cxx-medium-g-static).
- Items 2..10 per the brief.
