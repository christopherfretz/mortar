# WIP: WP-215 (U-3), branch wp/215-incr-resolve

## Done
- Seams 1-3 accepted (seam 3 = 61f26084, cherry-picked as bf4bb7d7).
- extract, recompute, refill_set (7ed417fc); `resolve` written (resolve.rs),
  roots split `foo@V` (extract.rs `split_version`).
- Test model extended (COMDAT sections, values, full link, join loop with
  DrawIn, `same_as_full`); `resolve_random_series` (condition 1(b)).

## Next
1. Make tests pass; named tests (see coordinator's list in the summary of
   briefs/wp-215-rulings*.md and spec/incr.md IN-71/71b/71c test lists).
2. `feedback_check_is_sound_random` in crates/mortar/tests/,
   `affected_key_feeds_back_is_fallback`.
3. `--start-lib` refusal in crates/mortar/src/incremental.rs + test.
4. Record spec/landed/incr/WP-215.md; gate; delete this file.

## How to resume
Scratch `.build/target-wp-143/wp215/` (chk.sh check, t.sh tests).
