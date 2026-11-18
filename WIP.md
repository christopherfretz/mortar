# WIP: WP-187 (RL-7), the -r join

Brief: `briefs/wp-187.md` + `briefs/rl-common.md` in the context repo.
Scratch: `.build/target-wp-154/rl7/` (cargo.sh = cargo with env + load wait;
m1 = stack-note measurement, m2 = .gnu.warning measurement).

## Done (uncommitted until the first test run passes)
- Driver `crates/mortar/src/relocatable.rs` (front, write, text map,
  size_disagreements), dispatch in lib.rs; -r/-Ur rows Supported.
- Named refusals: section groups (RL-8), {machine} output (RL-9),
  removed .eh_frame records (RL-8, shared with the image).
- stack note L-R6a in mortar_layout; undefined_without_key written;
  property warnings routed; odr-runtime -r test; census OUT_OF_M4R.
- Lane tests written: partial-mortar (renamed), r-busybox-gcc/clang,
  r-kernel-style(-m), r-archive-{alone,u,after-ref}, r-thin-whole-archive,
  r-start-group, r-dev-null-m, r-stack-note.
- crates/mortar/tests/relocatable.rs: sizes, -e, -u, symver, gnu.warning,
  unknown property.
- Spec: cli.md §3.16/C-R2b/C-R3 row, resolve.md §1.2/§11, layout.md L-R8
  (option-made note last), L-R1 .gnu.warning (carried, measured), diag.md.

## Next
1. Tests pass; commit. options.md bless + mirror script.
2. Lane on relocatable/ tests; stage R findings; compose by hand.
3. BusyBox real build; census; static-identical; gates.
4. harness.md cells; record spec/landed/layout/WP-187.md; plumb.md lines.

## Resume
Read this file, `git log`, then continue at the first Next item.
