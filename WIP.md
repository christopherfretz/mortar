# WIP — WP-083 (M2 errata; `.relro_padding` -> `.pad.relro`)

Done: rename (code, tests, specs outside landed/), aarch64 baseline re-cut
(211 rows: output+map only), name-only proof (old/new kept linktest runs
under .build/wp083, comparator in /tmp scratch).
Half-done: findings table, spec errata, test comments/xfails, diag.md rows.
Next: build the findings table in spec/landed/dyn/WP-083.md, apply edits,
then gate, push branch, hosted run for the x86-64 rows.
