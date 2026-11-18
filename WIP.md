# WIP: WP-215 (U-3), branch wp/215-incr-resolve

## Done
- First step (the brief's "before code" page): `spec/landed/incr/WP-215.md` §Q.
  Result: an update cannot follow §4.1's iterations (Q.1); the trigger is
  decidable per affected key (Q.2) given D1; eight data the stored tables
  lack (Q.3, D1-D8); IN-71b sentence proposed (Q.4). Stopped there, as the
  brief says, for the supervisor's ruling.
- Release build of the base in `.build/target-wp-143/release/mortar`;
  protoc run with `MORTAR_STEPS=1` (one iteration): scratch
  `.build/target-wp-143/wp215/` (medium.steps, *.rsp, cxx-medium-static.out).

## Next (after the ruling)
1. Apply the ruling to Q.3 (which data come, which cases fall back by name).
2. Code, in this order (each independent of the others' data gaps):
   `defined::recompute` (IN-71c; from `view.defined()` and `LayoutDelta`),
   `refill::refill_set` (IN-74; INDEX by key/section), `live::live`
   (IN-71a; needs D3's roots), `extract::extract` (IN-71b + Q.2),
   `resolve::resolve` (IN-71).
3. The join's loop (for the record): extract and resolve are called again
   after the join reads newly extracted / drawn-in files, until no file flips.
4. Tests in `crates/mortar_incr/tests/update_resolve.rs` (replace the
   "until it lands" tests); named tests: WP-207 §D U-3 row, WP-211 §D.
5. `crates/mortar/src/incremental.rs`: `--start-lib` refusal line + test.

## How to resume
Read the brief (`briefs/u-2-to-9.md` U-3), this file, then the record §Q.
