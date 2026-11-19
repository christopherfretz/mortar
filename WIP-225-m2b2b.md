# WIP WP-225 M2b-2b (branch wp/225-m2b2b)

Done:
- 1. M2b2a-1: `front_of_changed` (file_front.rs), layout_in's workaround gone,
  test `tests/update_placement.rs` (appeared file). Commit 55d5b08b.
- 2. Drawn-in design note in `spec/landed/incr/WP-225.md` "M2b-2b": stored
  range decided; edits outside the package listed; drawn-in keeps `(layout)`.
- 3. `reference_of` (update/mod.rs): `placement()` = layout_inputs, scan_of
  demands, defined_facts -> delta.resolved facts (R-4a, judged vs capture),
  delta::layout, `judge()` (M2b2a-2 properties). The image cannot take the
  delta's placement (only the front's replay hook, a whole Layout in front
  ids; no format-5 materializer): STOP clause, update falls back (layout)
  when not identity; identity case requires the delta at the full link's
  offsets. Commit e69f7e96.

Next: items 4-6 (C++ fixture with fewer FDEs, F-9 new-key test, e2e tests)
not started; the full gate.

Resume: scratch /Volumes/AgentWork/mortar-helper/.build/target-wp-145/scratch-m2b2b,
runner `t.sh <log> cargo ...`.
