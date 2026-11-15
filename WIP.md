# WP-034 work in progress (deleted in the final commit)

Run dir: /tmp/mortar-wp034-1178167 (`.rundir`), binaries in its `bin/`.

## Item 1 (.deplibs)
- References measured (objects and archive members): GNU ld 2.44 and
  mold 2.37 ignore the specifiers, copy `.deplibs` (non-alloc) to the
  output; a symbol only the named lib defines stays undefined. lld 19.1.7
  and 22 load the library (as -l in -L dirs), error `unable to find
  library from dependent library specifier: X` when missing;
  `--no-dependent-libraries` = mortar. GNU ld/mold have no such option.
- Done: resolve consumes it; refusal names known types; rules.rs row;
  tests/llvm_sections.rs; tests/link/resolve/{deplibs,deplibs-undefined,
  aarch64-attributes}.test; resolve.md §10 paragraph; cli row noop.
- lld 22 turns aeabi_feature_and_bits BTI/PAC into a GNU property note
  (verifier P1 flags lld's output) — report.

## Runs
- bfd full: started 06:49 (xtask-bfd at 1d578d4 + lane timing), out
  $R/full/bfd.
