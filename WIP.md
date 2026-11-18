# WIP — WP-244 (format-5 capture from a real link)

Branch `wp/244-incr-capture`, base 01f70788. Scratch:
`/Volumes/AgentWork/mortar-helper/.build/target-wp-143/wp244/`.

## Done
- Stage 1 (a9745afb): facts, recording scan, eh pads, the switch
  (`MORTAR_INCR_STATE5`, `mortar::invoke_state5`), `capture.rs` rows.
- Seam 6 picked (23011441); stage 2 (6bbef55b): one producer for the
  per-file stack/properties notes (`stack::file_notes`,
  `notes::object_properties` + `PropertyPlan::merged`), option bit,
  FILE_INDEX.locals, FRAGS.size, FILES.eh_fdes; tests in
  `crates/mortar/tests/state5_capture.rs` pass.
- protoc cxx-medium-static (AArch64) run: wl/cxx-medium-static/*.txt.

## Next
1. Diagnose the round trip (decode -> normalize -> encode refuses:
   "a reference to SECTIONS row N, which has none"), for capture AND convert.
2. cxx-large-g; M.1 + slot series both targets; plaincost.sh; kiln/hod subset.
3. Record `spec/landed/incr/WP-244.md`; full gate; delete this file.

## Resume
`bash .build/target-wp-143/wp244/t5.sh` runs the test file (log t5.log);
`rt.sh` runs the round-trip experiment on the protoc states.
