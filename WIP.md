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
- bfd full: done 07:33 (xtask-bfd at 1d578d4 + lane timing), out
  $R/full/bfd, collected to $R/full/bfd.tsv: 9247 tests, 8609 pass,
  5 fail (root-permission filesystem tests), 46 xfail, 2 xpass, 584
  unsupported, 1 timeout (offset_range); wall 2633.6 s; link sum 2802.7 s,
  median 336.6 ms, median RSS 31.1 MiB.
- The first chain waited with pgrep -f and matched its own waiter: 5.5 h
  lost (supervisor killed both 13:20). Now: chain.sh runs the commands
  directly as one background task.
- Merged main 390e705 -> da61bbc (merge 1 of 2). Full mortar run (plumb,
  -j 6) on mortar-da61bbc started 13:21, then lld+mold shards 1/6, 2/6
  ($R/chain.sh, $R/chain.log).
- Triage run of the 5 bfd failures in dyn/bfd/mortar, --keep: $R/triage.
- programs at ea4a0e1 (mortar-ea4a0e1): 58/58 hold, shipped archives,
  plumb clean unfiltered (39.5 s at -j 2). Variant libcxx-stripped ->
  libcxx-g (shipped).
- typical link (vector push_back.pass.cpp, $R/typical, bench.sh 15,
  load ~16): median wall bfd 53.7, lld19 42.9, lld22 45.7, mold 36.6,
  mortar 156.0 (default threads=12), mortar --threads=1 56.8 ms; RSS
  31.8/77.8/79.8/8.6(parent)/32.3/29.5 MiB. --time-phases: t1 total
  ~38-42 ms (input 27, image 4, resolve 3, layout 3); default 50-79 ms
  (input 26, layout 17, split 8, resolve 7, image 7, scan 6, synth 6).
- Supervisor (843419 fix on main cc4efaa): the libc++ lane's link line
  (clang++ driver via kiln, $R/linkline.txt) has NO
  --fix-cortex-a53-843419 (the run uses the merged tree anyway, at the
  supervisor's request). The
  g++-driver programs cells DO carry it (g++ -### shows it): plan = merge
  main once later, re-run programs with the merged binary, and time the
  typical link + 2 shards with/without MORTAR_HUGEPAGES=1.
- gate: ci --quick ok at 06d57ff.
- bfd's 5 FAIL: same 5 FAIL in dyn and mortar ($R/triage); every binary
  (bfd's and mortar's) passes run as uid 65534 (setpriv) -> root in the
  container defeats the permission checks: environment, not toolchain.
  offset_range TIMEOUT: writes a 4 GiB file 8 KiB at a time at -O0 (test
  cost). XPASS x2: verify tests (kiln diagnostics). stats.jsonl: 7207+17
  hod, 3 llvm (all hod_skip opt-level), 1 obj with no backend = a compile
  error (path.io.unicode_bug); no hod declines.
- typical link at da61bbc (bench2.sh 15, load ~7): median bfd 37.3,
  lld 32.5, mold 24.7, mortar 34.3 (default threads), --threads=1 36.6,
  MORTAR_HUGEPAGES=1 30.9 / t1 31.6 ms; RSS bfd 31.8, lld 77.6, mold 8.6
  (parent), mortar 32.4 / t1 30.0 / hp 36.2 / t1-hp 34.3 MiB.
  --time-phases default: total 37.6 (input 26.0, image 4.1, resolve 1.9,
  layout 1.3, split 1.2, synthetic 1.0, scan 0.8); t1 55.2 (input 44.5).
- programs at da61bbc (843419 cells via g++ carry the flag): 80 cells,
  80 hold, plumb clean ($R/programs-da61bbc.txt).
