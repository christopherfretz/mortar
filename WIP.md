# WIP — WP-050 (verifier stage 3)

## Done
- Step 0d (03d2154, on main as d3bde0d): map dynamic records.
- Stage 3: `dynout.rs` (output tables), `line.rs` (link line), `dynmodel.rs`
  (D-11/D-12/D-19a model), `loader.rs` (views, PLT decoder, TLS fake blocks),
  `dynverify.rs` (`verify_with`, views 1-3, exported IFUNCs), `dyncheck.rs`
  (P-D1, P-D4..P-D8, P-D10..P-D13); xtask plumb `--line FILE`.
- Dynamic TLS: imports are `Target::Tls` at `tls_fake`; AArch64 GD/desc kept;
  x64 GD->IE (GNU), desc->IE, GD/LD kept; exec TLS st_value is an offset.
- E1 matrix (6 TUs x pie/nopie x lazy/now x 3 linkers x 2 arches): all
  checked; only findings W3/F3 (x64 PIE bfd/mold), H1 (lld/mold), mold A-list.
- W1-W13, H1, F2, dupdef reproduced in /tmp/mortar-wp050-182487/w (build.sh);
  results table goes in the record.

## Next
- Committed tests `crates/mortar_plumb/tests/dyn.rs` (fixtures built at test
  time; sources inlined, not from .build/review-m2).
- Dynamic mutation classes + kill table; cost (C++ PIE, 1e6 RELATIVE).
- plumb.md stage-3 notes; record spec/landed/plumb/WP-050.md; gate.
