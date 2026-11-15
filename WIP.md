# WIP — WP-050 (verifier stage 3)

## Done
- Step 0d (03d2154, on main as d3bde0d): map dynamic records.
- Stage-3 skeleton, runs clean on PIE hellos of bfd/lld (mold: A9, A3 only):
  - `dynout.rs` output dynamic tables; `line.rs` the link line (+ scripts,
    `Files`); `dynmodel.rs` D-11/D-12/D-19a model, `lookup`/`bind_in`;
    `loader.rs` record application (views), PLT decoder; `dynverify.rs`
    `verify_with` (views 1-3); `dyncheck.rs` static checks (STUB).
  - engine: `dynbind`, `dyn_tls`, `plt`, `abs_words`, `extent`
    (reloc.rs Engine), PLT hop -> `Via::Plt`; `Rule::Dyn(n)` = P-Dn;
    `Reason::Dynamic`; S1 accepts ET_DYN with PT_DYNAMIC.
  - xtask plumb `--line FILE` (one word per line), `verify_link_line`.

## Next
- dyncheck.rs: P-D1, P-D4..P-D8, P-D10..P-D13.
- Experiments: W1-W13, H1, F2, dupdef from /Volumes/AgentWork/mortar-helper/.build/review-m2/
  (scratch /tmp/mortar-wp050-182487, wrappers in wrap/ write $LINE_OUT).
- Committed tests (fixtures built at test time), mutations, cost.
