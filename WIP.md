# WP-076 work in progress (deleted in the final commit)

Done:
- O-23 `--odr-check-archives`: `mortar_odr::archives`, `odr-archive-shadow`
  (diag kind 75), CLI row, register row 18, stats line; link tests
  `tests/link/odr/k17-archive-{shadow,silent,thin}.test` pass on all three
  arches.

- O-23 read counter: `odr_records.rs::archive_members_are_read_only_on_request`.
- `=full`: second note NT_MASON_ODR_FULL 0x4d4f4402 (`table::full_note`,
  `FullTable`, `find_full`), rows from `dynamic::full_rows`; `xtask meta
  table` takes `hidden`/`type` lines (and sorts .dynsym once); tests
  `odr_dynamic.rs::full_table_in_a_dynamic_executable`,
  `tests/link/dyn/odr/o50-full{,-input}.test`, mutation tests in table.rs.

Next:
- `--keep-mason-meta` (SynthKind 27 `.mason.meta`)
- measurements (cost on/off, corpus O-1, strip tools), spec edits, record
