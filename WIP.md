# WP-053 WIP (delete in the final commit)

Checkpoint 2026-11-15 ~15:15 (second agent). Briefs:
`briefs/m2-common.md`, `briefs/wp-053.md` (in the context repo).

## Done (first agent; see git log)
Guard lifted (AArch64 Exec/Pie), phase 80 in `crates/mortar/src/dynlink.rs`,
image wiring, CLI fold of AT_JOIN, targets 1a/1b.

## Done (this agent)
- Merged main d23258b (3c6c8d0); options.md regenerated (BLESS).
- (a) copy alias twice in .dynsym: FIXED. `mortar_synth/src/dynsym.rs`
  `copy_aliases` takes one key per shared definition (SymEntry), none for a
  definition a present key names; `one_entry_per_definition` refuses by name
  (unsupported feature) two program references (`foo`, `.symver foo@V`)
  that need a hashed entry. Test `dyn/dynsym/copy-two-names-refused`.
  Also turned copy-reloc / copy-typeinfo-relro (aarch64) green.
- (b) VARIANT_PCS: FIXED on imports (both rows) and exports
  (`CoreSyms::defined_other_flags`); D-23 rows amended. Test
  `dyn/dynsym/variant-pcs-export`. `.symtab` still drops it (static too):
  finding, not changed (static-identical).
- (c) versioned export: FIXED (refused): `CoreSyms::default_version` reads
  `foo@@V` from the defining entry.
- (d) tls-desc-large: NOT a wrong output: D-48 keeps the TLSDESC record for
  the large form, runs correctly; plumb P-D12 (plumb.md) lacks the case.
  Xfail re-tagged as a verifier request.
- (e) D-12 note: FIXED (`ScanInputs.closure_only`, report.rs `undefined`).

## Next
1. Target 3: `.build/wp053-scratch/corpus-c.sh` (c_main/c_helper of E-corpus,
   gcc/clang-19/clang-22 x 9 modes x O0/O2 x pie/no-pie, vs GNU ld, plumb).
2. Record `spec/landed/dyn/WP-053.md`; delete this file; full gate; hand back.
