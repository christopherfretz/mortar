# WP-238 WIP (step 2 Part 2: the producer and the capture)

Brief: the supervisor's Part 2 message; rulings `briefs/wp-238-rulings.md`
(P0-1..P0-7, F11..F15 win over S1-S7). Seam 9 = e968996e (fixed).

## Done (Part 2)
- `mortar_core::split::{EVERY_PIECE, piece_offset}` (F12's one conversion;
  a shared-crate addition: needs a ruling).
- `mortar_live`: gc.rs `prepare` (steps 1-3 shared), `stored` (the body of
  `stored_edges`: Walk::edges rows, F8, ROOT/NAMED/SPLIT, ring, REFUSE 1/2),
  `positions` (new pub `live_positions`: S4's capture side from def_target).
- `mortar_incr`: capture `rows5` turns `Link5.edges` into `EDGES`;
  `update/live.rs`: `walk_stored` (the stored-form walk, S2/S3), `live_hash`
  (S4 with F14's occurrence), `StoredWalk`; `testhook::live_check`.
- `mortar`: front computes `Incr5.live_rows` (gc + state5 only);
  incremental.rs passes the rows, writes `META.live_hash`, runs `live_check`
  under `MORTAR_INCR_LIVE_CHECK` (a disagreement fails the link).

## Next
- Tests (crates/mortar/tests/live_rows.rs): F1, scenarios.
- lane.py env for incr-update; the lane replay wrapper; L10; bench; gates.
- Record's Part 2 section; hand-back.

## How to resume
- Hooked binary: scratch `p2/hooks.sh` (into `.build/target-wp-125/testhooks`).
- Quick check: `p2/bin/ld` wrapper with gcc -B (scratch `p2/t1`).
