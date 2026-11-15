# WIP — WP-084 (x86-64 large sections)

Done: code, tests (x64.rs, link tests, differential, unit), specs, record spec/landed/x64/WP-084.md,
aarch64 baseline rows, main merged once (8366315).
Next: gate (MORTAR_REQUIRE_TOOLS=1 cargo xtask ci --quick); push branch; gh workflow run ci;
put the x86-64 lane's static-identical rows in tests/identical/baseline.on-x86_64.tsv; rerun;
delete WIP.md in the final commit; remove /tmp/mortar-wp084-2664338.
