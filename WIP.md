# WP-054 work in progress (deleted in the final commit)

Done (committed):
- STB_GNU_UNIQUE definitions of a dynamic output stay unique in `.symtab`
  and `.dynsym` (D-23 export row; `crates/mortar/src/syms.rs`
  `CoreSyms::dynamic`); static links unchanged (static-identical 0 differ).
- First chunk of an output section aligned to the section's widest chunk
  (L-5; `crates/mortar_layout/src/chunk.rs`): `.rodata` of `-static-libgcc`
  links was 8-aligned with `sh_addralign` 16.
- Tests: `tests/link/dyn/cxx/{unique-shared,exceptions-rtti,threads-tls,
  operator-new-replaced}.test`, `dyn/reloc/tls-gd-to-ie.test` (xfail:
  verifier request), `dyn/tools/debug-cxx.test`.

Left:
- Rerun the E-corpus matrix (/tmp/mortar-wp054-main/corpus/run.sh) at head.
- kiln+hod C++ hello through the default dynamic line (report only).
- eu-elflint on the hellos vs GNU ld.
- Gate `MORTAR_REQUIRE_TOOLS=1 cargo xtask ci --quick`.
- Record `spec/landed/dyn/WP-054.md`; delete this file; remove scratch.
