# WIP: WP-064a (deleted in the final commit)
- done: format 4 (FILES flags byte 2, FRAG_AT -> FRAGS row), positional_set, Translation,
  recapture, contribution::compare guard, floor m3/map/recapture, spec IN-18a4 + §7 rows;
  update_by; crates/mortar/tests/incr_review.rs (first run: s1, s1x, s2, s2c, W1, W1b,
  t-stamp, v3, refusals pass).
- FOUND (report at top): s3 hop 0 gives a WRONG PROGRAM in BOTH translations at the base
  commit 208581e8 (exit 14, want 6): GAS keys :got: relocs of locals by section symbols,
  whose §7.1 identity (empty name, ordinal) is a position; the GOT is not refilled. Also
  s4 hop 0: the judge finds b.o's reloc stale (Y4/R1) in both translations (ordinals).
  Guards added: resolution-changed (slot/thunk keyed by an ambiguous local of the changed
  file), layout-changed (a copy of an ordinal identity added/dropped).
- open: H2 (by position a third program?) and fuzz-21; replay failures now named
  layout-changed / state-corrupt.
- next: WP-094 record §3 table, WP-064a record, gate/lane/census, item 8 numbers.
