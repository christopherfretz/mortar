# WP-027b WIP

Branch `wp/027b-image-joins`. Merged main once to ec564dd (the free one; two left).
Delete this file in the final commit.

## Done
- `mortar_core::records::{RecIndex, Order}` (+ `RecIndex::new`), re-exported by `mortar_scan`.
- plan/reloc: `RecRows::{Decoded, Rela}`; records read at fill time from the input's
  SHT_RELA table; order + `leads` from `RecIndex`; site snapshot from the window buffer.
- synth: mortar_synth's `SynthChunk`/`FillCx` (`&(dyn SynthChunk + Sync)`).
- merged chunks by `Order::frags_in` (`merged.rs`); `.eh_frame` relocations S-53 (`eh.rs`).
- all existing tests + bench updated, green.

## Next
- `link` module: the driver's entry point over the front's tables (FillTables over
  `Addresses`, SlotValues/SymAddrs/code_addr, chunk list from SynthPlans).
- headers from Placement (I-40); from_layout test on real inputs; Store::recheck.
