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

## Next
- spec/plumb.md text for item 1 (R9, §4.8 BTI entry, S11, §6.7 style,
  §10 observations), then item 2 (mutation harness).
