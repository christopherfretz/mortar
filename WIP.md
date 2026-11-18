# WIP: WP-218 (U-6) refill, image, the update's map

Branch wp/218-incr-image, base a828a3de. Resume: read this, then
`git log a828a3de..`.

## Done
- map.rs decisions (I-78a) + update_map; update.rs refill_counted.
- L-159 region fix (members of another output section skipped).
- crates/mortar/tests/update_image.rs: 6 linked tests pass (AArch64,
  x86-64); hand-made growth unit test in update.rs.
- `delta cause` lines: no rule defines them (IN-79, WP-213 §J): none.
- .eh_frame_hdr on a real link (kiln++): 0x26514 B hdr, 0x146358 B eh.

## Next
1. confirm t_incr (incremental/incr_oracle/incr_review/written) passes.
2. record spec/landed/incr/WP-218.md; gates; delete WIP.md.
