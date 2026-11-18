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
- Part 0 handed back (interim, record S1-S7 at 358850da+); WAITING for the coordinator's word. The probe binary was deleted: rebuild with scratch `build.sh probe 2`.
- Then Part 1 (seam 9, ONE commit, the diff of S7), Part 2, 3, 4.

## How to resume
- Probe build: scratch `build.sh probe 2` (release mortar with the feature,
  copied to `.build/target-wp-125/wp238/mortar-probe2`).
- Scenarios: `python3 -I tests/incr-design/wp238/review_copies.py OUT`,
  then `bash OUT/{roots,fdeglobal,skip,shapes,newscen}.sh PROBE W` (W fresh).
- Lane: scratch `chain4.sh`; L10: scratch `chain3.sh` (second half).
