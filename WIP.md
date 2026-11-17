# WP-165 WIP (deleted in the final commit)

Brief: /Volumes/AgentWork/mortar-helper/briefs/wp-165.md

Plan:
- `crates/mortar_plumb/src/weakclass.rs`: D-25's class per undefined weak key
  (one derivation), the strict import rule (P-D7) and the split key's
  unfillable sites (P-D9 view 3 + no record).
- anomalies A55 (mold: no entry for a branch-only / split key), A56 (GNU ld
  x86-64: no entry for the split key).
- fixtures + mutations: `crates/mortar_plumb/tests/weak/`, `tests/weak.rs`.
- lift the 3 xfails of weak-branch-split; add `anomaly(mold)` lines.

Measured: mold writes no `__gmon_start__` entry (AArch64 GOT+JUMP26) in
every driver link: references are strict only for branch-only and split
keys (no GOT use, no writable abs word); mortar for every WeakDyn key.

Scratch: /Volumes/AgentWork/mortar-helper/.build/target-wp-125/wp165/
