# WIP WP-167 (as-needed extraction)

Rule approved with conditions (1 final-stage diagnostics, 2 U/W notes,
3 cells 7a/7b measured, 4 least-fixed-point text + cost + stage counter,
5 map reason, 6 D-19a order). Code + spec in commits 01c83354, b06ac9a9.
Scratch `/Volumes/AgentWork/mortar-helper/.build/target-wp-141/wp167/`
(`build.sh`, `cell.sh`, `matrix.sh`, cells*.txt, results *.tsv).

Done: resolve stages, unit tests (crates/mortar_resolve/tests/shared.rs),
`resolve count stages` step counter, counter test
`crates/mortar/tests/as_needed_stages.rs`.

Next: cost timings (--time-phases, 1/2/3 stages vs --no-as-needed);
lane tests tests/link/dyn/shlib/as-needed-extract*.test; static-identical
(entry_name map change: check existing rows); spec/diag.md, D-19a text;
spec/landed/dyn/WP-167.md; merge main; gate + lane + census in background.
