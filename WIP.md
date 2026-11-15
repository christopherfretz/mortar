# WIP WP-072 (deleted in the final commit)

Done: diag kinds (13 + odr-contents-mismatch, Relation::DefinedDifferentlyIn,
diag.md rows); plan::Odr (+check_archives, table, keep_meta); cli rows;
mortar_odr (register.rs O-47 + test, foreign.rs W1-W4, lib.rs check + stats);
front.rs call after resolve; Timer::line for --odr-stats; xtask `diag:`
directive (linktest/odr.rs, format.rs, engine.rs).

Half-done: options audit evidence (crates/mortar/tests/options.rs AUDIT),
spec/cli.md §4.1 counts, docs/status/options.md regen.

Next: tests/link/odr/*.test; crates/mortar/tests/odr.rs (off byte-identity,
stats, detect noop, threads); oracle; cost; record spec/landed/meta/WP-072.md.
Scratch: /tmp/mortar-wp072-main.
