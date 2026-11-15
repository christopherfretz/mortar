# WIP — WP-028c (mortar_plumb for x86-64)

## Done
- S-17 piece lookup (reloc.rs resolve_at/section_piece; tests/pieces.rs);
  WP-027b filter removed in crates/mortar_image/tests/link.rs (passes).
- x86-64 table src/x64.rs; record path src/x64rec.rs (GOT X1-X4, TLS
  per linker, X-46, X-53, W4 = A12); unjudged records exempt their window.
- eh.rs/debug.rs per machine; L3 judges EM_X86_64 (297 PASS); defaults
  `mold,x86_64: A13`.
- Properties per X-41 (property.rs); plumb.md §6.6 rewritten.
- S7 1-byte entry on x86; M3/M4 property note may change size; Y5 large
  sections; Y3 commons (W9 = A14); S19 .init/.fini padding (W8 = A16);
  X-53 (W10 = A15); S11 (A13).
- F5 (ifunc.rs x64_iplt): IBT entries start endbr64 (W6 = A17, mold);
  mortar entries exactly X-37/X-38; IFUNC initial word per linker (X-39:
  mortar/lld 0, mold resolver, GNU entry+6 or entry with endbr64, 0
  without entry). property.rs x86_feature_1_and.
- tests/x64.rs: 18 cases x 3 linkers, relax families, flips 384/384,
  opcodes 161/161, S19/Y3/P1/TLS 1/1, F5 2/2 GNU and lld.

## Learned, not yet written (into plumb.md / the record)
- GNU `_TLS_MODULE_BASE_` = p_memsz (at TP), x64.md X-47 says tls.start.
- GNU GOT load of IFUNC: `.got` slot with IRELATIVE (word 0), no entry.
- GNU IBT IPLT: initial word = the entry (endbr64), not entry+6.
- lld W4 also clips the preceding GD16's last byte.
- mold IE mov -> I1 per symbol unless a reference is `add`.

## Next
- Scope 5: large sections "not judged: named" (X-54).
- plumb.md: §4.5 x86 table, §4.8 F5, §5.9 per-linker forms, §6.2 S19,
  §6.5 commons, §9.2 kill rates, §10 A12-A17, R-12.
- Timing before/after (base xtask via git archive into
  .build/scratch-wp028c). Record spec/landed/plumb/WP-028c.md. Gate.
