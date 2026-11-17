# WP-067 (L10) work in progress
Done: tests/l10/ (l10.py, judge.py, edits.py, build.py, projects gen200 + corpus40, subjects), xtask/src/l10.rs, the
one-line hooks in lib.rs, main.rs, ci.rs (the smoke chained to the linktest step).
Learned: tests/incr-oracle/oracle.py reads thin-archive members at their archive offset and identifies members by
offset; judge.py adapts both (finding for the hand-back).
Next: replay the three first failures after the adapter; measure; choose QUICK; self-test; docs; record.
