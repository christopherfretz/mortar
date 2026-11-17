# WP-127 part 2 (x86-64 TLS and IFUNC in Shared) — work in progress

Brief: `/Volumes/AgentWork/mortar-helper/briefs/wp-127b.md`. Scratch:
`.build/target-wp-127/p2/` (m/: models x linkers; sec/: X-62 TLS ruling;
desc/: the gnu2 library by hand).

## Done
- Measurement of GD/LD/gnu2/IE/LE x bfd/lld/mold (p2/m/run.sh).
- Ruling probe: named local TLS symbol: all three agree (r_sym 0 + offset);
  a section-symbol TLSGD: bfd links (addend = PC tail only, mis-keyed),
  lld refuses, mold asserts; mortar refuses by name ("not thread-local").
- mortar_x64: X-61 in Shared (0038d72); shared_feature removed.
- Tests: ifunc-shared-equality widened; x86-64-tls-shared-{gd,ie} pass.
- x86-64-tls-shared-desc: passes with --no-plumb on all four; with the
  verifier every reference fails (lld/mold R1 = WP-144; bfd R1 + Y5
  `_TLS_MODULE_BASE_` expected 0xc found 0x0), mortar R1 only. The
  harness refuses markers on all three references: ASKED the coordinator.

## Next (waiting on the coordinator for desc marker and X-62 ruling)
- le-refused and cxx widened; x86-64-tls-ifunc-refused goes; census
  OUT_OF_M2 rows; rows.rs note + options.md; spec X-61/X-62; s1d --x86;
  static-identical; census; gates; record.
