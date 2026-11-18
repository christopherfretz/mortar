# WIP: WP-255c3 (branch `wp/255c3-no-invented`, base a33d1ed4)

## Done
- A (037d1f5b): `Rows::section_place -> bool`, `Valued::NoPlace` without a
  payload, `Rows::def -> Option<RowDef>` (rows.rs; `From<Def>`). No reader
  read either payload. static-identical: exactly the 12 known rows.
- B (4ac6e4b1): mutation tests `stored_rows_sees_a_stale_*` (6) +
  `stored_rows_value_of_gives_the_reason`; the comparison also compares
  slots, PLT, `def` (the value alone misses a SLOTS swap and a definer
  MENTIONS swap: verified for MENTIONS by running without the def check).
- C (uncommitted until its run): `stored_rows_scale` (`#[ignore]`,
  `WP255C3_WORKLOADS=<bench workload dir>[:...]`), run by
  `.build/target-wp-141/wp255c3/scale.sh "<workloads>"` (release, no LTO).

## Next
- Record the scale numbers in `spec/landed/incr/WP-255c3.md`; commit C.
- Final gates (quick, linktest, census, fmt, workspace clippy,
  static-identical 12 rows, incr-update); delete WIP.md in the last commit.
