# WIP — WP-082 (verifier requests of the M2 joins)

Brief: `/Volumes/AgentWork/mortar-helper/briefs/wp-082.md`. Scratch:
`/tmp/mortar-wp082-162509` (reference corpus `r1/` hello C/C++ × arch ×
linker × pie/no-pie × lazy/now with `.line` files, `r2/` IBT/BTI shapes;
`mk.sh`, `judge.sh`, `mut.sh`).

## Done
- V1 (item 1): `crates/mortar_plumb/src/plt.rs` (new): symbolic walk of
  each JUMP_SLOT entry's lazy path (x86-64 and AArch64), checks the
  resolver's inputs (index, `.got.plt[1]`, jump via `.got.plt[2]`, x16),
  `.got.plt[0..2]`, BTI/IBT landing pads; replaces the old lazy-word
  check in `dyncheck.rs` pd10. Mutation classes `plt0-disp1`,
  `plt0-disp2`, `plt-index`, `plt-back-jump` (dynmutate.rs), `lazy-slot`
  now also under `-z now`. Tests `PLT0` and `shapes` cases in tests/dyn.rs.

- V1 kill table over r1 (all candidates): every new class 100% caught on
  both architectures and the three references; survivors only
  `lazy-slot` on mold x86-64 (lazy word + 4 skips the `endbr64` of
  mold's PLT0: equivalent when the output does not claim IBT, and
  P-D10 then requires the pad). Fixed a harness panic (`iplt-entry`
  slice on mold x86-64 entries).

## Half-done (uncommitted, kept in /tmp/mortar-wp082-162509/keep)
- item 2: `sweep.rs` (byte sweep, `xtask plumb --sweep`), dynout.rs
  robustness (bloom_shift shift, bloom_size allocation).
- item 3: x64rec.rs accepts the kept-IE `mov` into any register.

## Learned
- All three references write the full lazy shape and lazy words under
  `-z now` too (x86-64 and AArch64); GOT[1..2] zero.
- Stage 2's `phdr-flags`/`phdr-align` classes mostly survive on dynamic
  outputs (36/42 flags missed on a GNU ld x86-64 hello PIE): item 2 row.

## Next
- Restore keep/ files; item 2 closes (see the sweep tables in the
  scratch `sw-pie/`, `sw-nopie/`, `agg.py`).
