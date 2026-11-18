# WIP: WP-253 (verifier: IN-51/IN-51b's neutral forms)

Brief: /Volumes/AgentWork/mortar-helper/briefs/wp-253.md. Scratch:
/Volumes/AgentWork/mortar-helper/.build/target-wp-156/wp253 (build.sh, logs).

Done (committed): spec/plumb.md P-I2/P-I6 text; src/neutral.rs (judgement +
table-driven tests, 160 accept / 1916 mutation rows); deaddwarf.rs calls it;
tests/dead.rs links the one table; fixtures converted by tests/retable.py;
symnames' evidence converted + resealed in its scratch copy. Whole
`cargo test -p mortar_plumb` green.

Next:
1. forms.sh run: `bash wp253/forms-run.sh` (FINAL line FORMS-RUN-FINAL in
   wp253/forms-run.final); tally findings from wp253/forms/*/ .plumb files
   vs WP-228 §A.4 (rows 1-7 must be 0, the $d P-I3 1,784 stays).
2. delete large outputs under wp253/forms; also wp253/try, wp253/regen.
3. plain-link cost measurement (xtask plumb on a plain map, base vs now).
4. full gate as one background script; record spec/landed/plumb/WP-253.md;
   delete this file.
