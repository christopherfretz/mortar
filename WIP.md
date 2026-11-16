# WIP — WP-062 (M7 S2: slack and dead ranges)

## Done
- Amendments A-E (layout.md L-141/L-142/L-144/L-147, synth.md S-110/S-111), plumb.md §3.2 `hole`/`dead` line formats.
- Slack policy (`mortar_layout/src/slack.rs`): regions in plan_parts, tails, packed capacity, debug classes, PT_LOAD reserve.
- `Placement.holes` (mortar_core layout + check), `.eh_frame` per-file CIEs and padding CIEs (mortar_synth eh.rs), neutral debug units (mortar_image neutral.rs), map `hole` lines.

## Half-done
- State v2 HOLES table: not started (replay restores no holes yet).

## Learned
- plumb on main errors on unknown map records (`crates/mortar_plumb/src/map/mortar.rs:461`).
- plumb applies S-50 rule 2 to crtbeginT's r=0 relocations against a kept own CIE (Y4 / R1 findings); mortar applies rule 1.
- equiv.py (R13) counts per-file CIEs as differences; IN-2 item 5 needs them passed over.

## Next
- equiv copy adapted; state v2; tests; gate harness; sizes; costs.
