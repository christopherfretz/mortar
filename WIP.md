# WIP: WP-123 (the join for -shared), branch wp/123-shared-join

Briefs: /Volumes/AgentWork/mortar-helper/briefs/{m4-shared-common,wp-123}.md
Scratch: /Volumes/AgentWork/mortar-helper/.build/target-wp-123/scratch/ (keep s1/, v3/, v4/)

## Done
- Shared wired (dynlink, image, dynamic arch_status AArch64 only, front); CLI rows; audit proofs
  in crates/mortar/tests/shared.rs; opts.rs mirror (1e41bd2); int/batch-5 merged (6dbd54c)
- link tests tests/link/dyn/shared/* incl. refusal tests (target, x86-64, i386)
- minimal fixes outside the list: check.rs, walk.rs, bound.rs/driver.rs (regflow hook, imported
  IFUNC), census.rs OUT_OF_M2, mortar_scan/tests/shared.rs (Clang row now refused)
- F2 fix (coordinator: mine): scan weak.rs `shared_undefined` fills Demands.weak_dyn in Shared;
  synth dynsym.rs and check.rs read it instead of FROM_OBJECT; dyn.md D-111 sentence

## Next
1. run crate tests + linktest shared/; commit F2
2. executable side of F2: -pie --gc-sections, only ref to a shared fn in a collected section;
   if an existing output would change: report, do not change
3. census (-shared line), Lua by hand, static-identical --update, bench guard (load < 10)
4. record spec/landed/dyn/WP-123.md (F3: GNU/lld/mold exact outputs), delete WIP.md, gate

## Findings to record
V1 (opts mirror), V2 (P-D6, fixed in batch 5), V3/V4/V5 (verifier on references), F2 (fixed),
F3 (refusal stays; census entry), .symver message nit, harness.md:1254 "until WP-123",
odr_output None for Shared (WP-129)
