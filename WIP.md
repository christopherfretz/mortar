# WIP — WP-221 (U-9)

Done (step 1): `mortar_diag::subject`: `subjects` (diag.md's column;
spec_tables compares both ways, WARNING_KINDS gone), the PROPOSED stored
form `stored`/`from_stored` with unit tests. Fixture
`tests/incr-update/scenarios/gnu_warning_reference_removed`. Record
`spec/landed/incr/WP-221.md` "Step 1" (design, seam diff, experiments,
F-1..F-8). Interim hand-back sent; waiting for the ruling.

Experiment scratch: `.build/target-wp-156/wp221/` (probe-lane.tsv,
measure.txt, glibc/, gw/). The main.rs probe was never committed.

Next (after the ruling): step 2 — `diags` + `row`/`diagnostic` in
`update/diags.rs`, the five named tests in `tests/update_diags.rs`
(remove `diags_until_it_lands`), then the full gate.
