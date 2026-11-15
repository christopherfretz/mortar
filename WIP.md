# WIP — WP-032g (deleted in the final commit)

Done:
- `par`: `MORTAR_STEPS` region lines (`par <file>:<line>`), lazy tree
  spawn in `drive` (`Team`), `Pool::unit_cost` + `WORKER_NS`; tests in
  `crates/mortar_core/tests/par.rs` (s6_unit_cost_*, s6_a_small_region_*,
  s6_nested_regions); spec/model.md §6 paragraph.

- call sites annotated with `unit_cost` outside 032f's crates (golden
  IDENTICAL vs 543fd4e, 36 links).

Half-done / next:
- annotate mortar_image / mortar_scan sites (separate commit).
- item 2: archive members symbol-table-only residency (`mortar_input/src/store.rs:1113-1170`).
- items 4-7 after.

Measuring: /tmp/mortar-wp032g-652866 (bench.py, golden.sh, workloads with static rsp).
