# WP-076 work in progress (deleted in the final commit)

Done:
- O-23 `--odr-check-archives`: `mortar_odr::archives`, `odr-archive-shadow`
  (diag kind 75), CLI row, register row 18, stats line; link tests
  `tests/link/odr/k17-archive-{shadow,silent,thin}.test` pass on all three
  arches.

Next:
- read-counter test with each option off (crates/mortar/tests/odr_dynamic.rs style)
- `--odr-table=full` (second note type, reader in mortar_meta::table)
- `--keep-mason-meta` (SynthKind 27 `.mason.meta`)
- measurements (cost on/off, corpus O-1, strip tools), spec edits, record
