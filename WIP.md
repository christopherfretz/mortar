# WIP — WP-134 (kiln as a producer of .mason.meta)

kiln: d854b23bc (from 2026-11-17 01:47; earlier numbers 55b7e0538:
producers.tsv, the 48-unit read-back — to re-measure).

- done: O-35 as amended (records.rs `kind5`, `kind5_one`, `c_linkage`);
  tests in `crates/mortar/tests/odr_records.rs`.
- done: producers `kiln`/`kiln-hod`, `(kiln)` steps, `requires: kiln`
  (probe `mason::KilnRecords::find`, needs enum rows + kind 4 + kind 5);
  `(kiln)` steps keep kiln under every producer; `meta_level` honours
  `level detail`.
- done: merged int/batch-8 (WP-145's kind 4 reader).
- done: link tests tests/link/odr/kiln-*.test (9), PASS on both arches,
  SKIP with the old kiln. `meta dump` prints kind 4 items as comments when
  the typed form does not reproduce kiln's bytes.
- learned: O-34 not implemented (register "cannot run yet"): kiln-mixed
  records the miss. bfd x86-64 static IFUNC address: verifier F2 (GCC
  objects too): kiln-o33-glibc links dynamically.
- next: re-record producers.tsv with d854b23bc (kiln-why for WP-145's 2
  new tests); re-run read-back (48 units + kind 4 counts on kiln's
  bench/frontend/format-like-g.cpp: 368 rows, 31 enums, 906 items);
  legal corpus (mason programs, -fmason-metadata, --odr=strict); record
  spec/landed/meta/WP-134.md; gates x2.
