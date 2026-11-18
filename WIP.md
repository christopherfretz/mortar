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

## Done since
- tests `crates/mortar/tests/live_rows.rs` (F1, scenarios, F6, -s/-S, thin,
  negative); lane.py live check; linktest replay (scratch `p2/lt`), L10
  (`p2/l10.sh`), bench (`p2/bench.sh`): all agree or refused by name.
- Record: `spec/landed/incr/WP-238.md` "Step 2 Part 2".

## Next
- Gates (`p2/gates.sh`), hand-back. Then wait for Part 3's word.

## How to resume
- Hooked binary: scratch `p2/hooks.sh` (into `.build/target-wp-125/testhooks`).
