# WP-032i — work in progress (deleted at the end)

Scratch: `/tmp/mortar-wp032i-3693169` (`ident.sh <bin> <tag>`: 78 links;
`bench.py`: interleaved runs). Base binary = 2297dac; identity
`64c89cf1e44a38d7` (combined hash printed by ident.sh); i1, i1b, i2 equal.

## Done
- Item 1: caeff1a (scan side, `need::branch_candidate`, `Scanned.branches`),
  f88be9f (`LinkSites::new` reads them; old walk kept as a debug assertion).
  protoc layout sites 18.5 -> 4.5 ms (1 t); objects 65 -> 10 ms. Cost: scan
  walk +3 ms protoc; RSS +5 MiB protoc / +21 MiB objects (duplicate
  BranchRec + BranchAt) -> fix in front.rs after WP-041 merges.
- d1a2cab: symtabs from the walker's view (no eager per-file tables).
- Item 2: 38e7b0e `walk_where` (records outside erratum windows skipped
  before classify): erratum needs+fixes 11.0 -> ~8 ms (1 t, protoc).
- Item 4 (validate): measured, sort is memory-bound; packed keys / fused
  slots / radix tried, no gain; reverted, nothing committed.

## Next
- Item 3: PieceIndex::at, table_order re-read.
- Item 5, item 6, batched-apply estimate.
- After WP-041 on main: front.rs (in-place BranchAt, shared View), static-identical.
