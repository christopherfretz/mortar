# WP-086 work in progress (deleted in the final commit)

## Done
- Item 1 instrument: `crates/mortar/tests/incremental.rs`
  `replay_reproduces_and_each_table_matters` now replays each mutation
  with `--mortar-map`, runs the verifier on changed bytes and prints one
  `OUTCOME dir | table | kind | outcome` line per mutation (no outcome
  assertions yet: they come with the code of item 3). New mutations:
  CHUNKS (shndx), FRAGS (swap), SLOTS (swap), EH_RECORDS (swap, drop,
  dup, keep a CIE), SYMTAB_LOCALS (shift). The two assembly programs got
  two more 6-byte strings and (AArch64) a second GOT load.

## Half-done
- CHECKPOINT hand-back to the supervisor (item 1 table, item 2
  proposals). Waiting for the rulings before specs and code.

## Learned, not yet written
- Findings of the same kind (front plans as a second source): GotPlan's
  key lists (SLOTS swap: verifier rejects GotSlot), ShstrtabPlan from the
  computed placement (CHUNKS shndx swap passes Layout::check, headers get
  the computed names), SymtabPlan's patch copy, merge `Order.order`
  (sorted by the computed map), erratum fixes (F8).

## Next
- After the rulings: specs (synth.md S-49 + §7 S-88, layout.md, image.md,
  incr.md IN-18a/IN-61), then `EhPlan::from_records`, front rebuilds after
  the replay, state version, test assertions.
