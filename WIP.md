# WP-238 WIP (step 1: design with experiments)

Step 1 is done and handed back: `spec/landed/incr/WP-238.md` "Step 1". The rulings
R-1..R-7 are awaited. No step-2 code until they come. Experiment logs (small)
are in `.build/target-wp-125/wp238/`; the work dirs and binaries were deleted.
To re-run:
- `tests/incr-design/wp238/l10drive.py PROBE WORK CFG` for (a), (b), (d) on L10;
- `gscen.sh PROBE WORK` for the g scenario;
- `emulate.py OLD.rows NEW.rows`;
- `plaincost.sh` for item 1.
A probe build is `cargo build --release -p mortar --features mortar_live/wp238-probe`, run with `MORTAR_WP238=<dir>`.
