# WP-049 (M2-LAYOUT) work in progress

Deleted in the final commit.

## Plan
- mortar_core: `derive_segments` gains PT_PHDR + PT_INTERP (iff an `.interp`
  chunk) and PT_DYNAMIC (iff a `.dynamic` chunk); `SegmentRules.pie`;
  `Layout::check`: base 0 iff pie, `.interp` after the headers, `.dynamic` in
  RELRO when RELRO exists, copy placement (chunk, alignment, order, extent),
  copy chunks first in their section.
- mortar_layout: ranks of the dynamic tables (D-60), `.dynbss` joins `.bss`
  first, `.bss.rel.ro` copy chunk first; `-z norelro` ranks `.bss.rel.ro`
  as a writable NOBITS section (after `.bss`; lld measured); Options.pie
  (base 0); ELFCLASS32 dynamic refused; `_DYNAMIC`; copy sym_place.
- tests: `tests/dynamic.rs` table per kind x now x norelro; reference
  comparison against GNU ld and lld.

## Done
- core: SegmentRules.pie; derive_segments PT_PHDR/PT_INTERP/PT_DYNAMIC;
  check.rs `dynamic()`, `copies()`; core tests d42_copy_placement, d1_pie_base.
- layout: rank.rs (D-60 rows, home/leads, Z), chunk.rs (CopyRow,
  copy_rows, copy_extent, copy chunks), run.rs place_with_copies,
  target.rs (pie, ELF32 refused), address.rs (base 0), symbols.rs _DYNAMIC.
- static-identical: ok (167 links, 0 differ).

- tests/dynamic.rs: table (18 cases), d62_again, copy chunk size, L-5
  leading copy chunk alignment (chunk.rs post-pass + check rule), input
  .interp, reference comparison (32 links: lld classes equal, GNU ld
  phdr order equal; stated: AArch64 R in RX, lazy .got.plt head in RELRO).

## Next
- D-60 proof image: GNU ld script in D-60/D-61 order, run natively,
  eu-elflint, gdb, lldb (scratch).
- spec/landed/dyn/WP-049.md; merge main when WP-051 lands; gate.
