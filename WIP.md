# WP-027e work in progress

## Done
- Item 1 (tombstones): on main as 117a614, b18e431. Results for the
  landed record: refs write 0 except ranges/loc (1); protoc byte identity
  = 25012 rnglists fields + build-id; plumb 0 vs 25012 R1.
- Item 2 (-Map text map, GNU 2.44 format): textmap.rs, link.rs
  text_map_of, `--mortar-map` row for the machine map; pipeline test vs
  GNU's map. Cost on protoc (median of 11, interleaved): 1t 0.782 ->
  0.836 s; 8t 0.268 -> 0.320 s; map 14.7 MB / 140682 lines.
  Findings: COMDAT winner differs from GNU (numpunct_cache D0 72 vs 44
  bytes); archive taker differs for 129/381 members (sets equal).
- xtask linktest.rs:632 filter "e/" -> "e/a.test" (scratch path held "e/").

## Half-done
- Item 4 `-z now`: rank.rs rank_with, target.rs Options.now, run.rs,
  chunk.rs, scale.rs (compiles, tests pass). Still to do: rows Supported,
  options.rs check vs GNU, layout.md L-60 row 7 + L-8.

## Next
- Item 4 remaining rows (see brief), options.md, histogram before/after
  (before binary: /tmp/mortar-wp027e-651725/mortar-exp).
- Item 3 .comment, item 5 BTI, landed record, gate, full linktest.
