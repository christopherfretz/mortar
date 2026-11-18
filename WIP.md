# WP-238 WIP (step 1: design with experiments)

Record: `spec/landed/incr/WP-238.md` "Step 1" (committed; the "running:"
marks are amended when the scratch's `chain1.sh` ends).
Running: `.build/target-wp-125/wp238/chain1.sh` > `chain1.log`, in this order:
- the L10 drives `l10-<cfg>.log`, one per configuration;
- `si.log` (static-identical);
- the cost logs: `cost-default.log`, `cost-t1.log`, `cost-large.log`;
- `lane.log`.
Its last line is "FINAL chain1".
Then:
- fill in the medians, the AGREE counts and the skip-rule frequency;
- delete `mortar-base`, `mortar-probe` and `mortar-probe2/3` after the timings, and the `l10-*` work dirs;
- remove `/tmp/mortar-wp238-bWI1Ed`;
- hand back.
Scripts: `tests/incr-design/wp238/` (`l10drive.py`, `emulate.py`, `gscen.sh`, `plaincost.sh`).
