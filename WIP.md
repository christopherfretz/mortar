# WIP: WP-186 (RL-6), deleted in the final commit

Done (uncommitted until the first commit after compose7 passes):
- `compose.rs` P-P1 + `tests/compose.rs` (passes, mutants, exclusions);
  rulings 5/13/15/16, RL-2 rulings 1-3 in relocatable.rs/relsym.rs + tests
  (q5, q15, rv-r6); §14.5 readings, §14.8 rulings, A120, harness.md.
- xtask: stage R on `-r` link outputs; `partial mortar` (+LT-R4);
  `plumb --compose`; census `-r` class (13 variations, OUT_OF_M4R, summary).
- smoke tests `tests/link/relocatable/` (3 + WP-203 xfail + partial-mortar
  pending).

Next:
1. compose7 results -> fix -> commit.
2. linktest on tests/link/relocatable (background, load-gated).
3. item 6 fixtures (build-id first).
4. record spec/landed/harness/WP-186.md; final gate; delete WIP.md.

Resume: cargo per the brief's env; scratch `.build/target-wp-154/rl6/`
(`run.sh <log> <cmd>` is load-gated).
