# WP-238 WIP (step 2: liveness in an update, after the review)

Brief: `/Volumes/AgentWork/mortar-helper/briefs/wp-238-step2.md`. Rulings:
`briefs/wp-238-rulings.md` ("After the review" wins).

## Done
- Part 0 in progress: record section "Step 2: the design after the review"
  (`spec/landed/incr/WP-238.md`, S1-S7).
- Probe (`wp238-probe` feature, R-7: deleted in Part 4) extended:
  `stored_rows` (the Part-0 producer prototype) and `stored_dump` (dump
  lines `C`, `A`, `Q`, `W`).
- `tests/incr-design/wp238/oracle_design.py`: the design-faithful oracle
  (single link and update; `WP238_MUT` mutations).
  `review_copies.py OUT` makes rm-free copies of the reviewer's scenarios
  that also run it; `newscen.sh` (F6, scripts, ring + --strip-debug,
  defsym start/stop, -fini/--require-defined).
- `l10drive.py` also runs the oracle (ORA1/ORA2 lines).

## Next
- Part 0 accepted (rulings "After Part 0", P0-1..P0-7).
- Part 1 DONE: seam 9 is commit e968996e (the first commit after 7d242c1f;
  differences from S7 listed in the hand-back). Gate script: scratch
  `p1-gate.sh` (logs in scratch `gate/`).
- **WAITING for the coordinator's word before Part 2** (the second review,
  `briefs/review-238-live-2.md`, may amend the stored form).
- Then Part 2 (producer `stored_edges` body + capture of rows and
  `live_hash`), Part 3 (walk in `update/live.rs`), Part 4 (delete probe and
  scripts).

## How to resume
- Probe build: scratch `build.sh probe 2` (release mortar with the feature,
  copied to `.build/target-wp-125/wp238/mortar-probe2`).
- Scenarios: `python3 -I tests/incr-design/wp238/review_copies.py OUT`,
  then `bash OUT/{roots,fdeglobal,skip,shapes,newscen}.sh PROBE W` (W fresh).
- Lane: scratch `chain4.sh`; L10: scratch `chain3.sh` (second half).
