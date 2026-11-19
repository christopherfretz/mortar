# WIP-255e (C' in shadow)

Done:
- Part 1 note: `spec/landed/incr/WP-255e.md` "Design".
- Shadow S1 + S2 in `crates/mortar/src/update/mod.rs` (`shadow_set`,
  `shadow_255e`, `first_output_difference`, `shadow_near`), always on in
  `front_two` after front 2's refill; one call in `placement`'s
  `if materialize` block, one field on `Placed`/`Materialized` (named in the
  note, R-e1).
- `update_two_front.rs`: S1 asserted equal on every taken hop; hop 4
  (`NEWREL`, a new relocation into `u.c`).

Next:
1. FINDING S2-1 (x86-64 hop 2, the clip plan's gap rule refuses 5 bytes
   [0xc7b,0xc80) of `.text` between a shrunk region's `Fill` hole and the
   next member): bytes of the output there, then which producer ends the
   hole short (the delta's `holes()` region end vs the full link's
   `slack.region`).
2. Lane (done, ec3550da): incr-update 584 PASS / 0 FAIL; 86 taken hops, 86
   `255e shadow: equal` (S1), 0 finding, 0 refused; S2 78 equal, 8 with no
   range outside synthetic chunks. Evidence `w255e/after-findings.txt`.
3. NOT done (hand-back at the budget): protoc timing, the full gate.
Resume: `w255e/lane.sh` (tests + `cargo xtask incr-update` with findings).
