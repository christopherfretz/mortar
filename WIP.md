# WP-053 WIP (delete in the final commit)

## Done
- Guard lifted for AArch64 Exec/Pie (`crates/mortar/src/dynamic.rs`
  `arch_status` now takes the machine; x86-64 refused naming WP-055, i386
  refused for M2). `front.rs` calls it at phase 45 and passes the status to
  `LinkArch::of`.
- Target 1a fails for the right reason: `unsupported feature: PLT entries in
  a static output` (front still calls the static `GotPlan::new`).

## Next (in order)
1. Phase 80 dynamic planning in the driver (new module
   `crates/mortar/src/dynlink.rs`): `DynSymSource`/`DynNames` over Inputs
   (shared `.dynsym`, soname, versym), `KeyKinds` via `Walker::facts`,
   `SectionMap` over output-section flags; `copy_plan`, `dynsym_plan`,
   `verneed_plan`, `dynstr_plan`, `GotPlan::new_dynamic`, `RelaPlan`,
   `DynamicPlan`, `InterpPlan`; `ScanOptions{dynamic:true, sections}`.
2. `synth_chunks`/`placed_names`/`place_with_copies` in `front.rs`.
3. Image: chunk arms in `mortar_image/src/link.rs`, `.rela.dyn` writer,
   sh_link/sh_info, EI_OSABI, `SlotValues::dynamic`, map DynRecords + args.
4. CLI AT_JOIN fold, docs/status/options.md, tests.

Scratch: /tmp/mortar-wp053-1856285 (hello.c/.o, w/ld wrapper, line.txt).
