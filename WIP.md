# WIP — WP-028c (mortar_plumb for x86-64)

## Done
- S-17 piece lookup (supervisor's addition): `reloc.rs` `resolve_at` /
  `section_piece` (section symbol + A selects the piece; S-18 outside =
  `Target::Outside`, R6 under mortar); S-19 boundary (half-open, end only
  at sh_size) in `in_section` and `symbols.rs`; used by `record`, the
  tombstone check, `tls_target`, `eh.rs` pointers. Test `tests/pieces.rs`
  + corpus case `pieces` (fails with R1 without the fix). plumb.md §4.4.

## Half-done
- none

## Learned, not yet written
- lld refuses LD22 (X-27); reference byte forms per §8.8 still to confirm
  by objdump in /tmp/mortar-wp028c-main.
- Acceptance for S-17: after `git merge main`, remove the filter in
  `crates/mortar_image/tests/link.rs` (WP-027b) and prove it clean.

## Next
- Scope item 1: x86-64 `Def` table (`src/x64.rs`), machine dispatch at
  `reloc.rs` `width()` and the refusal in `section()`.
