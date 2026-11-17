# WIP — WP-134 (kiln as a producer of .mason.meta)

- done: O-35 as amended in `mortar_odr` (records.rs `kind5`, `kind5_one`,
  `c_linkage`); tests `crates/mortar/tests/odr_records.rs`.
- done: producers `kiln`/`kiln-hod` (engine.rs `compile_kiln`, `meta_levels`,
  `meta_level`), `Family::Kiln` / `Feature::Kiln` (`requires: kiln`, probe
  `mason::KilnRecords::find`), producers.tsv 11 columns, classified
  (script /tmp/mortar-wp134-1/scripts/classify.py; texts in the tsv).
- learned: kiln = kiln-hod on all 570; kiln = gcc 478; 10 tests differ, all
  misses (no kind 2, no enum rows, O-17 pointee) or synthetic rows; no new
  finding from kiln's rows. Read-back: 48 units clean; MT-26 flags conform.
- next: link tests tests/link/odr/kiln-*.test; legal corpus (mason programs
  with -fmason-metadata, --odr=strict); record spec/landed/meta/WP-134.md;
  gates.
