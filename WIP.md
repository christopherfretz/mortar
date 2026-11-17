# WP-148 work in progress (deleted in the final commit)

Measurement scripts and outputs (not committed):
`/Volumes/AgentWork/mortar-helper/.build/target-wp-145/wp148/` (`p1.sh`,
`elfq.py`, `p1-all.txt`; `p2/` for part 2).

## Part 1 (S-18 under -pie)
- Cause: a section symbol + addend outside a split Merge section (GAS
  turns `.dc.a .Lend` with addend 0 into `sec + sh_size`) is S-18's
  refusal in fill, but `.rela.dyn`'s planner turned it into `None` ->
  internal error. Fixed: `dynrel::Unvalued`/`RecordError`, rela_dyn
  collects the refusals at their places (fill's key); fill adds the
  referring place; `LinkTables::outside_merge` names the target.
- Next: spec sentences (S-18, P-1 .comment), link test
  `tests/link/split/merge-end-*.test` or `resolve/place-merge-end.test`.

## Part 2 (-E exports nothing)
- Cause: `mortar_resolve::resolve` runs `shared::link` (which computes
  `exported`) only with a shared input or a shared output.
- Next: run it also for a dynamic executable with -E or
  --export-dynamic-symbol; tests; static-identical rows.
