# WIP WP-052 (static-pie)

Done:
- layout: StaticPie placed as Pie on ELF64 (target.rs); i386 keeps the refusal name.
- resolve: `_DYNAMIC` defined under static-pie (linker.rs When::Dynamic; resolve.md 8.1 row).
- mortar front: `has_dynamic` (dynamic || StaticPie) drives the dyn tables; scan/arch stay static (M1, dynamic=false).
- dynlink: DynKind::StaticPie, no interp (DynTables.interp Option).
- synth got.rs: DynGotCx.static_pie -> IPLT IRELATIVE in .rela.dyn (rela_dyn_irelative, irelative_records); lazy/jump_slot only with PLT entries.
- rela.rs/dynrel.rs/link.rs: IRELATIVE tail of .rela.dyn.
- C hello runs on aarch64 and x86-64 (qemu); the kind test passes with --no-plumb.

Learned: plumb reports static-pie "outside stage 3 (WP-052)": verifier request; xfail with that reason.

Next: item 1 table into record; programs of item 1 through mortar; refusals (item 3); tests (item 4); corpus (5); static-identical (6).
