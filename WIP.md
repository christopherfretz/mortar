# WP-088 work in progress (removed in the final commit)

Brief: /Volumes/AgentWork/mortar-helper/briefs/wp-088.md. Plan: items 1 and 2
first (interim hand-back allowed), then 3-6.

## Done (uncommitted until the tests pass)

- F1: `dyncheck.rs` pd11_13: RELRO comparison only when the output has a
  PT_GNU_RELRO; new `Format::Mortar` rule: the copy is in `.bss.rel.ro`
  (read-only definition) or `.bss` (writable). KNOWN_FINDINGS emptied.
  Measured: under -z norelro GNU ld puts the read-only copy in `.data.rel.ro`,
  lld `.bss.rel.ro`, mold `.copyrel` (all writable); before the fix all three
  got P-D11. Test `tests/dyn.rs` case `copy-relro`.
- F2: `line.rs` no outside line for static-pie + -E; `export_symbols` read
  (and `--export-dynamic-symbol <v>` no longer taken as an input file);
  `spie.rs` P-D14: with exports on the line a reference's exports are held to
  `.symtab` (value, visibility); mortar: null entry only. `dynverify.rs`:
  W13's exported-IFUNC rule skipped in a static-pie. `anomaly.rs` A34 (GNU ld
  -E static-pie: RELATIVE on absolute-symbol words, x86-64 TPOFF64 records:
  SIGSEGV). Tests `tests/spie.rs` cases `export`, `export-sym`, mutant test.

## Next

- run `cargo test -p mortar_plumb --test dyn --test spie`; fix.
- plumb.md text for P-D11 and §12.4; §10.2 A34; record spec/landed/plumb/WP-088.md.
- census x-norelro line on both lanes; gate.
