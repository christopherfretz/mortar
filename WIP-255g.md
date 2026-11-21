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

# WP-255g2 (branch `wp/255g2-members`, base 22664067 = batch 23 head)

Stopped by the operator's quota pause, 2026-11-19 15:36Z. Everything below is committed; no gate was run.

## Done (commits 7c260ba8, then the test commits up to 2d9e52fa)
- `CHUNKS.members`, VERSION 12:
  - `format::ChunkRow.members: u32` (format 4's row unchanged: writes 72 bytes, reads 0).
  - `state5/mod.rs`: own `Rec for ChunkRow` (WIDTH 76, `members` after `shndx`; `ok()` refuses a count on a non-`Body` chunk). `columns.rs` CHUNKS has `members`. `VERSION` 12 with its doc line.
  - `state5/read.rs` `LayoutOf::chunk_members()`.
- The source (ruling item 3):
  - `mortar_image::layout_plan::member_counts(&Model)` (`is_member` + `member_front`).
  - It is called in `link.rs` `with_image` on THE model from `from_layout`, only when `link.options.keep_leaves` (incremental). It is passed to `go` as a new 5th closure argument (a private fn; the 3 other callers take `_`). `write_link` puts it into a new `Written.members: Option<Vec<u32>>`.
  - `incremental.rs` `state()`: `Link5.members` comes from `written.members`. When that is `None` (the update's hand-made `Written`, `update/mod.rs` ~:524, which has no plan), it counts from a `Model` built from the front `f` with the same function. The "chunk members" lap is timed.
  - `capture.rs`: `Link5.members` (one per chunk or the capture fails) is written into each CHUNKS row.
- The carried count:
  - `mortar_core::incr`: `LayoutRows::chunk_members()` (default `not_given()`), `LayoutDelta.members: Vec<Option<u32>>`, `LayoutDelta::members_of(ChunkId)`.
  - `delta.rs`: `Base.members` is read from the stored layout (not given -> `None`). `Update::members()`, called in `finish`, works per change: -1 for its stored row if that was a member, +1 if it is now placed as a member. Orphans count -1. A `Fate::Debug` change makes its old chunk and every non-alloc chunk `None`. A count below 0 is `state-corrupt`.
  - `update/materialize.rs` `front_delta` (a compile-forced edit in a file I was told to keep away from): maps `members` from CHUNKS rows to the front's chunk ids, so `d_F` carries it.
- Compile-forced one-liners: `members: None` in `update/mod.rs` (the hand-made `Written`), `mortar/src/relocatable.rs`, `mortar_image/src/relocatable.rs`.
- Tests (all pass):
  - `mortar_incr` `state5_columns::chunk_members_survive_the_patch_and_the_compaction`: encode/decode, `LayoutRows`, a PATCH rewriting CHUNKS in place, the compaction, the non-Body refusal, and VERSION 11 refused.
  - `mortar` `state_producers::the_carried_member_count_is_the_full_links`: grow, shrink with 3 killed and 1 revived, and an unchanged file given as changed, on both targets. Carried equals the full link's capture in 32-34 counts; in the shrink, one count moves.
  - `clip_plan.rs` RESIDUE takes its count from the incremental link's STATE. The state's count equals the member index, and the dropped member is refused by it.
  - `mortar_incr` whole suite and `mortar_layout --test delta` (48) pass.

## Done in the continuation (2026-11-21)
- The lane before (22664067) and after (branch, TEMPORARY instrumentation, reverted, tree clean): both 584 PASS, 0 FAIL, tools 0. The `(layout)` split, the count check on 105 S2 hops (0 refused) and the bench are in `spec/landed/incr/WP-255g.md` "Evidence". Logs: `.build/target-wp-145/w255g2/{lanes,bench}.log`, `{before,after}-findings.txt`.

## Next
1. The gate once on the final tree: `.build/target-wp-145/w255g2/gate.sh` (final line `GATE FINAL done`), then the hand-back.
