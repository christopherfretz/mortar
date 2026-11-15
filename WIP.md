# WP-035 work in progress

- [x] item 1: `xtask/src/bench/verify.rs` + bench.rs wiring, `--no-verify`
- [ ] item 1: bench run evidence (hello-*-static), before/after medians
- [ ] item 2: experiment (static twin of synthetic-sections, window fix reverted in scratch)
- [x] item 3: holes closed in pipeline/options/sysroot/x64/deferred/image tests, mason --mortar-map; table in VALIDATION.md
- [x] item 4: crate-local FNV hasher (timing pending)
- [ ] item 5: ci --quick delta
- [ ] spec/harness.md "Verification", spec/landed/harness/WP-035.md
Finding: every bench workload but hello-c-static/hello-cxx-static is -pie: mortar refuses them.
