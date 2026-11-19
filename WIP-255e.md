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
2. Lane counts: `.build/target-wp-140/w255e/after-findings.txt`, bucket
   by `255e shadow:` prefix.
3. Timing (protoc static), the full gate.
Resume: `w255e/lane.sh` (tests + `cargo xtask incr-update` with findings).
