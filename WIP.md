# WIP: WP-228 (G-1), debug information in an incremental update

Step 1 DONE and ruled (`/Volumes/AgentWork/mortar-helper/briefs/wp-228-rulings.md`).
NOW: step 2, **Part A only**. Part B (R-1 commit, R-2 seam commit,
`update/debug.rs`) waits for the supervisor's word and a new base.

Done (part A):
- `neutral.rs`: all forms (loclists, loc, frame, addr v5 and GNU, macro,
  macinfo, `info_addr_base` codes 3/4, `type_unit` code 2), exact-byte
  tests, `emit_forms`.
- WP-229's address-base forms reconciled: ONE table of four
  abbreviations, 38 bytes (record A.1, A.2).
- `forms.sh` run on the one-table build: 816 outputs, both targets
  (record A.3); verifier findings per form (A.4). Logs:
  `.build/target-wp-154/wp228/a2/keep/`.

Next:
1. Pre-table gate (`a2/gate.sh pre`): fmt, clippy, ci --quick, linktest,
   census, static-identical (0 differ), incr-update.
2. LAST commit: `git apply .build/target-wp-154/wp228/a2/one-table.patch`
   (`neutral::ABBREV_TABLE` 37 bytes, `slack::ABBREV_TABLE` 38), commit,
   then `a2/gate.sh post`: list static-identical's changed rows (no
   `--update`), the lane with and without.
3. Interim hand-back; wait for Part B.
