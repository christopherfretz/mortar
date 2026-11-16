# WP-080 work in progress (deleted in the final commit)

Stage A (split DWARF tested). Stage B (index question) waits for the
supervisor.

## Done
- census (`scripts/split-census.py`), record `spec/landed/image/WP-080.md`,
  rule `spec/image.md` I-49a.
- `tests/link/split/` (5 tests, 48 PASS), `llvm-dwarfdump` check tool.
- debugger sessions in `crates/mortar/tests/debuggers.rs` (split_*), green.
- Mason lane sample (shard 1/46, 202 tests): 0 DIFFERENT, 151 links clean.
- merged main once (1a6152e; WP-059 not on main yet).

## Next
- static-identical --update (aarch64), push, hosted ci, x86_64 rows from
  the lane log, gate, final commit deleting this file.
