# WP-056 work in progress (deleted in the final commit)

Branch `wp/056-opts`, from main 74feb7f. Record: `spec/landed/dyn/WP-056.md`.

Done:
- Census before (main's mortar): 289 real 64-bit lines; table in the record.
- D-25 writable place = OUTPUT section's SHF_WRITE (mortar_scan driver.rs,
  weak.rs; mortar_image link.rs `section_writable`); test
  `tests/link/dyn/reloc/writable-place-output.test`.
- `--export-dynamic-symbol` globs (were matched as literal names: a silent
  non-export): `mortar_resolve` options.rs `glob_match`, tables.rs
  `shared_link`; test `tests/link/dyn/dynsym/export-dynamic-symbol.test`.
- static-pie `--export-dynamic`: measured useless; the refusal stays (the
  verifier keeps the line outside stage 3); finding.
- `.got` slots of a static-pie (X-19): finding, no change.

Next:
- `cargo xtask census` (xtask/src/census.rs): build, run, measure; quick
  subset into `ci --quick` if under a minute.
- dyn.md §16 audit (full `linktest tests/link/dyn`), §2.3/§2.7 rewrite,
  §13.1 rows, D-25 note, options.md rows, the record's after table.
- Gate, hosted run, hand back.
