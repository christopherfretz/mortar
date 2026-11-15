# WP-027e work in progress

## Done
- Item 1 experiment (scratch harness, results to go in
  `spec/landed/pipeline/WP-027e.md`): references write 0 (lld 19/22,
  mold; GNU ld redirects to the kept copy) in every debug section but
  `.debug_ranges`/`.debug_loc` (1). All ones wraps in DWARF 5
  `DW_RLE_start_length` (GCC) and `startx_length` through `.debug_addr`
  (Clang): gdb 16 "inverted range" on every DWARF 5 C++ config.
- Item 1 fix: `.debug_rnglists`, `.debug_loclists`, `.debug_addr` -> 0
  (`mortar_arch/src/dead.rs`), spec image.md I-47 / §15, arch.md §3.11,
  xfail removed in `crates/mortar/tests/debuggers.rs`.
- plumb expected table (`mortar_plumb/src/debug.rs`, plumb.md §5.8 row).

## Next
- Byte-identity proof on protoc (static line in scratch).
- Item 2 (-Map text), then 4, 3, 5.
