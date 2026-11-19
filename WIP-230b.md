# WIP WP-230 G-3b (branch wp/230-g3b, base cda9d7ec = batch 22)

Brief: `briefs/wp-230-g3b.md`. Scratch: `.build/target-wp-154/g3b/`
(`lane.sh <log> <args>` sets MORTAR_UPDATE_FINDINGS=<log>.findings;
`tests.sh <log> <cargo test args>`; `s1.sh` = step 1 + `tools/dbg.py`).

## Done
- Step 1: `g_gc_same_size_body_edit` on batch 22: 12 of 12 hops TAKEN
  (gcc/kiln x a64/x64), each `G-3 debug: judged` (4-5 contributions in
  place, 10 strings, aranges 160 B gcc / 0 B kiln, 0 group copies);
  debuggers 18/18 equal (gdb 12, lldb 6 on a64). The test
  `gc_identity_edit_with_debug_falls_back_until_g1_runs` became
  `gc_identity_edit_with_debug_is_taken` (identity_case under gc).
- Hooks (testhooks only): `MORTAR_G3_INJECT=in50|in56|in54|r5a`
  (`debug_injected`, `debug_injected_plan` in mod.rs), and
  `MORTAR_G3_STR_LIMIT` (`str_limit` in mortar_incr update/debug.rs).
- R-5 a's silent `continue`s in `debug_judged` are FINDINGs now.

## Next
- Run `update_debug_join` with `--features testhooks` (t1.log); fix.
- Steps 5-7 (see brief), the record section "G-3b", the gate.
