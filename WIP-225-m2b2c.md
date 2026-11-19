# WIP WP-225 M2b-2c (branch wp/225-m2b2c)

Done:
- 1 M2b2b-3: cause = the state has NO `REGIONS` rows (no full-link producer;
  `capture::state5_rows` never fills `MemoryState.regions`). `delta.rs`
  `Update::file` sent every section to overflow. Fix: refused
  `update-not-implemented (layout)` when a stored place has no region.
  Test `tests/update_placement.rs` `a_same_size_edit_keeps_its_place_given_its_regions`.

Next: 2 (M2b2b-2, L-159 empty-only output section), 3, 4, 5, 6.
Resume: scratch /Volumes/AgentWork/mortar-helper/.build/target-wp-145/scratch-m2b2c (scripts t1.sh, t2.sh, lane1.sh).
