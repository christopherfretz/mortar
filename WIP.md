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
- `link` module (`link::write`, `structure`, `LinkTables`); headers from Placement (I-40);
  `.relro_padding` (layout's NOBITS chunk) has a header, no plan.
- non-alloc values: merged/regular non-alloc chunks give section offsets (`.debug_str`).
- `tests/support/front.rs` (main's phases to `place`, no range extension) + `tests/link.rs`:
  static AArch64 program runs (hello, 42); readelf/llvm-readelf/eu-elflint ok (elflint's
  `.relro_padding` name finding filtered); staged == resident at 1/2/4/8 threads with a
  `-g` big.o; changed input at recheck -> input-changed, no output.

## Next
- merge main (2nd) -> mortar_plumb on the written image; GNU ld comparison (headers,
  .symtab multiset, frames).
- unit tests: structure, split_target, S-53; timing tables; spec/image.md; landed record.
