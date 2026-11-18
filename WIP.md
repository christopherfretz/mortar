# WIP: WP-246 (V-2), names of .symtab entries judged

Brief: /Volumes/AgentWork/mortar-helper/briefs/wp-246.md. Scratch:
/Volumes/AgentWork/mortar-helper/.build/target-wp-141/wp246/ (evidence copies in ev/,
big/ = V-1's 106 MB output copy, base-src/ + base-target/ = base a828a3de xtask build,
sweep.py = xtask plumb over every committed map fixture, per rule).

Done:
- src/symnames.rs: Y5's closed set, Y6 (mortar maps only), Y7; opts.rs Discard, undefined;
  report.rs coverage; Y3 hidden names; spec/plumb.md §6.5 text incl. the measured exceptions.
- tests/symnames.rs + fixtures + mut.py: all 9 pass; evidence: inc.bad 15 findings.
- Other plumb tests updated (eh s4_4, symfill near_filler / p_e3 nonalloc, equiv's line -x).

Next: full plumb test run; sweep.py base vs new (per-linker counts); big-output timing;
linktest + census + L10 counts; record spec/landed/plumb/WP-246.md; full gate; delete WIP.md.
