# WP-247 WIP (delete in the final commit)

Brief: /Volumes/AgentWork/mortar-helper/briefs/wp-247.md. Scratch: .build/target-wp-157/wp247 (build.sh <log> <cargo args>).

## Done
- Item 1 format side: DEBUG_STRS (57, KeyKeyed, 13) in TABLES, Rec/Keyed (DUPLICATES), reader
  `debug_str`, encode/normalize (`compact::debug_strs_whole`)/canon, capture `debug_str_rows`
  (capture.rs, one call line in rows5), MemoryState field. Tests: state5.rs `debug_strs_*` (3),
  state5_capture.rs `debug_strs_name_the_output_strings`. duty.py JUDGED_BY_OUTPUT +
  `debug_strs_check` + selftest `debug-strs-*`.

- Items 2+7: file_flags::DEBUG_EXPORTS = 16 (bit 4: "bit 2" was format 4's numbering; 2/4/8 taken),
  ALL = 31; capture::object_flags/eh_fixed/debug_exports (per file, one call line in rows5's per-file
  task: `acc.flags = object_flags(inputs, file)`); read::eh_fixed_check (the plan's check, for the join).
  Tests: capture_flags.rs eh_fixed_is_ins_34a_sentence; state5.rs eh_fixed_check_reads_the_bit;
  state5_capture.rs object_flags_on_a_real_link (0 EH_FIXED members on both targets).
- Measurement switch: MORTAR_INCR_NO_DEBUG_STRS (testhooks build) in debug_str_rows.

## Next
- Item 1: seam 8a cherry-picked (d148c1e0) and applied (plan_in_place, compact::apply; deletes refused;
  test debug_strs_patch_puts_rows). LEFT: measurement on protoc -g (cxx-medium-g-static).
- Items 2..10 per the brief.
