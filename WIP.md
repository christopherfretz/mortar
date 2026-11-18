# WIP: WP-246 (V-2), names of .symtab entries judged

Brief: /Volumes/AgentWork/mortar-helper/briefs/wp-246.md. Scratch:
/Volumes/AgentWork/mortar-helper/.build/target-wp-141/wp246/ (evidence copies in ev/).

Done:
- src/symnames.rs: Y5's closed set (`linker_name`, `unexplained`), Y6 (input locals as a
  multiset), Y7 (STT_FILE named); opts.rs `Discard` (-x/-X/--discard-none); report.rs
  coverage + summary lines; Y3's missing-global bullet now covers hidden names.
- Evidence reproduced (findings: 0 on base); now inc.bad: 15 findings, inc/plain: 0.

Next: measure on plumb's own tests, the corpus, the lane, census; spec text in plumb.md §6.5;
tests (tests/symnames.rs + mutation script); record spec/landed/plumb/WP-246.md.
