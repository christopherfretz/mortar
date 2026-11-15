# WIP — WP-028e (plumb i386)

Scratch dir: /tmp/mortar-wp028e-405572 (remove by explicit path at the end).
Logs: /Volumes/AgentWork/mortar-helper/.build/wp028e-logs/
Merges of main used: 1 of 3 (d9aecd2).

## Done
- 0 ModRM split; 1 table + record path; 2 TLS variant II; 3 IFUNC/IPLT
  (x86_iplt F5 + mortar exact form); 5 R-13 (b) overlaps (x86_rel_overlaps,
  R8) + I-47 ELF32 (already in debug.rs); 6 L3 i386 (98 PASS 1 XFAIL);
  8 W coverage (tests/x86.rs).
- 4: S1 EI_OSABI/e_flags (all) + mortar OSABI rule; S2 ELF32 2^32 end;
  mortar i386 p_align 0x1000, base 0x08048000, PT_GNU_STACK align 16.
- 7: mutate.rs class-aware (Lay), i386 forms, header-ident class;
  tests/x86.rs x86_mutation_coverage.

## Next
- Timing on /tmp/mortar-wp028e-405572/big/big.{bfd,lld,mold} (release).
- §9.5 i386 table in plumb.md; landed record; gate; hand-back.
