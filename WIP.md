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

## Findings so far
- .dynsym WeakDyn entry drops VARIANT_PCS and DT_AARCH64_VARIANT_PCS is missing for a JUMP_SLOT
  to it (GNU ld, lld write both): A2 commit (a2.patch), D-23 row text.
- B: mortar -r refused ("unsupported option: -r"): -r not in play.
- C: GNU ld refuses "failed to convert GOTPCREL relocation; relink with --no-relax" when the
  relaxed form does not fit; GNU keeps the GOT (no relax) for a target in SHF_X86_64_LARGE.

## Next
- commit A (after update + tests), apply a2.patch, D-23 text, test, commit A2.
- B: syms.rs keep_unique always; spec; tests; baseline.
- C: x64 scan: GOTPCRELX relaxed at scan (kind 1 + refusal when it does not fit and no slot);
  SymFacts.large; i386 GOT32X scan; spec X-19, X86-25; tests; baseline.
