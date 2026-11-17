# WIP WP-174 (kiln K-3c kind 2 rows meet mortar)
## Done
- read briefs, WP-134/151 records, kiln codegen.md kind 2, abi.md Definition hash
## Learned
- kiln K-3c writes kind 2 rows anchored only (name 0; named rows are K-3f): a dropped definition's row is unanchored and never compared, even under strict (records.rs entries(): only INLINED_ONLY rows; kiln never sets it)
- kind 2 runs only under strict (O-47)
- producers --update run (= supervisor's probe table; notes kept); notes not yet rewritten
- kiln-k4-shapes expectation (width), kiln-define-same-layout header
- fix: info_of layout_scheme fallback only with kind 3 rows (records.rs ~577) + odr_records layout_schemes test
- fix: kind 2 note "written in different files" (records.rs kind2) + test kind2_note_names_different_files, odr.md §9.1
- new tests kiln-k2-differ/silent/equal/schemes: PASS both targets
## Next
- producers table: investigate 62 differing links (o24-output, o50-full-input, c-enum-compatible, k4-pointer-only, k7-old-rules, mixed-std-system-row, records-legal)
