# WIP-255d2b

Done: step 1 (7bc8aab7, keeps_local/file_locals pure move, static-identical 0 differ,
timing 212 vs 211 ms), step 2 (b90729c2, update/file_symbols.rs + tests/file_symbols.rs,
green on AArch64 and x86-64). Record: spec/landed/incr/WP-255d2b.md.

Next (after the supervisor's rulings R-1..R-3 in the record): global_changes in
update/file_symbols.rs (kind from (SYMTAB_KEYS stored, classify over the new RESOLVED
row), name/info/other from a moved CoreSyms::global part, place/value/size from the new
RESOLVED row, stored from view.symtab_key); then the file's demoted globals appended to
FileSymbols::locals; then step 4's keys without a row.

Resume: scratch /Volumes/AgentWork/mortar-helper/.build/target-wp-140/wp255d2b
(t2.sh runs the test; gate.sh the full gate; protoc.rsp the timing link).
