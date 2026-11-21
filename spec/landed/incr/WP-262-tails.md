# WP-262 (layout tails): `Why::Packed`, the AArch64 island tail, P3-1

The record of the package (brief `briefs/wp-262.md`, branch
`wp/262-layout-tails`), written by its agent. `WP-262.md` in this
directory is another package's record (the `SYMTAB_KEYS` producer), so
this one has its own name.

The bytes behind every diagnosis are in `WIP-262.md` (worktree root) and in
the scratch `/Volumes/AgentWork/mortar-helper/.build/target-wp-141/w262/`.

## C: P3-1, a growth step past a `PT_LOAD`'s last chunk (BUILT)
- **Cause** (bytes: `s4_frag_ordinals` gcc x86-64, hop 2): `.rodata` (a
  merged chunk, `capacity .rodata 0 m 400120 14 14`) ends the read-only
  `PT_LOAD`; the revived fragment grows it to 0x1c, past its capacity; the
  growth step gave it `need + max(4 KiB, size/16)` of slack (capacity
  0x101c), and `derive_segments` ended the segment's memory at the
  capacity end, 0x40113c, in no section (L-9, the verifier's S18).
- **Fix** (`mortar_layout/src/delta.rs`, `Update::growth`): a chunk with no
  follower in its `PT_LOAD` that takes memory (`Chunk::takes_memory`) gets a
  step of exactly `need`: its capacity is its size, as L-87 gives the full
  link's last chunk. A step that `A` would round past `need` (an empty
  chunk of larger alignment after it, the RELRO page) is
  `update-not-implemented (layout)` (`last_chunk_refused`).
- `derive_segments` is unchanged: it ends a `PT_LOAD` at its last chunk's
  capacity end, which is a section's end wherever L-87 holds, and the
  growth step now keeps L-87. A plain link's segments cannot change
  (`static-identical`, below).
- `mortar_incr`'s `materialize::segment_ends` guard (not this package's)
  stays as a check; on the lane it no longer fires.
- Tests: `mortar_layout/tests/delta.rs`
  `growth_step_of_a_pt_loads_last_chunk_keeps_l9` (x86-64 and AArch64: two
  exact steps, the segment's memory and file ends at `.rodata`'s end, the
  next `PT_LOAD` unmoved; then `reserve-exhausted ... segment reserve`;
  fails with the old step). End to end: the lane's `s4_frag_ordinals` and
  `w6_string_dedup_into_changed` hop 2 (gcc, a64 and x64) are TAKEN, with
  verify, equiv and run passing.

## A: `Why::Packed` (NOT built: a datum the state does not hold)
All 16 lane fall-backs are allocated packed sections in two scenarios
(`start_stop_registry_grows_then_shrinks` hops 1-3: a `reg` section of
`a.o` appears or dies before `b.o`'s; `init_array_grows_then_shrinks` hop
2: `a.o`'s `.init_array` dies, with `b.o`'s and libc's after it). Each
needs item 4's whole rewrite to move an unchanged file's member. The
state gives per chunk only the member COUNT (`CHUNKS.members`); the members
themselves are found only through every file's `SECTIONS` range, a whole
read (IN-18c). **The missing datum**: per packed chunk, its members'
`SectionId`s in offset order (the rows then by `LayoutRows::section`), plus
each member file's load rank (`FILES.rank`) for the full-link order
(`.init_array.N`'s priority is in the row's name). Owner `mortar_incr`
(schema, capture; a VERSION bump). Test pinning the refusal:
`packed_section_appears_or_dies_is_refused_by_name` (both targets, both
shapes, `reg` and `.init_array`).

No non-alloc non-debug section reached `Why::Packed` in the lane
(`.comment` is item 6's, `.note.GNU-stack` and the AArch64 attributes are
ignored). The full link concatenates such a section in input order, so the
same member list would let it be replayed.

## B: the AArch64 island tail (NOT built: needs a new rule)
- **Bytes** (protoc `cxx-medium-static`, `main.cc.o` +0xc and +0x18):
  `main.cc.o`'s `.text` region is `[0x668660, 0x669608)` (capacity 0xfa8) in
  the middle of `.text`'s body chunk; `.text` (0xc94) is followed by eight of
  its own COMDAT functions up to 0x669448; slack 0x1c0. The grown `.text`
  (0xca0) cannot grow in place and does not fit the slack, so IN-72 item 2
  sends it to phase 3, `.text`'s tail, which is the island chunk
  (`capacity .text 1 p0 b28da0 18 4c2ea`: three thunks, redirected from
  files 203, 281 and 738).
- **Contradicts the brief's reading**: the region is not `.text`'s last
  and keeps its IN-30 slack; the island is not empty. The lane's six
  AArch64 island refusals (`grow_one_function` hop 1, the two
  `growth_step_*` hop 1; gcc, kiln) are the same overflow with an empty
  island.
- **No fix within IN-72's text**: phase 3 is right, and the tail is where
  the full link put it (F-3, accepted as v1). The refusal and its tests
  (`update_debug_join.rs` `growth_step_with_debug_is_debug_growth`,
  `gc_grow_shrink_and_new_relocation_with_debug_are_taken` hop 2) stand,
  unflipped.

## Proposed (for the supervisor; none is in `spec/*.md`)
1. **IN-72 item 3** (C, built), after the step's formula: "For the last
   chunk of its `PT_LOAD` that takes memory, which L-87 gives no slack, the
   step is `need`: the chunk's capacity stays its size, so the segment's
   memory ends at a section's end (L-9). When `A` would round that step past
   `need`, the update is `update-not-implemented (layout)`."
   This replaces Part 3's proposed L-9 sentence ("a delta that would is
   refused"): the grow is placed, not refused.
2. **B, preferred: IN-33 / layout.md L-87** (the full link): "On a target
   with patch islands, an output section's tail reserve (IN-33) is the
   capacity of its last BODY chunk, before the island chunk that follows
   it; the island chunk's capacity is its own reserve, `size + max(64 B,
   size/8)` [value to rule]." Then a `.text` overflow opens its region in
   the body chunk's tail, before the island, and no island ever moves (IN-72
   item 9 stands). Cost: the incremental-mode full link's layout on AArch64
   changes (every island after a tail moves by it; a plain link has no
   slack and is unchanged); the branch-reach part limit (`chunk.rs`
   `plan_parts`' `limit`) must count the tail between a part and its island
   (a design point: the island must stay in reach of the part's first
   site). No state schema change.
3. **B, the narrower alternative: IN-72 item 9**: "An overflow region at
   the tail of an output section whose last chunk is an island holding no
   patch (no `PATCHES` row) opens at the end of the body chunk before it;
   the empty island moves to the new region's end, inside its old range, and
   its capacity shrinks by the move." It covers the lane's six hops, NOT
   protoc (its island holds three thunks), and it amends item 9's "fixed
   capacity".

## Measurements
- Lane at e9ff4163 and at 32228732: 47 scenarios, 604 hops, 604 PASS, 0
  FAIL both. Taken hops 247 -> 251 (the four P3-1 hops), none lost.
- The plain link's cost and the gate: not run by this package (hand-back).
