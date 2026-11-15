# WIP — WP-028c (mortar_plumb for x86-64)

## Done
- S-17 piece lookup: `reloc.rs` `resolve_at`/`section_piece`, S-18
  `Target::Outside`, S-19 half-open + end case; test `tests/pieces.rs`.
- x86-64 table `src/x64.rs`; record path `src/x64rec.rs` (GOT forms
  X1-X4, TLS IE/desc/GD/LD per linker, X-46 DTPOFF, X-53, W4 = A12).
- `eh.rs` x86 pointer types; `debug.rs` per-machine ABS64.
- L3: `xtask/src/linktest/engine.rs` `not_judged` judges EM_X86_64;
  default `mold,x86_64: A13` in `tests/link/anomalies`. Full L3: 297 PASS,
  0 FAIL; i386 120 outputs `machine`.
- mold relaxes IE `mov` -> I1 per symbol unless a reference is `add`
  (then all keep GOTTP). A13: mold's PT_NOTE p_align 8 over 4-aligned
  notes (`structure.rs` S11 message, `anomaly.rs`).
- Unjudged x86 records exempt their rewrite window (`x64_unjudged`),
  fixing S12 on stripped outputs.
- Properties per X-41 (`property.rs` rule/treat): no not-judged rows but
  MEMORY_SEAL (3) and GNU on AArch64. Experiment
  /tmp/mortar-wp028c-main/prop: GNU drops 0xc0018000 and 0x7000 with a
  warning; lld drops all non-AND; mold drops COMPAT.
- S7 entry instruction 1 byte on x86 (was 4). M3/M4: `.note.gnu.property`
  may change size (GNU's map gives the merged size).

## Half-done
- none

## Learned, not yet written (into plumb.md / the record)
- GNU `_TLS_MODULE_BASE_` = p_memsz (at TP), x64.md X-47 says tls.start.
- GNU GOT load of IFUNC: `.got` slot with IRELATIVE, no IPLT entry.
- lld W4 also clips the preceding GD16's last byte.

## Next
- x86 corpus test file (tests/x64.rs): C/C++/Fortran, gcc/clang, pic/pie/
  nopie, -fno-plt, gnu2; W1/W5/W9/W10 experiments.
- Scope 3 IPLT shapes; scope 5 layout (.init W8, large sections); scope 7
  mutations; plumb.md amendments; timing; record; merge main (S-17
  acceptance: remove WP-027b filter in crates/mortar_image/tests/link.rs).
