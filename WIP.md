# WIP: WP-228 (G-1) part B, branch `wp/228-incr-debug-b`

Part A and step 1 are done and ruled (`/Volumes/AgentWork/mortar-helper/briefs/wp-228-rulings.md`,
last section "After part A" wins). Base e14dd5f4 (the pair base).

Done (B1):
- Seam 8, ONE commit 4f721d3e: `format::kind::DEBUG_STRS` (57),
  `rows::DebugStrRow` (13 bytes, no tomb), `rows::debug_str_class`,
  `StateRows::debug_str` (provided `Ok(None)`), `IncrReason::{DebugComdatKeeper,
  DebugGrowth}` with their two rows in `spec/incr.md` section 7.
- Gate on the branch head (seam 4f721d3e + the cherry-picked baseline 0592b43d): logs in `.build/target-wp-154/wp228/b/gate/`
  (script `wp228/b/gate.sh`). Accepted failures: exactly the 7
  `mortar --test debuggers` tests (`incr_dwarf_{c_gcc,cxx_gxx,cxx_clangxx,fortran_gfortran}`,
  `incr_sessions_{c_gcc,cxx_gxx,cxx_clangxx}`) until WP-253.
- DWARF64 inputs exist (GCC 14 and Clang 19 `-gdwarf64`, both targets;
  `wp228/b/dw64/`); mortar links them (plain link). R-2.6: no name added.

Next: WAIT for the supervisor's word on B1 (R-1's line, the questions in the
hand-back). Then B2: R-1 commit in `crates/mortar_layout/src/delta.rs`,
then `update/debug.rs`, then `tests/update_debug.rs`, the record, the gate.
Do NOT start `update/debug.rs` before the word.
