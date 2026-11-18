# WP-222 (V-1) work in progress (deleted in the final commit)

Brief: /Volumes/AgentWork/mortar-helper/briefs/wp-222.md. Scratch:
/Volumes/AgentWork/mortar-helper/.build/target-wp-140/wp222/ (c.sh runs a
command in the worktree's env; xtask-base = the base's xtask for timing).

## Done
- L-1 bytes: gen200 clang-a64-O0-g base pair (l10 --keep, seq/replay.gen200...
  /s0 x, full). The 933 findings are `$d` in NON-ALLOC debug sections
  (.debug_abbrev/info/line/rnglists/str_offsets/addr), not .eh_frame (whose
  locals P-E3 already passes over). X's debug sections carry IN-51 slack
  (dead neutral lines), so offsets differ; the identity multisets
  (section, unit, input offset) of the 1608 debug `$d` are EQUAL on both
  sides. Verifier: equiv.rs locals use only allocated spans, so a
  non-alloc value falls back to equality. Outcome (ii).
- L-2 bytes: corpus40 clang-x64 seed2 step 6 (seq/replay.corpus40.../s2):
  F has `.pad.relro` header of size 0 at 0x535000 (no `chunk` record: I-78
  writes one only for size > 0); X's is 0x428. L-64: the chunk exists with
  size 0. Outcome (i): P-E1 segment lists and P-E2 chunk sets leave
  `.pad.relro` out.

## Next
1. map reader: common <file>, decision records, order, writer, unit tests.
2. src/symfill.rs: fillers, slack holes, dead .strtab names, regions.
3. equiv.rs: fillers / near-fillers, non-alloc local identities, pad, common file.
4. coverage lines; spec; record; fixtures tests/symfill (gen.sh + edit script).
5. Gate script (background), timing base vs new.

## Open (for the supervisor)
- Today's incremental-mode maps write `common` with 4 fields; the text
  requires 5. Transitional: required once the map has decision records (U-6).
