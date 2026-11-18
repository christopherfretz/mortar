# WIP — WP-244 (format-5 capture from a real link)

Branch `wp/244-incr-capture`, base 01f70788. Scratch:
`/Volumes/AgentWork/mortar-helper/.build/target-wp-143/wp244/`.

## Done
- `SymFacts::{pack, unpack}` (mortar_arch), `View::{key_facts, at_mention}`,
  `Walker::{key_facts, at_mention}` (mortar_scan).
- `run_unit` generic over the global sink (`Shared` = today's atomic OR;
  `Recording` also logs); `mortar_scan::scan_recording` gives per-file
  `FileDemands`.
- `Slack::eh_pad` (identical to 236e5838), `EhPlan::{file_runs, run_pads}`,
  `EhRun` (mortar_synth).
- Switch: `Env.state5` (front), `incremental::State5` on the session, set by
  `mortar_incr::testhook::state5_path()` (`MORTAR_INCR_STATE5=<path>`,
  testhooks build) or `mortar::invoke_state5` (tests). With it: per-file eh
  pads, recording scan, facts; `capture5_timed` + `state5_bytes` write the
  format-5 state beside format 4.
- `capture.rs`: `Link5`, `EhFile`, `Rows5`, `rows5`, `state5_bytes`.
- `rows.rs`: PROPOSED `file_flags::EH_FIXED = 8` (needs a ruling).
- Tests: `crates/mortar/tests/state5_capture.rs`.

## Next
1. Make the tests pass; commit.
2. Experiments (scripts under `tests/incr-design/wp244/`): protoc + cxx-large-g
   (AArch64), M.1 + slot series both targets, determinism, canon vs convert,
   size gate, plain-link cost, incremental-mode cost, M.1 stability.
3. Record `spec/landed/incr/WP-244.md`; full gate; delete this file.

## Resume
`bash .build/target-wp-143/wp244/t5.sh` runs the test file (log t5.log).
