# WP-247 WIP (delete in the final commit)

Brief: /Volumes/AgentWork/mortar-helper/briefs/wp-247.md. Scratch: .build/target-wp-157/wp247 (build.sh <log> <cargo args>).

## Done
- Item 1 format side: DEBUG_STRS (57, KeyKeyed, 13) in TABLES, Rec/Keyed (DUPLICATES), reader
  `debug_str`, encode/normalize (`compact::debug_strs_whole`)/canon, capture `debug_str_rows`
  (capture.rs, one call line in rows5), MemoryState field. Tests: state5.rs `debug_strs_*` (3),
  state5_capture.rs `debug_strs_name_the_output_strings`. duty.py JUDGED_BY_OUTPUT +
  `debug_strs_check` + selftest `debug-strs-*`.

## Next
- Item 1: seam 8a cherry-pick (hash from the supervisor) -> plan_in_place `ed.keyed(DEBUG_STRS)`
  and compact::apply keyed, deletes refused; area lookup test. Measurement on protoc -g.
- Items 2..10 per the brief.
