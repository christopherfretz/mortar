# WP-127 part 2 (x86-64 TLS and IFUNC in Shared) — work in progress

Brief: `/Volumes/AgentWork/mortar-helper/briefs/wp-127b.md`. Scratch:
`.build/target-wp-127/p2/` (m/: models x linkers; sec/: X-62 TLS ruling).

## Done
- Measurement of GD/LD/gnu2/IE/LE x bfd/lld/mold (p2/m/run.sh).
- Ruling probe: named local TLS symbol: all three agree (r_sym 0 + offset);
  a section-symbol TLSGD: bfd links (addend = PC tail only), lld refuses,
  mold asserts. Not a producer form.

## Next
- mortar_x64 tls_scan/apply in Shared; shared_feature; IFUNC; tests.
