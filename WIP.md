# WP-027b WIP

Branch `wp/027b-image-joins`. Merged main once (ec564dd; one of two used).
Delete this file in the final commit.

## Done
- `mortar_core::records::{RecIndex, Order}` (+ `RecIndex::new`), re-exported by `mortar_scan`.

## Next
- plan/reloc: records read at fill time from the store (RelocSection = table ref), order and
  `leads` from `RecIndex`; per-clip site; `target_alloc` real source.
