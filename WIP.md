# WP-032a work in progress (deleted in the final commit)

## Done
- `MORTAR_TIME_STEPS=1`: sub-step times and work counts of phases 20 and 30
  on stderr (`crates/mortar/src/front.rs` `print_steps`;
  `mortar_input::load::load_steps`, `mortar_resolve::Outcome::steps`,
  `StoreStats::wave_ns`).

- F1 resolve hash index (`mortar_resolve/src/index.rs`): protoc resolve
  35.7 -> 17.7 ms (1T), 22.1 -> 9.0 ms (8T), golden identical.

## Next
- per_file remainder; input interning (estimate pass, dyn iterators);
  staged read wave B/C.
