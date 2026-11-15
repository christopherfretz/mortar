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

- Item 0: erratum.rs C1-C6 (§5.10), mutation class erratum-fix, tests
  s9_2o_*; A4 judged, A18 (GNU ld unfixed site in synthetic-sections);
  xtask --fix-cortex-a53-843419 + rsp detection. Finding: main's plumb
  (4afb6df) already flags all pre-fix outputs with R1; the bench never
  runs plumb. Repros in /tmp/.../e843 (e,e3a,e4,me3a) and sec/.
  S7 entry symbol may be local (hidden _start).

## Next
- Full plumb tests; pipeline tests; then items 2/4 (protoc sweeps, cost
  table, §9.5 text), item 5 sizing, gate, linktest, landed note.
