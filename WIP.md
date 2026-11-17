# WIP: WP-185 (RL-5), stage R of spec/plumb.md §14

Verifier agent. Scratch: /Volumes/AgentWork/mortar-helper/.build/target-wp-154/rl5/
(t.sh runs `cargo test --test relocatable`, writes t.out).

## Done
- report/map groundwork (c04eab54); rules P-P2..P-P11 + dispatch (a86772cb).
- references on all three targets: every finding classified (A90-A102,
  A120-A126 of anomaly.rs), tables in tests/relocatable.rs `expected()`;
  s14_references_{aarch64,x86_64,i386} pass.

## Next
1. Renumber the new anomaly ids contiguous from A120 (A121 is unused).
2. Mutations (V-R12, §9.2 (t) (1)-(22) but (21)), mortar-form ones via a
   hand-written mortar map; map-reader unit tests for the -r records.
3. spec §14 text ("planned" -> implemented, §14.6 counts), §10.2 A120+,
   record spec/landed/plumb/WP-185.md, static-identical --update.
4. clippy; gates: quick, linktest, census. Delete WIP.md in the final commit.

## Resume
cd worktree; unset CARGO_TARGET_DIR; source scripts/env.sh; export MASON_KILN/MASON_HOD; ulimit -c 0.
