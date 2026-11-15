# WP-036 work in progress (deleted in the final commit)

Done: static twins (protoc plain/-g/gc, synthetic-*, library shape,
fortran, rust-self); `run` key; plumb `-l`; run-vs-GNU-ld comparison;
CPU column; fastest-linker ratios; perf.md; `--judge`; spec.
First whole run (3 rounds): 12 of 23 linked by mortar, all clean, 11 run
the same as GNU ld; rust-self-static refused (`-Bdynamic`).
Done: three whole runs (09:42, 10:00, 10:28), margins 35% time, 4% memory.
Next: record the 12c baseline (4th run), judge runs 1-3 against it,
landed record, gate.
