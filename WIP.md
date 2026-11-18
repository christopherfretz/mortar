# WIP: WP-225 (U-10, the join), M2a2

Done (record "M2a2"): A 03591af0 (the lane's `same_size_body_edit`:
12 real updates; the update writes `update_map` and its report),
B ee4288f0 (`UpdateError`, a failed W-step fails the link), C 70085f6b
(the identity check narrowed; merged-string test ignored: FINDING).

Next (in order):
1. D (M2a-3): call extract, resolve, delta::layout, refill_set and the
   symtab plan in `reference_of` (crates/mortar/src/update/mod.rs) and
   refill with their results; the capture equality stays the guard.
   A byte the packages' set leaves out = finding (bytes first).
2. G-12: the update's `delta` lines from the RefillSet (judgement (c)).
3. M2b.

Resume: scripts in /Volumes/AgentWork/mortar-helper/.build/target-wp-155/wp225/
(`chk.sh` cargo check; `c3.sh <filter>` fmt+clippy+`--test incremental`;
`iu1.sh <scenario>` one lane scenario with --keep; `gate.sh` the whole
gate, GATE-DONE).
