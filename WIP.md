# WIP — WP-249 (DW_MACRO_import into a dropped .debug_macro copy)

Step 1 (measure, propose): scripts in tests/image-design/wp249/ (README there).
Scratch: /Volumes/AgentWork/mortar-helper/.build/target-wp-143/wp249/ (survey/, linkers/, modes/, plumb/, rule/).

Done: survey.sh (class census), rule.sh (GNU ld's condition by experiment), linkers.sh
(5 linkers + 2 patched mortar outputs, gdb/lldb/verify), modes.sh (gc, -r, incremental),
plumbprobe.sh (verifier on patched outputs).

Next: interim hand-back to the supervisor; on the ruling, step 2: the rule in
crates/mortar_image/src/link.rs LinkTables::sym_value_of (Local, facts.dead branch),
lane tests under tests/link/, unit tests, record spec/landed/image/WP-249.md.
Resume: re-run the scripts (each prints its DONE line), see the record draft.
