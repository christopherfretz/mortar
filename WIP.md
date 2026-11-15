# WIP — WP-029b mortar_x64 (stage A)

## Done
- mortar_arch: `SymValue.tls`, `SectionOut.exec` + `with_exec`; tls set at
  construction sites (facts.tls in crates/mortar/src/sites.rs, mortar_image
  link.rs; false elsewhere with a stage B note).
- crates/mortar_x64 source: table.rs (30 M1 rows), forms.rs, relax.rs,
  apply.rs (scan/apply/relaxations), synth.rs (IPLT, static slots), lib.rs
  (Arch impl). Clippy clean, workspace check clean.

## Half-done
- Tests: none yet (crates/mortar_x64/tests/).

## Learned
- Spec findings list lives in the hand-back draft (see landed record when written).

## Next
- tests/support/mod.rs from a64's; table.rs, x64.rs, relax.rs, clip.rs, synth.rs.
