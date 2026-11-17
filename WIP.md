# WP-161 WIP (verifier: -shared option tail; Shared sweep)

Brief: /Volumes/AgentWork/mortar-helper/briefs/wp-161.md
Target dir: .build/target-wp-141 (env.sh's default for this worktree).

## State
- Part 1 done (8fe775b). Part 2: rules + reference tests (5189e60);
  mortar fixtures tests/shared/optail + `p_d_mortar_option_tail`, spec
  sentences (P-D7, Y3, P-D22, §3.2a table, M10 roots) in progress.
- Part 3: xtask/src/sweep.rs gains cases `shared`, `shared.tail`
  (library: true); mortar_plumb sweep covers `.gnu.version_d`.

## Next
- Run the optail test; measure the Shared sweep survivors
  (`cargo xtask sweep --case shared,shared.tail --arch aarch64,x86_64`);
  judge or name each in plumb.md §12.3 + sweep::EQUIVALENT.
- Gate + lane in the background; record spec/landed/plumb/WP-161.md;
  delete this file.
