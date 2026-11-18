# WIP: WP-215 (U-3), branch wp/215-incr-resolve

## Done
- Record §Q (first step); seam 1 + seam 2 (85c3b28e, accepted).
- Step 1 committed 165110d7: `mortar_resolve::tables::option_keys`
  (static-identical 3450 links 0 differ; protoc 10 interleaved runs each,
  median 0.29-0.30 s before, 0.29 s after; RSS 150,040 vs 148,432 KiB).
- `update/extract.rs`: shared `Ctx` (update's link: facts by current rank,
  ments_old/new, targets, wrap redirect/sources, edges) and `extract`
  (IN-71b: lost/gained edges, removal cone by round, re-seed + addition).
  Being compiled.

## Next
1. `resolve.rs` (defs per §3.2 classes, COMDAT re-election, Q.2 closure
   check, symbol-kind-changed, KEY_REFS/GROUPS/COMMONS edits; fallbacks
   for gaps G1 values, G2 strings), `live.rs` (G3: empty without gc, else
   named fallback), `defined.rs` recompute, `refill.rs`.
2. `--start-lib` refusal in `crates/mortar/src/incremental.rs` + test.
3. Tests in `crates/mortar_incr/tests/update_resolve.rs`; the random
   feedback test in `crates/mortar/tests/` (flag it).
4. Cherry-pick U-8's seam when its hash arrives; do NOT touch ChangedFile.
5. Gate, record `spec/landed/incr/WP-215.md`, delete this file.

## Gaps to report (rulings): G1 ChangedFile lacks st_value/st_size;
G2 ChangedFile Strs unreadable; G3 SECTIONS lacks IN-29 root flags, gc
unknown.

## How to resume
Brief `briefs/u-2-to-9.md` U-3 + `briefs/wp-215-rulings.md`, this file,
record §Q. Scratch `.build/target-wp-143/wp215/` (chk.sh = cargo check).
