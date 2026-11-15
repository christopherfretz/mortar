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

- V2 (item 3): x64rec.rs accepts the kept-IE `mov` into any register;
  case `tlsdesc-regs` (15 registers run under qemu, rsp linked only).
- Item 2: `sweep.rs` (`xtask plumb --sweep`), dynout robustness
  (bloom_shift shift, bloom_size allocation), the closes in
  `dyncheck.rs` `fields()` (P-D5..P-D10), Verneed shape (dynout),
  AArch64 PLT decode requires `br x17` (loader.rs). Re-sweep: `sx-*`.
- Item 4: P-D12 accepts the kept large-model TLSDESC record exactly
  (`a64_large_desc`); xfail line removed; linktest passes.

## Half-done
- Item 5 (uncommitted): `reloc.rs` `patch_stubs`, `erratum.rs` fills it,
  `symbols.rs` judges `__CortexA53843419_<site>`, `corpus.rs` no longer
  allows them. Needs build + corpus test.

## Learned
- All three references write the full lazy shape and lazy words under
  `-z now` too (x86-64 and AArch64); GOT[1..2] zero.
- Stage 2's `phdr-flags`/`phdr-align` classes mostly survive on dynamic
  outputs (36/42 flags missed on a GNU ld x86-64 hello PIE): item 2 row.

## Next
- Item 5 test; item 6 (tlsgd-to-ie a64, GNU_UNIQUE, S1 EI_OSABI);
  plumb.md §12.2/§12.3 text; cost before/after; record; gate.
