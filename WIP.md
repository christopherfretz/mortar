# WIP — WP-063 (delta, refill set, dry run)

Done:
- format v2 (crates/mortar_incr/src/format.rs): MENTIONS 32, RESOLVED 33, INDEX 34; v1 still decodes (State.format).
- capture of the new tables (crates/mortar_incr/src/tables.rs), `capture()` takes `arch`.
- detect.rs (IN-22..24 from two states), delta.rs (compute + report), wired in crates/mortar/src/incremental.rs (dry_run, old state kept only with the report).
- parallel file/member hashes in incremental.rs `state()`.

Next:
- run incremental tests; unit tests for delta (drop equivalence); oracle lane under tests/incr-oracle/; xtask entry; docs (incr.md IN-18a, cli.md §3.13, §7); record; measurements.
