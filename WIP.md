# WP-110 work in progress (deleted in the final commit)

## Written
- record skeleton `spec/landed/dyn/WP-110.md` (sections A-E, appendix).
- experiment tree `.build/target-wp-110/exp/` with `run-all.sh WORK`,
  `PREAMBLE.txt`; areas `census`, `export`, `dyn`, `tools` (subagents),
  `design` (mine).

## Census plan
- A1: captured survey lines: `.build/real-builds/results/*/lines`, kind
  `shared`: 15 build lines (lua 10, googletest 4, zlib 1) + 3 configure
  probes (zlib `ztest*.so`, expat and gmp `a.out` C++ probes).
- A2: drivers gcc, g++, gfortran, clang 19/22, clang++ 19/22 x aarch64,
  x86-64, i386 x the brief's variations (area `census`).
- A3: real libraries rebuilt (zlib with zlib.map, lua test modules,
  googletest libgtest_dll), linked by the three references, run.
- A4: libtool-shaped (no libtool on host) and CMake BUILD_SHARED_LIBS lines.

## Verified
- (none yet)

## Learned, not yet written
- The survey lines are default driver lines + `-soname` (5),
  `--version-script` (1 build + 1 probe), CMake's `--dependency-file`
  (googletest; already supported). No `-Bsymbolic`, `-z defs`, rpath.
- gcc's `-shared` line: no `Scrt1.o`, no `-dynamic-linker`, no `-pie`;
  crti, crtbeginS, crtendS, crtn; same `-l` tail as PIE.
- arch.md §4.3 and a64.md §5 already have `Shared` cells (Pc/Preemptible
  `E`, TlsLe `E`, AbsWord `D`+`TX`); x64 X-? "recompile with -fPIC".

## Next step
- Wait for the four areas; meanwhile write the inventory (B) from the
  specs; then C rules into `spec/dyn.md` §17, resolve §9.6/9.7,
  script.md version scripts; D cut; E questions.
