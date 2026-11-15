# WP-053 WIP (delete in the final commit)

## Done
- Guard lifted for AArch64 Exec/Pie (`crates/mortar/src/dynamic.rs`
  `arch_status`); x86-64 refused naming WP-055, i386 refused for M2.
- Phase 80 dynamic planning in `crates/mortar/src/dynlink.rs` (DynSource,
  OutWritable SectionMap, ScanKinds, `plan` -> GotPlan + DynTables).
- `front.rs`: dynamic ScanOptions, walker with the scan's weak_dyn,
  `dyn_chunks`, `place_with_copies`, `rank::home` for `.dynbss`.
- Fixes in others' code: `sites.rs` RoundTables::plt uses `plt_address`;
  `mortar_scan/src/walk.rs` `Walker::with_weak_dyn`.
- Target 1a passes phases 20-90; fails in image: `.interp has no plan`.

## Next (in order)
1. Image: chunk arms in `mortar_image/src/link.rs` (Interp, GnuHash, Dynsym,
   Dynstr, GnuVersion(R), RelaDyn, Dynamic, DynBss/BssRelRo), DynAddrs /
   DynamicAddrs, `.rela.dyn` records, sh_link/sh_info, EI_OSABI,
   `SlotValues::dynamic`, map DynRecords + args;
   `mortar_synth/src/image.rs:244` GotPlt -> `write_got_plt_at`.
2. `crates/mortar/src/image.rs`: dynamic scan options, weak_dyn, plans.
3. Targets 1a/1b checks; CLI AT_JOIN fold, docs/status/options.md, tests.

Scratch: /tmp/mortar-wp053-1856285 (hello.c/.o, w/ld wrapper, line.txt).
