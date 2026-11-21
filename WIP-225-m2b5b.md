# WIP WP-225 M2b-5b (the update's map states the output's `.symtab` blocks)

Brief: `/Volumes/AgentWork/mortar-helper/briefs/wp-225-m2b5b.md`; ruling
"After M2b-5's hand-back" item 1 of `briefs/wp-225-rulings.md`.

## Done
- a822d254 the fix: `refill_capture` (crates/mortar/src/update/mod.rs)
  gives `update_map` the front's `.symtab` plan blocks (`incr_blocks`)
  and a stored view whose `symtab_block` answers `None` (`OutputBlocks`,
  a private wrapper beside `refill_capture`). `refill` keeps the default
  plan (its `write_symtab` never reads `blocks`).
- caf07ff2 the test `the_update_map_states_the_output_symtab_blocks`
  (crates/mortar/tests/state_producers.rs); fails on the base, passes now.
- w10_local_to_global lane bytes before/after in the scratch
  (`.build/target-wp-155/w2b5b/w10-{before,after}-bytes.txt`).

## Next
- The gate once on the final tree (`w2b5b/gate.sh`), then the record
  section "M2b-5b" in `spec/landed/incr/WP-225.md`, hops-after table
  against M2b-5's `final-hops.txt` (210 taken).

## Resume
- Scratch `/Volumes/AgentWork/mortar-helper/.build/target-wp-155/w2b5b`.
