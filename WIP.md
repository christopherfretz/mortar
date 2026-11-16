# WIP — WP-095 (closed-pipe stderr/stdout)

Done: reproduced (MORTAR_STEPS + EPIPE/ENOSPC on stderr -> SIGABRT, from
std's print_to panic in mortar_core par.rs Region::drop; stop condition not
met); references measured; writer `mortar_diag::stream`; sites moved
(main.rs, front.rs print_steps, fill.rs print, wp032b_steps.rs, par.rs);
-M printed by `mortar::run` before the verdict (lib.rs); scan test
crates/mortar_diag/tests/streams.rs.
Next: spec text (cli.md §3.15, diag.md §7.1), binary tests
crates/mortar/tests/streams.rs, record spec/landed/cli/WP-095.md, gate.
Scratch: /tmp/mortar-wp095-4109496 (probe.py, reader.py).
