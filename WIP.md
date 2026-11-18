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

- Item 1 measured (protoc -g, 9 interleaved runs, load 7.9-8.3): base plain 0.260 s/250.7 MiB, plain
  0.270/249.9; rows 0.650/402.2, norows 0.610/396.3; DEBUG_STRS 77490 rows 1,064,762 B (2.95% of
  36,052,992); duty check on the real output 0 problems. Format 4 of protoc -g = 31,016,292 (U-2's 31.0).
  Evidence: .build/target-wp-157/wp247/cxx-medium-g-static-meas/.
- Item 6: capture::option_names (plan.option_keys() + OptionKeys::live_roots() names), ONE line in
  crates/mortar/src/front.rs (Incr5.option_keys); state5_rows adds rows for unmentioned names.
  Test state5_capture.rs option_names_have_their_rows (7 options, both targets).
- Item 3: normalize sorts DIAGS by the writer's key (the bug: append order of DIAGS strings).
  Tests: state5.rs decode_normalize_encode_is_byte_equal; state5_capture.rs capture_round_trip_is_byte_equal.

- Items 3 (real: protoc, protoc -g, clang -g byte-equal), 4 (state5::columns + state5_columns.rs; DIAGS.unit
  F-1 fix), 5/g (reseal, FILE_INDEX, every column mutated; UNCHECKED list), 9 (sizes), h: done. Record written:
  spec/landed/incr/WP-247.md (items 8 and 9 proposals in it).

## Next
- Gates (scratch gates.sh, log gates.log, final line "gates done"), then delete WIP.md, hand back.
