# WP-080 work in progress (deleted in the final commit)

Stage A (split DWARF tested). Stage B (index question) waits for the
supervisor.

## Done
- `scripts/split-census.py`: the census (producers x forms x arches x
  linkers, plain and --gc-sections): object sections, fates, runs,
  verifier, address/tombstone counts. First run: every cell links, runs
  as GNU ld's, verifier clean (200 mortar outputs); mortar's section fate
  and size equal at least one reference's everywhere.

## Half-done
- census re-run with tombstone counts and the single-form/type-unit forms.

## Next
- record `spec/landed/image/WP-080.md` (census table, item 2 table).
- `tests/link/split/*.test` (gcc and clang 22; static, PIE, static-pie).
- debugger sessions: split flag sets in `crates/mortar/tests/debuggers.rs`,
  `.dwp` row with llvm-dwp (GNU dwp is not installed: request).
- static-identical rows: `tests/identical/split-dwarf.test`.
- Mason lane sample with `--cflags -gsplit-dwarf`.
