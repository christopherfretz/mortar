# WP-243 WIP (deleted in the final commit)

Done: first test `select_in_per_file_equals_restricted_full_selection`
(crates/mortar_synth/tests/real.rs; passes on the built-in gcc sets);
`Slack::eh_pad` (crates/mortar_layout/src/slack.rs), the shared capacity rule.

Next: protoc sets via MORTAR_EH_SELECT_SETS (scratch t-select2.sh);
crates/mortar_layout/src/delta/eh.rs (item 5), wiring in delta.rs,
tests/delta.rs, record section "WP-243" at the end of
spec/landed/incr/WP-216.md, full gate.

Resume: scratch /Volumes/AgentWork/mortar-helper/.build/target-wp-155/wp243-scratch
