# WIP — WP-221 (U-9)

Done: step 1 experiments in progress. `mortar_diag::subject`: `subjects`
(diag.md's column; spec_tables compares both ways, WARNING_KINDS gone),
the PROPOSED stored form `stored`/`from_stored` with its unit tests.
Fixture `tests/incr-update/scenarios/gnu_warning_reference_removed`.

Not committed, experiment only: `crates/mortar/src/main.rs` probe
(`MORTAR_WP221_PROBE`), reverted with `git checkout crates/mortar/src/main.rs`
after the lane+census probe run (`.build/target-wp-156/wp221/probe-run.sh`).

Next: record `spec/landed/incr/WP-221.md` "Step 1"; interim hand-back;
then step 2 after the ruling.
