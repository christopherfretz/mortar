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

- Item 4 rows: -z now/lazy (layout rank_with; pipeline.rs
  z_now_puts_the_got_plt_in_relro), push/pop-state, --dependency-file
  (crates/mortar/src/depfile.rs), --compress-debug-sections partial, 17
  noops; --warn-common stays refused. options.md blessed; cli.md,
  layout.md L-60, input.md §10.4 amended.

## Next
- Histogram before/after: scratch bin/hist.py -> hist/{before,after}.txt.
- Item 3 .comment, item 5 BTI, landed record, gate, full linktest.
