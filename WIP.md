# WIP: WP-123 (the join for -shared), branch wp/123-shared-join

Briefs: /Volumes/AgentWork/mortar-helper/briefs/{m4-shared-common,wp-123}.md
Scratch: /Volumes/AgentWork/mortar-helper/.build/target-wp-123/scratch/

## Done
- read the records (WP-110 §D/§G.4, 116-122, 120 harness, 121 plumb)
- wired Shared: dynlink.rs (kind_and_interp, plan, SharedTables, shared_init_fini),
  image.rs (SharedPlans), dynamic.rs (arch_status: AArch64 only), front.rs (CheckContext.shared)
- minimal fixes outside the list: mortar_core/src/check.rs (CheckContext.shared, D-111
  undefined set, no copy alias in Shared); mortar_input/src/walk.rs (closure: Shared roots'
  DT_NEEDED edges recorded without search, D-11 item 3); mortar_scan/src/bound.rs + driver.rs
  (regflow at WP-117's hook)
- CLI: -shared partial, -h supported (SHARED_ONLY refusal outside Shared), -Bsymbolic-functions,
  -Bno-symbolic, --no-undefined, -z defs supported; mortar_cli tests updated; options.md blessed
- first library links, runs under GNU host / mortar host

## BLOCKED (asked the coordinator)
- V1: verifier's ACCEPTED (crates/mortar_plumb/src/opts.rs) lacks -shared and -h/-soname:
  M9 on every mortar library's map; its test s3_2a fails. Right side:
  scratch/accepted-right.txt (two added lines). V2 P-D6 likely follows from V1.

## Next
1. crates/mortar/tests/options.rs AUDIT: -shared, -h, -Bsymbolic-functions, -Bno-symbolic,
   --no-undefined, -z defs -> Test(crates/mortar/tests/shared.rs, ..); bsymbolic_is_for_shared_objects
   -> -Bsymbolic refused in Shared
2. interposer pairs; first-target link tests in tests/link/dyn/shared/
3. remove WP-123 xfail markers; refusal tests; census; D-124/D-125; Lua; static-identical; record

## Open
- spec edits to make: input.md §8.6 (Shared edges), model.md §4.3 (.dynsym set in Shared)
- odr_output None for Shared (WP-129); --allow-shlib-undefined stays refused
