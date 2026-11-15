# WIP — WP-050 (verifier stage 3)

## Done
- Step 0d: map dynamic records (plumb.md §3.2): parser/writer in
  `crates/mortar_plumb/src/map/{mod,mortar}.rs`, writer side
  `crates/mortar_image/src/map.rs` (`DynRecords`), cross test
  `crates/mortar/tests/image.rs map_dynamic_records_read_by_the_verifier`.

## Next
- Stage 3 modules in mortar_plumb: shared-object reader, command-line model,
  dynamic image reader, D-11/D-12/D-19a/D-21 recomputation, P-D checks,
  loader model (P-D9).
