# WIP WP-072 (deleted in the final commit)

Done: diag kinds (13 + odr-contents-mismatch, Relation::DefinedDifferentlyIn,
diag.md rows); plan::Odr (+check_archives, table, keep_meta); cli rows;
mortar_odr (register.rs O-47 + test, foreign.rs W1-W4, lib.rs check + stats);
front.rs call after resolve; Timer::line for --odr-stats; xtask `diag:`
directive (linktest/odr.rs, format.rs, engine.rs).

Done also: options audit, cli.md counts, options.md, crates/mortar/tests/odr.rs,
tests/link/odr (9 tests, pass on 3 arches).

Next: oracle (wrapper runs of linktest + static-identical, design corpora),
cost (bench), record spec/landed/meta/WP-072.md, gate.
Scratch: /tmp/mortar-wp072-main.
