# WIP: WP-125 part 2 (symbol versions in a shared object)

Done: measurements (.build/target-wp-125/meas); CLI rows; resolve version.rs
(§9.7, rule 9, rule 12, duplicates); driver; synth verdef/versym/dynstr/dynamic;
link tests tests/link/dyn/version/ (11); version_table.rs (195/213, 18 C++);
diag/synth expectation updates; spec edits D-115, diag.md versioned-export,
cli.md §4.2.
Next: crate tests green; zlib census (.build/target-wp-125/zlib/run.sh);
static-identical rows; record part 2; merge int/batch-6; full gate + linktest.
Known: plumb s3_2a fails (ACCEPTED lacks --version-script) -> tell supervisor.
