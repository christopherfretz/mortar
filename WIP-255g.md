# WIP-255g (branch `wp/255g-members`, base a7d250e1)

## Done
- Step 1, S2-1, option (b): commit 4552368a.
  - `crates/mortar_layout/src/delta.rs` `Update::region_pads`, called by `holes()` per touched chunk. It records the pad `[e, start)` before a touched region. `e` is the end of the last hole at or before the start, valid by invariant I, which the function itself enforces.
  - `crates/mortar_incr/src/capture.rs` `regions_check` accepts a pad `[previous region's end, region start)`.
  - Evidence: `.build/target-wp-145/w255g/t2f-full1.log`. That run made hop 1 a full link via a temporary test edit, which was reverted.

## Not done (next, in order)
1. `CHUNKS.members`, VERSION 12. The design worked out so far:
   - `format::ChunkRow` gets `members: u32`. Format 4's `Row` is unchanged: it writes 72 bytes and reads 0.
   - state5 gets its own `Rec` for `ChunkRow`: WIDTH 76, `members` after `shndx`. Take `ChunkRow` out of `same_as_format4!`. `state5/columns.rs` CHUNKS gets `c("members", 4, V)`.
   - Bump `state5::VERSION` to 12, with a doc line.
   - The count's source is the full link's member rule: `mortar_image::layout_plan::is_member` plus `member_front(..).is_some()`, the same pair `from_layout` uses to make `Input` rows.
     - mortar_incr cannot call mortar_image, so the count needs a `pub fn` in `layout_plan` (e.g. `member_counts(&Model)`).
     - `incremental.rs` `state()` builds a `Model` from `f`, and a new `Link5.members` field takes the result.
     - These are edits outside the brief's files: they need the supervisor's word.
   - `LayoutRows` gets `fn chunk_members()`, with `not_given()` as the default. `LayoutDelta` gets the carried count: +1/-1 per change, a member being size > 0, not NOBITS, placed in an in-file Body chunk. A debug-fate change gives `None` for its chunk.
2. Wire `check_member_counts`' count closure to `LayoutDelta`'s count (the call lives in `update/*`, which is 255e3's), and give the RESIDUE test a state count.
3. Tests, the lane before and after, the bench.
