# WIP: WP-182 (RL-2), the command line and resolution at `-r`

Deleted in the final commit.

## Done
- Measurements in `.build/target-wp-125/rl2/m1` (undefined roots, `-e`
  extraction) and `m2` (`.symver`, `-rpath*`, `.gnu.warning`, input
  build-id).

## Next
1. `grammar.rs`: classes at `-r`, conflicts, `-Ur` sets the kind, the
   seam `Parsed::refusals_landed`.
2. `mortar_cli/tests/relocatable.rs` (+ census replay).
3. `mortar_resolve/src/relocatable.rs` + tests.
4. Spec edits, record `spec/landed/resolve/WP-182.md`, gate.

## Resume
Worktree `mortar/.worktrees/wp-125`, branch `wp/182-r-cli-resolve`,
target `.build/target-wp-125`, scratch `.build/target-wp-125/rl2/`.
