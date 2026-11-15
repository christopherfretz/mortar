# WIP: WP-051 (dynamic linktest harness)

## Done
- Harness: fixtures (`#@ fixture:`, tests/link/fixtures/*.fixture, built once per (name, arch) per run under <work>/fixtures), `shlib` step, `env:` on run, check tools elflint/ldd/gdb/lldb, expectations then/count/allow, `Raw::Skip` (gdb/lldb on emulated arch), per-test xfails in tests/link/xfails, stage-3 probe (engine.rs `stage3_present`) with "stage 3 absent".
- Cases: dyn/kinds/output-kind-table, dyn/tools/{ldd-lists,gdb-break-main,lldb-break-main,elflint-dynamic}: references PASS, mortar XFAIL by package.

## Next
1. Unit tests for new parser forms, program_env, Raw::Skip in self_check, stage3_present.
2. spec/harness.md linktest sections: document all of the above.
3. Author all dyn.md §16 rows + census (subagents by area), xfail lines.
4. Gate, timings, spec/landed/dyn/WP-051.md, delete WIP.md, rm /tmp/mortar-wp051-137416.
