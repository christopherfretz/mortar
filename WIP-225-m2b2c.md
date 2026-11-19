# WIP WP-225 M2b-2c (branch wp/225-m2b2c)

Done (record: spec/landed/incr/WP-225.md "M2b-2c"):
- 1 M2b2b-3: no full-link producer of REGIONS; delta refuses by name instead of overflow.
- 2 M2b2b-2: L-159 empty section beside a neighbour chunk (Fate::Beside).
- 4 C++ fixture judged (tests/update_placement.rs, with test-only regions).
- 5 timing on protoc (placement ~12 ms of ~720 ms update; live 0).
Next: 6 the full gate (scratch gate.sh, FINAL line), then 3 the fall-back
table from findings-gate.txt, into the record.
Scratch: /Volumes/AgentWork/mortar-helper/.build/target-wp-145/scratch-m2b2c
