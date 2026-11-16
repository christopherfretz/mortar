# WIP — WP-083 (M2 errata; `.relro_padding` -> `.pad.relro`)

Done: rename, both baselines (x86-64 rows from hosted run 43786825990),
name-only proof, spec errata (record spec/landed/dyn/WP-083.md §2),
test comments/xfails, diag.md §4.2.
Next: hosted re-run green on both lanes; local gate `[ci] ok`; fill the
record's §6; delete this file in the final commit; remove the scratch.
