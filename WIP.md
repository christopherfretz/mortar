# WIP: WP-028d (deleted in the final commit)

Scratch: /tmp/mortar-wp028d-447239 (remove by explicit path at the end).

## Done
- Item 1: BTI IPLT entry starts at its `bti c` (ifunc.rs scan); pipeline
  BTI tests assert clean. R9 (veneer to no landing pad under BTI) with
  chained veneers (GNU ld pad veneer, lld 22 BTIThunk fall-through);
  corpus case `bti`; mutation s9_2n. S11: last note may be unpadded
  (gABI). B1 reads the style from the build-id chunk's target
  (`Style::of`); test s9_2l_build_id_style_from_the_map.
- Observed: lld 19 writes `ldr; br x16` to a padless target in a BTI
  output (R9) — lld 22 does not.

- Item 2 code: mutate.rs (22 classes), `xtask plumb --mutate`, test
  s9_5_mutation_coverage_on_the_small_corpus. New S5/S7/S9/S11/S13
  clauses, S20 (RELRO), S21 (EH_FRAME phdr), W1 NOP-ADRP hole closed.
- Item 3: X-48 rule (x64rec.rs) + test; F5 on mortar IPLT checked.
- Item 0 repro: pre-fix mortar (4afb6df) at .build/target-wp-028d-pre;
  e843/{e,e3a,e4,me3a} flagged R1 by this worktree's plumb.

## Next
- Item 0: was main's plumb silent? new §11.3 rules (missing fix, patch
  return), mutation tests, bfd/lld checks; plumb.md text.
- Items 2/4: release sweeps on protoc (a64 + x64), cost table; §9.5 text.
- Item 5 sizing; gate; linktest; landed note.
