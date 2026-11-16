# WIP — WP-062 (M7 S2: slack and dead ranges)

## Done
- Amendments A-E (layout.md L-141/L-142/L-144/L-147, synth.md S-110/S-111), plumb.md §3.2 `hole`/`dead` line formats.
- Slack policy (`mortar_layout/src/slack.rs`): regions in plan_parts, tails, packed capacity, debug classes, PT_LOAD reserve.
- `Placement.holes` (mortar_core layout + check), `.eh_frame` per-file CIEs and padding CIEs (mortar_synth eh.rs), neutral debug units (mortar_image neutral.rs), map `hole` lines.

- State format 2: HOLES (kind 39) before STRINGS; reader takes v1 as no holes. Replay restores holes.
- Tests: replay mutations for capacity/holes; padding CIEs and dead writer via hand-made HOLES; debuggers incremental lane (not yet run); map grammar test.
- Neutral abbreviation table hole is 9 bytes (8 + terminating 0): readelf complained with 8.

## Half-done
- Gate harness (.build/wp-062-gate: equiv.py adapted, mortar-incr-wrap.py) written, not yet run on the corpus.

## Learned
- plumb on main errors on unknown map records (`crates/mortar_plumb/src/map/mortar.rs:461`).
- plumb applies S-50 rule 2 to crtbeginT's r=0 relocations against a kept own CIE (Y4 / R1 findings); mortar applies rule 1.
- equiv.py (R13) counts per-file CIEs as differences; IN-2 item 5 needs them passed over.

## Next
- equiv copy adapted; state v2; tests; gate harness; sizes; costs.
