# WP-030c work in progress (deleted in the final commit)

## Done
- 4681e52 spec rulings; 10e5f46 `PlaceFacts.exec`.
- Driver selects i386; REL in fill (`mortar_image/src/rel.rs`, dispatch in
  reloc.rs); `.rel.plt` names; ELF32 class for symtab/property/chunks.
- resolve: SHT_RELA in EM_386 (and SHT_REL in 64-bit) refused by name.
- X86-1 e_flags value shown (`RefName::Value`).
- mortar_live: REL addend selects the live merge piece (`LiveInputs.arch`).
- front: `_GLOBAL_OFFSET_TABLE_` named w/o reference -> GOT header;
  PT_TLS >= 2^31 refused on ELF32.
- tests/x86.rs (13 tests, matrix + refusals), linktest i386 32 PASS 1 XFAIL.
- docs/status/i386.md.

## In flight
- Identity of AArch64/x86-64 outputs vs merge-base build
  (/Volumes/AgentWork/mortar-helper/.build/target-wp-030c-base, built from
  `git archive 86d5439` in /tmp/mortar-wp030c-3761555/base; script
  /tmp/mortar-wp030c-3761555/bin/ident.py).
- Timing: 160-unit C++ program in /tmp/mortar-wp030c-3761555/big
  (i386-*.o, x64-*.o), `--time-phases`.

## Next
- Record spec/landed/x86/WP-030c.md; gate `cargo xtask ci --quick`;
  remove .build/target-wp-030c-base and /tmp/mortar-wp030c-3761555.
