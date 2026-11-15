# WP-030b WIP (deleted in the final commit)

## Done
- Step 0 (interface): `RelocAt.addend: Option<i64>` (+ `RelocAt::rela_addend`),
  `SectionOut::input` with `with_resident`/`with_margins`; mechanical edits in
  a64/x64/scan/image and tests; arch.md R-1 (App. B), R-2 (§3.3), R-3 (§3.1),
  R-13 (§3.3, §3.4, §3.6).

## Half-done
- (none)

## Learned, not yet written
- image.md I-111's i386 values (R-3 first half) are image.md's: not edited
  (WP-032c is in image); for the record.

## Next
- Gate step 0 (ci --quick, background); report hash.
- Stage A: crates/mortar_x86 (table.rs, forms.rs, relax.rs, apply.rs, synth.rs).
