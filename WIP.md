# WIP: WP-228 (G-1), debug information in an incremental update

Step 1 (design with experiments) DONE and handed back; step 2 waits for the
supervisor's ruling on the record `spec/landed/incr/WP-228.md` "Step 1"
(rulings R-1..R-10 in its §0).

Done (step 1):
- item 1's survey: `tests/incr-design/wp228/survey.sh`, `elfdbg.py`.
- `place.py` (hand-made neutralization of a dead file's debug contributions,
  incl. `.debug_types` with a type-unit abbreviation table; the edited map
  for plumb), `emulate.sh` (S1-S5 x variants; readelf, llvm-dwarfdump
  --verify, gdb, lldb, xtask plumb), `tally.py` (DIFF classes),
  `measure.py` + `protoc.sh` (item 5). Results in the record §5, §6.1.
- No Rust changed in `crates/`.

Next (step 2, after the ruling): fill `crates/mortar_incr/src/update/debug.rs`
(`debug::place`), `crates/mortar_image/src/neutral.rs`, tests in
`crates/mortar_incr/tests/update_debug.rs`, per the rulings; seam diffs in
the record §3.3 need the supervisor's word first.

Resume: scratch `/Volumes/AgentWork/mortar-helper/.build/target-wp-154/wp228`
(logs only now; rerun `emulate.sh OUTDIR CONFIG...` to regenerate outputs).
Run scripts in the background with a load wait (< 12).
