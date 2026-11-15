# WP-036 work in progress (deleted in the final commit)

Done: static twins (protoc plain/-g/gc, synthetic-*, library shape,
fortran, rust-self); `run` key; plumb `-l`; run-vs-GNU-ld comparison;
CPU column; fastest-linker ratios; perf.md; `--judge`; spec.
First whole run (3 rounds): 12 of 23 linked by mortar, all clean, 11 run
the same as GNU ld; rust-self-static refused (`-Bdynamic`).
Next: three whole runs at different times (results kept in scratch),
judge them pairwise, set the threshold, record the 12c baseline, drop
the 24c one, landed record, gate.
