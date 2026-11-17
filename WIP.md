# WIP: WP-185 (RL-5), stage R of spec/plumb.md §14

Verifier agent. Scratch: /Volumes/AgentWork/mortar-helper/.build/target-wp-154/rl5/.

## Done
- (nothing committed yet)

## Next
1. report.rs: Rule::Rel (P-Pn), Via::Place/String, Rewrite::PerLinker, relocatable summary line.
2. map: mortar -r records (rplace, rframe, rgroup, rcommon, output relocatable); foreign blocks.
3. relocatable.rs (model, P-P2/3/7/8/9/10/11), relgroup.rs (P-P6), releh.rs (P-P5), relsym.rs (P-P4).
4. dispatch (verify, verify_with, verify_from) on ET_REL / -r / output relocatable; STAGE_R.
5. tests/relocatable.rs: corpus fixtures (built at test time), mutations (§9.2 (t)).
6. spec §14 text, §10.2 anomalies from A120, record spec/landed/plumb/WP-185.md.

## Resume
cd worktree; unset CARGO_TARGET_DIR; source scripts/env.sh; export MASON_KILN/MASON_HOD; ulimit -c 0.
