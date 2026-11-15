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
- Image wired (`mortar_image/src/link.rs`: DynPlans, DynView (DynAddrs +
  DynamicAddrs), DynChunks, `rela_dyn`, sh_link/sh_info, EI_OSABI, map
  DynRecords + args; `KeyFacts::walk_records`; `dynrel::push_records`).
  `mortar_synth`: `symtab::global_value`, GotPlt `write_got_plt_at`,
  DynstrChunk/VerneedChunk `names: &(dyn DynNames + Sync)`.
  `mortar_scan` Visit carries place/name/output (`Visit::cx`).
- Targets 1a and 1b: link, run, plumb findings 0, gdb + lldb break on main.
  eu-elflint: `.relro_padding` flagged (elflint `.relr` prefix match; lld
  same) - finding, not fixed.

## Next (in order)
1. Workspace tests (static-identical, image, options tests).
2. Targets 2 (tests/link/dyn xfails tagged WP-053) and 3 (dyn.md §16 C).
3. CLI AT_JOIN fold, docs/status/options.md, tests; record; gate.

Scratch: /tmp/mortar-wp053-1856285 (hello.c/.o, w/ld wrapper, line.txt).
