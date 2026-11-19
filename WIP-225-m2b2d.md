# WIP WP-225 M2b-2d (branch `wp/225-m2b2d`, worktree wp-140)

Brief: `/Volumes/AgentWork/mortar-helper/briefs/wp-225-m2b2d.md`.
Scratch: `/Volumes/AgentWork/mortar-helper/.build/target-wp-140/m2b2d/`
(`lane2.sh <scenarios> <label>`, `check.sh <label> <cmd...>`).

## Done
- gc-3 diagnosed from the bytes: the DRY RUN (`mortar_incr/src/delta.rs`,
  holes loop) makes holes only for placed sections; an unchanged file's
  `.eh_frame` contribution whose records all die is not placed, so no
  hole. The oracle is right (IN-63a, IN-27). Fix NOT committed (not my
  crate): `m2b2d/gc3-dryrun-hole.patch`; with it both parked scenarios
  pass on both targets (lane-gc1.log). Scenarios kept in
  `m2b2d/gc3-scenarios/` (not in the lane).
- The update's side of gc-3: `placement` refuses `(layout)` by name when
  `live` draws in an unchanged file (its layout inputs are not produced).
- gc-2: `(symtab)` instead of `state-corrupt`; `expect-full` takes
  `a|b` details (judge.py, selftest, README).
- One live walk: `live_judged` returns the delta, `placement` takes it.
- `layout_in`: `layout_inputs_live` (gc via `LiveDelta`), `layout_inputs`
  keeps its signature (no liveness: `(live)` under gc).
- Tests: `WithRegions` == stored `REGIONS`; Beside end-follows test.

- gc-3's `mortar_incr` fix + the two scenarios: branch
  `wp/225-m2b2d-gc3` (one commit on this branch), green there.
- Fall-back table and record section "M2b-2d" written (before = b21 findings,
  equal bucket for bucket to the base tree run here). e6b722b3
  cherry-picked (supervisor: state_producers test fix).

## Gate (5d8e19d0, once)
quick ok, linktest 0 FAIL, census 0 fail, fmt, clippy workspace, static
0 differ, incr-update 472/472 PASS: all green. clippy testhooks: RED on
`incremental.rs:632/648` `similar_names` (M2b-3 Part 2's, not mine).

## Next
- Handed back; waiting on rulings (gc-3 branch, update_image.rs edit,
  expect-full a|b, `layout_inputs_live`).
