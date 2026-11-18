# WIP: WP-228 (G-1), debug information in an incremental update

Step 1 DONE and ruled (`/Volumes/AgentWork/mortar-helper/briefs/wp-228-rulings.md`).
NOW: step 2, **Part A only** (IN-51b text, neutral.rs forms with tool
tests, the 17-byte table as the LAST commit, plumb findings per form).
Part B (R-1 commit, R-2 seam commit, `update/debug.rs`) waits for the
supervisor's word and a new base (batch 14).

Done (part A):
- `crates/mortar_image/src/neutral.rs`: forms of `.debug_loclists`,
  `.debug_loc`, `.debug_frame`, `.debug_addr`, `.debug_macro`,
  `.debug_macinfo` in `unit` (signature unchanged), new `info_addr_base`,
  `type_unit`, `type_signature`, `ABBREV_TABLE_ADDR_BASE`,
  `ABBREV_TYPE_UNIT`; exact-byte tests; `emit_forms` (ignored) for scripts.
- `place.py --rust=BIN` takes every form from neutral.rs; `emulate.sh`
  `RUST_BIN`, `VARIANTS` (adds `v4`); `forms.sh` the batch driver.

Next:
1. `forms.sh` batches (scratch `.build/target-wp-154/wp228/a2/`), tally,
   plumb findings per form with rule ids.
2. Record: "Step 2, part A" section in `spec/landed/incr/WP-228.md`
   (IN-51b sentences, verdicts, findings).
3. Gate before the last commit (quick ci, linktest, census, fmt, clippy,
   static-identical, incr-update).
4. LAST commit: the 17-byte table (`neutral::ABBREV_TABLE` 16 bytes,
   `mortar_layout::slack::ABBREV_TABLE` 17); static-identical rows listed;
   lane with and without it; forms rerun on the types batch.

Resume: scripts run in the background with a load wait (< 12).
