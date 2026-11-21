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

## Appendix: the package notes (WIP-262.md at the merge, kept for WP-269/WP-270)

## WIP-262: the delta layout's tails (A `Why::Packed`, B the AArch64 island tail, C P3-1)

Branch `wp/262-layout-tails`, base e9ff4163. Scratch
`/Volumes/AgentWork/mortar-helper/.build/target-wp-141/w262/` (logs, the
probe output, the repro scripts `repro-b.sh`, `repro-c.sh`).

### Item 1: the measurement (temporary probe, never committed)
The probe (`w262/probe+c.diff`, the delta.rs hunks with `wp262_probe`) wrote
one line per `Why::Packed` refusal, per killed fixed section, per island-tail
refusal and per `not_implemented()` caller, over the whole lane at e9ff4163
(`w262/probe-before.txt`, `w262/lane-before.log`: 47 scenarios, 604 hops,
604 PASS, 0 FAIL; 247 hops `reason=update`).

`(layout)` from `delta.rs`, by cause (23, as 255g2 counted):

| cause | hops | where |
|---|---:|---|
| `Why::Packed` (delta.rs `Why::reason`) | 16 | below |
| `rela_plt_refused` (`delta/slots.rs`:193) | 4 | `ifunc_added_moves_rela_iplt_end` hop 2, gcc/kiln x a64/x64 |
| invariant I (`region_pads`, first site) | 1 | `grow_one_function.gcc-x64` hop 1 |
| (the 2 `layout_in` lines are not `delta.rs`'s) | | |

#### A: the 16 `Why::Packed` (all ALLOCATED; no non-alloc section occurs)
| scenario | hops | targets x compilers | section | L-142 class | old -> new |
|---|---|---|---|---|---|
| `start_stop_registry_grows_then_shrinks` | 1, 3 | a64, x64 x gcc, kiln (8) | `reg` (PROGBITS, `SHF_ALLOC`, align 4) | `__start_`/`__stop_` section | none -> 4 (a section new to `a.o`; `old=None`, so no keep) |
| `start_stop_registry_grows_then_shrinks` | 2 | same (4) | `reg` | same | 4 -> killed (`place (chunk 27/28/29, 0)`) |
| `init_array_grows_then_shrinks` | 2 | same (4) | `.init_array` (INIT_ARRAY, WA) | `.init_array` | 8 -> killed (a64 `(chunk 26/27, 24)`, x64 `(25/26, 8)`) |

(`init_array` hops 1 and 3 are `update-not-implemented (scan)`, before the
layout: not this package's.)

Bytes (gcc-a64 hop 1's full link, `prog.map`): `.init_array` chunk at
0x4ddad0 size 0x30: libc's members (files 27, 34) at +0, +8; crtbeginT
(file 2) +0x10; `a.o` (file 4) +0x18; `b.o` (file 5) +0x20; libc (170)
+0x28. Killing `a.o`'s moves `b.o`'s and libc's down 8. In the registry,
`a.o` precedes `b.o`: its new `reg` at +0 moves `b.o`'s from 0 to 4.

**Diagnosis A**: every one of the 16 needs IN-72 item 4's whole rewrite to
MOVE a member of an UNCHANGED file. The rewrite needs the packed chunk's
member list (which sections of which files are in the chunk, at which
offset, with their order keys). `LayoutInput`/`LayoutRows` give per-file
`SECTIONS` ranges (`sections_of`), one row by id (`section`), and per chunk
only a COUNT (`CHUNKS.members`, WP-255g). Finding the other files' members
needs a whole `SECTIONS` read (IN-18c forbids it). **STOPPED on A; the
missing datum**: per packed chunk, its members' `SectionId`s in offset order
(a chunk -> member range index; the rows themselves then by `section(id)`),
and for the full-link order the file's load rank (`FILES.rank`; the
`.init_array.N` priority is in the row's name). Owner `mortar_incr`
(schema + capture), a VERSION bump.

Non-alloc non-debug sections: none occurred. `.comment` is `SHF_MERGE`
(item 6, `Fate::Pieces`); `.note.GNU-stack`, `SHT_AARCH64_ATTRIBUTES`,
`.gnu.warning*` are `Fate::Ignore`. Any other non-alloc non-debug section
(`.gnu.attributes`, `.stab`, a producer's own note) is `Why::Packed`: the
full link concatenates them in input order (no relocation-free rule needs
more than the member list), so the same datum would let it be replayed.

#### B: the AArch64 grow on protoc (repro `w262/repro-b.sh`)
protoc `cxx-medium-static`, `main.cc.o` recompiled from `main.cc` with one
block after `CommandLineInterface cli;` (base `.text` 0xc94, +0xc: 0xca0,
+0x18: 0xcac). `--incremental` full link, then `--incremental=update`:
both grows `cannot update: reserve-exhausted: .text: its tail is in a patch
island chunk`. The bytes (`b-protoc/map.full`, `probe-g4.txt`):

- `main.cc.o` (map file 3, `FileId` 4): one `.text` region, category 6,
  `region 3 .text 6 668660 fa8` = [0x668660, 0x669608), in body chunk 14
  (0x666080, size 0x4c2d20, capacity = size). It is NOT `.text`'s last
  region (offset 0x25e0 of 0x4c2d20).
- Its members: `.text` (index 8) at 0x668660 size 0xc94 (to 0x6692f4), then
  its COMDAT functions 13, 15, 17, 19, 21, 23, 25, 28 contiguous from
  0x6692f4 to 0x669448. Slack: 0x669608 - 0x669448 = **0x1c0** (448 bytes).
- The grown `.text` (0xca0) cannot grow in place (section 13 starts at its
  old end) and does not fit the slack (0x1c0 < 0xca0): IN-72 item 2
  sends it to phase 3, `.text`'s tail.
- `.text`'s tail is the island chunk 15 (`capacity .text 1 p0 b28da0 18
  4c2ea`): size **0x18, three thunks** (`redirect 203 243 208 b28da0`,
  `redirect 281 1 1e4 b28da8`, `redirect 738 1 904 b28db0`), capacity
  0x4c2ea (the output section's tail, `chunk.rs` `capacities`).

**Diagnosis B, contradicting the brief's reading**: the region is not
`.text`'s last and it has its IN-30 slack (448 bytes); the grow is a real
overflow (a section bigger than its region's slack, followed by its own
file's COMDAT code), and on AArch64 every `.text` overflow meets the
island tail (F-3). The island is NOT empty on protoc: it holds three
thunks reached by kept references in files 203, 281, 738, so "open the
overflow before an empty island" does not apply here. The lane's six
AArch64 island refusals (`grow_one_function` hop 1, the two
`growth_step_*` hop 1, gcc and kiln) are the same shape with an EMPTY
island (size 0, capacity 0x65c4) and grows of 60 -> 176 bytes (region
0x7c) or 24 -> 0x18018.

No fix within IN-72's text: the tail is where the full link put it. Both
fixes need a new rule (item 9 fixes the island's capacity; F-3 accepted the
refusal), so per the brief B is NOT built. Proposed sentences: see the
record (`spec/landed/incr/WP-262-tails.md`, "Proposed").

#### C: P3-1 (repro `w262/repro-c.sh s4_frag_ordinals.gcc-x64 elf_x86_64`)
Hop 1 is taken; hop 2 `materialize ... segments: the delta's PT_LOAD 0
(addr 0x400000 memsz 0x113c) would end at 0x40113c, where no chunk's size
ends (L-9, S18) ... (P3-1)`. After hop 1: `capacity .rodata 0 m 400120 14
14`, R `PT_LOAD` filesz = memsz = 0x134 (the section's end). Hop 2 appends
the revived fragment (size 0x14 -> 0x1c): past the capacity, so a growth
step `round_up(need + max(4 KiB, size/16), A)` with no follower (A = 1):
capacity 0x14 + 0x1008 = 0x101c, and `derive_segments` ends the memory at
the capacity end, 0x40113c. **Diagnosis C** (as the M2b-3 record said):
the growth step gives slack to a chunk that ends its `PT_LOAD`, which L-87
forbids the full link; `derive_segments` is right for every chunk L-87
holds for.

### Done
- C fixed in the growth step (`delta.rs`): the last chunk of a `PT_LOAD`
  that takes memory gets the step it needs, no slack; a step the following
  chunks' alignment would round past need is refused `(layout)` by name
  (`last_chunk_refused`). `derive_segments` unchanged (a plain link's
  segments cannot change). Test `growth_step_of_a_pt_loads_last_chunk_keeps_l9`
  (both targets; fails without the fix). Lane: `s4_frag_ordinals` and
  `w6_string_dedup_into_changed` hop 2 gcc a64/x64 TAKEN (verify, equiv,
  run pass); update's R `PT_LOAD` 0x13c = `.rodata` end 0x40013c.
- A: negative test `packed_section_appears_or_dies_is_refused_by_name`.

### Lane after (32228732; w262/lane-after.log)
`incr-update: 47 scenarios, 604 hops: 604 PASS, 0 FAIL, ... 198s` (load 11.6
at start). Taken hops (`reason=update`) 247 -> 251, none lost: the four new
are `s4_frag_ordinals` and `w6_string_dedup_into_changed` hop 2, gcc a64 and
x64 (`update-not-implemented` 173 -> 169). `(layout)` from `delta.rs`
unchanged (Packed 16, rela_plt 4, invariant I 1); materialize's P3-1
`segments` refusal 4 -> 0. `reserve-exhausted` 22 -> 22 (B not built).

### Unfinished (hand-back at the supervisor's word, 150K budget)
- NOT run: the full gate (ci --quick, linktest, census, fmt --check,
  workspace clippy, testhooks clippy, static-identical); the supervisor runs
  it at the merge. Only `cargo test -p mortar_layout` (all pass) and the lane.
- NOT run: item 7, the protoc plain-link timing (script ready:
  `w262/bench.sh`, base e9ff4163 vs head, medians of 5; it builds a base
  tree in `w262/base-target`, to delete after). A plain link never calls
  `delta::layout`, the only code changed, but that is argued, not measured.
- A: stopped on the datum (member list per packed chunk + FILES.rank).
- B: proposals only (record "Proposed" 2, 3); `update_debug_join.rs`
  refusal expectations unchanged.
- C end-to-end test in `crates/mortar/tests/update_*.rs`: not written (the
  lane's four hops are the end-to-end evidence). Lane scenario proposal: a
  `.bss`-last growth step in the RW `PT_LOAD` (a64, x64), the other
  last-chunk shape C covers and no scenario exercises.
