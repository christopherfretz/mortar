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
- programs at ea4a0e1 (mortar-ea4a0e1): 58/58 hold, shipped archives,
  plumb clean unfiltered (39.5 s at -j 2). Variant libcxx-stripped ->
  libcxx-g (shipped).
- typical link (vector push_back.pass.cpp, $R/typical, bench.sh 15,
  load ~16): median wall bfd 53.7, lld19 42.9, lld22 45.7, mold 36.6,
  mortar 156.0 (default threads=12), mortar --threads=1 56.8 ms; RSS
  31.8/77.8/79.8/8.6(parent)/32.3/29.5 MiB. --time-phases: t1 total
  ~38-42 ms (input 27, image 4, resolve 3, layout 3); default 50-79 ms
  (input 26, layout 17, split 8, resolve 7, image 7, scan 6, synth 6).
