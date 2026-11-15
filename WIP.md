# WIP — WP-084 (x86-64 large sections)

Done: layout/resolve/core code; run_class (.lbss-only joins small RW, core layout.rs);
tests: x64.rs large-model tests (pass), lcommon.rs, stages.rs, differential large builds
(not yet run); specs: X-52 note removed, X-54 measured, L-36a rewritten, X-52 test row.
Measured: /tmp/mortar-wp084-2664338/e1.

Next: tests/link/layout/x64-large-*.test (pie, no-pie, 4 linkers); weak-class-table xfail line;
run differential; diag.md rows; layout.md 1575 refusal list; Q3 row; record; AArch64 large;
baselines; CI; gate.
