# WP-059 work in progress (removed in the final commit)

Scratch: /tmp/mortar-wp059-a (A/, B/, C/ experiment scripts; mortar-old = main's binary;
bin/ld -> it; keep/keep-*.sh = --mortar wrappers that keep outputs; tools/cmp.py the comparator;
a2.patch = the .dynsym WeakDyn fix, re-applied after commit A).

## Done
- Measured A (AArch64 static/PIE/static-pie), B (3 targets, static/PIE/static-pie/-r, OSABI),
  C (x86-64 4 kinds, i386 static). Tables to go in spec/landed/synth/WP-059.md.
- A code: resolve flag VARIANT_PCS (bit 10) -> RefFlags::VARIANT_PCS (bit 6); syms.rs writes
  defining entry's `st_other & !3`, undefined: VARIANT_PCS if a ref has it (AArch64).
  spec synth S-127 + §13 row, resolve §3.7 paragraph. Tests resolve/symtab-variant-pcs,
  dyn/dynsym/symtab-variant-pcs. static-identical: 70 aarch64 links change, each exactly two
  .symtab entries (libgcc __arm_tpidr2_save, __arm_za_disable: other 0x2 -> 0x82) + build-id.

- A2 committed (15de784): WeakDyn .dynsym bit + DT_AARCH64_VARIANT_PCS; test dyn/dynsym/weak-undef-variant-pcs.
- B: syms.rs keeps STB_GNU_UNIQUE in every kind (CoreSyms::dynamic removed); spec resolve §3.7
  paragraph, synth S-75 cite, D-23 sentence; tests resolve/symtab-unique(+ -debuggers). aarch64
  baseline: 36 links change (.symtab info 0x11->0xa1 + build-id; OSABI already 3), 7 rows added.
  Changed links verifier-clean (linktest incl. tests/identical/{clang-cxx,cxx-large,gc-debug}).

## Findings so far
- -nostdlib static link with a .bss-only writable PT_LOAD: verifier S17 on mortar x86_64
  (.comment inside the PT_LOAD's (empty) file image; same with main's binary) and S2 on GNU ld
  aarch64. Not B's; for the record.
- .dynsym WeakDyn entry drops VARIANT_PCS and DT_AARCH64_VARIANT_PCS is missing for a JUMP_SLOT
  to it (GNU ld, lld write both): A2 commit (a2.patch), D-23 row text.
- B: mortar -r refused ("unsupported option: -r"): -r not in play.
- C: GNU ld refuses "failed to convert GOTPCREL relocation; relink with --no-relax" when the
  relaxed form does not fit; GNU keeps the GOT (no relax) for a target in SHF_X86_64_LARGE.

- C x86-64: stop condition met (relaxation is kind 2: fit is an address fact); built as 9ab85d9,
  reverted 711767d; options in the record. X-19 paragraph + test reloc/x64-gotpcrelx-slot.
- C i386: GOT32X that relaxes (kind 1) demands no slot at scan; X86-25 paragraph; test
  reloc/x86-got32x-slot; baseline: 68 i386 links change (.got + knock-on), 7 rows added.

## Next
- record spec/landed/synth/WP-059.md (draft, uncommitted); merge main; gate; hosted run for
  x86_64 baseline rows; remove WIP.md; rm -rf /tmp/mortar-wp059-a.
