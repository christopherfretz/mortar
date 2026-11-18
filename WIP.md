# WP-238 WIP (step 1: design with experiments)

Done: the enumerator refactor in `crates/mortar_live/src/gc.rs` (`Walk::edges`,
`To`, `Via`, `record_to`); the probe `gc/probe.rs` behind feature
`wp238-probe` (experiment code, deleted in step 2); scripts in
`tests/incr-design/wp238/` (plaincost.sh, l10drive.py, emulate.py, copies of
livemodel.py/oracle.py from `.build/review-incr`).
Next: run the experiments (scratch `.build/target-wp-125/wp238/`), write
`spec/landed/incr/WP-238.md` "Step 1", hand back, wait for the ruling.
Resume: `.build/target-wp-125/wp238/build.sh [probe]` builds mortar-new /
mortar-probe; `mortar-base` is ffdda274's binary (delete after timings).
