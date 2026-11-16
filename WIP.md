# WIP — WP-062 (M7 S2: slack and dead ranges)

## Done
- Amendments A-E (layout.md L-141/L-142/L-144/L-147, synth.md S-110/S-111), plumb.md §3.2 `hole`/`dead` line formats.
- Slack policy (`mortar_layout/src/slack.rs`): regions in plan_parts, tails, packed capacity, debug classes, PT_LOAD reserve.
- `Placement.holes` (mortar_core layout + check), `.eh_frame` per-file CIEs and padding CIEs (mortar_synth eh.rs), neutral debug units (mortar_image neutral.rs), map `hole` lines.
- State format 2: HOLES (kind 39) before STRINGS; reader takes v1 as no holes. Replay restores holes.
- Tests: replay mutations for capacity/holes; padding CIEs and dead writer via hand-made HOLES; debuggers incremental lane (7 tests pass); map grammar test.
- Neutral abbreviation table hole is 9 bytes (8 + terminating 0): readelf complained with 8.
- Gate harness (.build/wp-062-gate): linktest through the wrapper. Incremental first links: aarch64 61, x86-64 70, i386 66, all EQUIVALENT; x86-64/i386 0 FAIL; aarch64 50 FAIL = S-50 Y4 + 2 R1 (crtbeginT, WP-091), 0 FAIL with --no-plumb.
- IN-33 measured on protoc / protoc -g.

## Learned
- plumb on main errors on unknown map records (`crates/mortar_plumb/src/map/mortar.rs:461`).
- plumb applies S-50 rule 2 to crtbeginT's r=0 relocations against a kept own CIE (Y4 / R1 findings); mortar applies rule 1.
- equiv.py (R13) needed: CIE pieces in one output passed over; .tbss units skipped (overlap); thin members; erratum 843419 veneers.

## Next
- ci --quick, push, hosted CI.
