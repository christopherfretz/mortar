# WP-032b work in progress (deleted in the final commit)

## Done
- `mortar_core::wp032b_steps`: `MORTAR_STEPS=1` prints sub-step lines for
  split (phase.rs), merge (merge.rs), order (order.rs) and scan (lib.rs).

## Measured (protoc static -g, threads=1)
- scan walk 35.6 ns/record (1.78 M records); units() 3.8 ms serial.
- merge intern 32 ns/piece (804 k pieces, 79 k frags); cut+hash 0.25 ns/byte.

## Next
- scan units() from headers (no Sec::open per section).
