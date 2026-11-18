# WIP: WP-253 (verifier: IN-51/IN-51b's neutral forms)

Brief: /Volumes/AgentWork/mortar-helper/briefs/wp-253.md. Scratch:
/Volumes/AgentWork/mortar-helper/.build/target-wp-156/wp253 (build.sh, logs).

Done:
- read IN-51, IN-51b, plumb.md §6.8/§3.2, WP-228 §A.2-A.6, deaddwarf.rs, dead.rs, tests/dead.rs.

Next:
1. look at a real incremental map with -g: which line names the 38-byte table.
2. spec/plumb.md §6.8 text (P-I2 list, P-I6 grammar).
3. code: pure judgement of neutral ranges (src/neutral.rs), deaddwarf.rs calls it.
4. table-driven tests (forms x mutations, counted); tests/dead.rs models updated.
5. forms.sh run (CARGO_TARGET_DIR_WP=/Volumes/AgentWork/mortar-helper/.build/target-wp-156).
6. gate; record spec/landed/plumb/WP-253.md; delete this file.
