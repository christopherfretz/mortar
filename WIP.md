# WIP — WP-095 (closed-pipe stderr/stdout)

Done: reproduction and references (spec/landed/cli/WP-095.md); the rule
(spec/cli.md §3.15, diag.md §7.1); writer crates/mortar_diag/src/stream.rs;
sites moved; -M printed by mortar::run before the verdict; tests
crates/mortar/tests/streams.rs, crates/mortar_diag/tests/streams.rs;
mortar_testutil::run_with_streams.
Next: gate (MORTAR_REQUIRE_TOOLS=1 cargo xtask ci --quick), push branch,
hosted run, delete WIP.md in the final commit.
Scratch: /tmp/mortar-wp095-4109496.
