# WIP WP-238 Part 3b (branch wp/238-live-5)

Done (committed): Task 1 (cause, fix, oracle on the written bytes, hook
`MORTAR_INCR_LIVE_HASH`, tests), Task 3 (piece-move, START_STOP scenario,
REFUSE mutations, FDE local symbol). Record: spec/landed/incr/WP-238.md
"Step 2 Part 3b".

Task 2: done (record P3b-5); script and log:
(builds base 7ca766c0 by checking its crates out and back; nothing else
may run in the worktree meanwhile), results in `bench.log` there.

Not done: Task 4 (removals of the design oracle / probe): budget.

Resume: re-run bench.sh if bench.log is incomplete; then the hand-back
gates (quick, linktest, census, fmt, clippy, static-identical,
incr-update).

Gates on 844b84ab: fmt, clippy (workspace), linktest 0 FAIL, census 716
pass, incr-update 440 PASS / live-check agree:40; quick and
static-identical fail on exactly the 12 known rows only. Hand-back done.
