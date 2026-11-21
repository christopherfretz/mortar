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

## State
- Gate green on 0d95e313 (table in the record, "M2b-5b"); lane 210 taken,
  the same set as M2b-5's. Handed back; nothing pending.

## Resume
- Scratch `/Volumes/AgentWork/mortar-helper/.build/target-wp-155/w2b5b`
  (logs: gate.log, final-hops.txt, final-findings.txt, w10-*-bytes.txt).
