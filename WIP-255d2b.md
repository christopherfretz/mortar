# WIP-255d2b

Done: step 1 (7bc8aab7), step 2 (b90729c2); continuation: item 5 (85732937), R-2 moves
(4073d01a global_info, 462779eb defined_global; static-identical 0 differ each; protoc
201/195/199 ms), global_changes + demoted_run (132ee164). Record: spec/landed/incr/WP-255d2b.md.

Blocked: F-2 (no capture writes SYMTAB_KEYS: every row NO_ENTRY), F-1 (merged visibility
and referenced not in ResolveDelta; global_changes takes `merged`). R-3's tests not built.

Resume: scratch /Volumes/AgentWork/mortar-helper/.build/target-wp-140/wp255d2b
(t2.sh runs tests/file_symbols.rs; gate.sh the full gate; moves.sh the move checks).
