# WP-093 WIP

Done:
- `xtask/src/identical.rs`: `DYNAMIC_CASES` gone, corpus is every dir of `tests/link`.
- `xtask/src/linktest/engine.rs` `normalise_map`: gcc's `-fresolution=.../ccXXXXXX.res` map word.
- Item 1 runs: before 301 links/81 tests 29.6 s; with dyn 806/205; only `map` columns moved (404 rows), cause the `.res` name.

Next: confirm 0 differ across TMPDIR/-j after normalisation; checkout-at-other-path run;
mutation checks (PIE, static-pie) in a scratch copy; spec/harness.md; record
spec/landed/harness/WP-093.md; hosted run for the x86-64 baseline.
