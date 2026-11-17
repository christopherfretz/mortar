# WP-067 (L10) work in progress
Done: tests/l10/ (l10.py, judge.py, edits.py, build.py, projects gen200 + corpus40, subjects), xtask/src/l10.rs, the
one-line hooks in lib.rs, main.rs, ci.rs (the smoke chained to the linktest step).
Learned: tests/incr-oracle/oracle.py reads thin-archive members at their archive offset and identifies members by
offset; judge.py adapts both (finding for the hand-back).
Fixed: the previous snapshot shared inodes with W (an in-place rewrite changed it: a false oracle MISS);
PIE runs report `incremental-unsupported`; a base link refused by name ends the sequence as REFUSED.
Findings so far: clang -O2 -g a64: `--incremental` output has 201 more `$d` locals in .eh_frame than F
(plumb --equiv P-E3, 920 findings) -- to minimize into a replay file.
Next: self-test (running); QUICK cost; nightly-scale run for the edit table; docs; record.
