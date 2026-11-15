# WP-032a work in progress (deleted in the final commit)

## Done
- `MORTAR_TIME_STEPS=1`: sub-step times and work counts of phases 20 and 30
  on stderr (`crates/mortar/src/front.rs` `print_steps`;
  `mortar_input::load::load_steps`, `mortar_resolve::Outcome::steps`,
  `StoreStats::wave_ns`).

## Measured, not yet written
- resolve: `symbol_lookup` sort 7 ms serial, comdat sort + binary search
  ~10 ms (1T). Hash index prototype: resolve 50 -> 20 ms (1T).

## Next
- commit the hash index (resolve `tables.rs`).
