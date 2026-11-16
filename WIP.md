# WP-091 work in progress (deleted in the final commit)

Done: map reader/writer (`incremental`, `hole`, `dead` records, mortar-map 1
kept); `dead.rs` (resolve, coverage, P-I1/2/3/5/6 alloc); `deaddwarf.rs`
(P-I4, P-I6 debug); hook in `run_engine` when `Map::claims_coverage`;
`cargo xtask plumb --coverage`.

Next: coverage table over the kept linktest corpus
(`.build/target-wp-091/linktest/run-3079192`, script in the scratch dir),
finalize the padding rule; mutation tests (IN-60 cases) in
`tests/dead.rs`; mutation coverage table; plumb.md; record
`spec/landed/incr/WP-091.md`; costs on protoc; gate; hosted run.
