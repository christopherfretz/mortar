# WIP: WP-183 (RL-3), layout at -r

Done: relocatable.rs (layout + pure sizes, 8550a350), check/relocatable.rs
(validate, validate_groups, validate_relocs), tests/relocatable.rs, spec
edits (model §4.6 sentence; layout §24 L-R4, L-R6 "In bytes", L-R1a S15),
record spec/landed/layout/WP-183.md.

Next: tests green + clippy; commit; final gate
(.build/target-wp-141/rl3/gate-rl3.sh, background, one line RL3-GATE);
delete WIP.md in the final commit; hand-back per rl-common.md.

Resume: cargo with the worktree's scripts/env.sh; load < 12;
rl3/test-rl3.sh runs the tests.
