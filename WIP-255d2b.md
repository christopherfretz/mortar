# WIP-255d2b

Done: step 1 (7bc8aab7), step 2 (b90729c2); continuation: item 5 (85732937), R-2 moves
(4073d01a, 462779eb), global_changes + demoted_run (132ee164). Second continuation:
R-4 `ResolveDelta::merged` + oracle fixture + R-1 order (57ae0328), R-1 test (2d588a9f),
F-3 check (9bd7c6ce), R-3 tests (33411af5), clippy (b371cd72).
Record: spec/landed/incr/WP-255d2b.md ("The second continuation").

Open: F-2 producer is WP-262's; F-4 (x86-64 objects mention _GLOBAL_OFFSET_TABLE_ ->
(symtab) fall-back); demoted_run's signature change needs the supervisor's word.

Scratch /Volumes/AgentWork/mortar-helper/.build/target-wp-140/wp255d2b
(t2.sh runs tests/file_symbols.rs; gate.sh the full gate).
