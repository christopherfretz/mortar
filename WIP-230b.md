# WIP WP-230 G-3b (branch wp/230-g3b, base cda9d7ec = batch 22)

Brief: `briefs/wp-230-g3b.md`. Record: `spec/landed/incr/WP-230.md`
section "G-3b" (G3b-1..6: everything measured is there). Scratch:
`.build/target-wp-154/g3b/` (`lane.sh <log> <args>` sets
MORTAR_UPDATE_FINDINGS=<log>.findings; `tests.sh <log> <cargo test
args>`; `s1.sh` = step 1 + `tools/dbg.py`; `gate.sh` = the full gate).

## Done (commits 75aee832, 595b08c1)
- Steps 1-6 (record G3b-1..G3b-6); FINDING G3b-3 fixed in debug.rs
  (`own_allot`: R-5 a's run tried in place first).
- Hooks (testhooks only): `MORTAR_G3_INJECT=in50|in56|in54|r5a`
  (mod.rs `debug_injected`, `debug_injected_plan`), `MORTAR_G3_STR_LIMIT`
  (debug.rs `str_limit`). The hooks' tests run only with
  `cargo test -p mortar --features testhooks --test update_debug_join`
  (CI's `cargo test --workspace` has no testhooks).

## Gate (step 7, ran once, 12:17-12:57Z): FAILED
- ci --quick: exit 1, only `-p mortar --test update_debug_join`
  (8 pass, 2 fail: `identity_edit_with_debug_is_taken`,
  `kept_group_copy_left_in_place_is_judged`, x86-64 only).
  linktest, census, static-identical, incr-update, fmt, clippy
  (workspace and testhooks): exit 0.
- testhooks update_debug_join: 10 pass, 3 fail (the two above plus
  `gc_identity_edit_with_debug_is_taken`, x86-64 only).
- Cause: a test race in `Prog::gdb`, not a mislink. The qemu port
  is `20000 + pid % 20000 + arch name length`, which is the same for
  concurrent tests in one binary. gdb attached to another test's qemu:
  the r5a transcript reads "Reading symbols from
  ...g3join-id-x86_64.../full". Fix (not applied, the supervisor said
  no new work): a unique port per call, an atomic counter plus a
  per-test offset; or `--test-threads=1`.
- In the gate there were 0 FINDINGs; G-3 debug lines: 36 placed,
  36 judged. Fall-back tables: `fb-before.txt` (b21) and
  `fb-after.txt`. Hop `update` went 47 -> 83; the base differs (b22 adds
  tests), so the comparison is not like-for-like.

## Next (for the supervisor)
- Ruling: refuse `new-output-section` at U2 in `debug_condition`
  (T-1/T-2 assertions change), or keep `(image)`.
- Fix the gdb port race in `Prog::gdb`, then rerun the gate.
- Not taken: the plain-link timing (update-only code by grep).
