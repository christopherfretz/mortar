# WIP: WP-183 (RL-3), layout at -r

Done: design read; cells C1..C4 sent to the supervisor (terminators, consumed
kinds copied at -r, -u globals, compressed inputs).

Next: relocatable.rs (layout + pure sizes), check/relocatable.rs (validate),
tests/relocatable.rs, spec edits (model §4.6 sentence, layout §24 S1), record
spec/landed/layout/WP-183.md, background gate script in
.build/target-wp-141/rl3/.

Resume: cargo with the worktree's scripts/env.sh; load < 12.
