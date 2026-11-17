# WIP — WP-121 part 2 (plumb stage 3, shared objects): hand-over

Branch `wp/121-plumb-shared`, worktree `mortar/.worktrees/wp-121`.
Briefs: `briefs/m4-shared-common.md`, `briefs/wp-121.md` (workspace root).
Part 1 (P-D16..P-D22, P-D25, P-D26, the interposer view) is merged and
recorded in `spec/landed/plumb/WP-121.md` §1-§5. Every commit below up to
cede841 is merged into the local `int/batch-5` (coordinator's merges
fcef136, 6dbd54c, and the cede841 merge). Those merges delete this file:
expect it absent on int/batch-5.

## Part 2: state per item

1. **SharedLine takes the `-B` family from `Line.symbolic`**: DONE
   (0f709f7). `DT_SYMBOLIC`/`DF_SYMBOLIC` constants stay in shared.rs
   because `dynverify::SYMBOLIC` is a message string. Coordinator extra (a):
   the A44 resolution in `judge` was checked and is correct.
2. **P-D4's RELRO end on the line's max-page-size**: DONE (3022ad1).
   `dyncheck::pd4(image, out, line, map, v)` takes maxpage from
   `crate::opts::of(map)` (mortar maps); spec P-D4 text updated. Test:
   `tests/shared.rs p_d4_relro_ends_on_the_lines_maxpage` (case `relro`:
   AArch64, `-z max-page-size=0x1000`; ends bfd 0x2000, lld 0x3000,
   mold 0x3000).
3. **P-D17's address clause + `src/sharedflow.rs`**: DONE (f7508cb), not
   yet in the record.
   - D-117's relocation-level rule (exact fields; x86 only absolute,
     GOTOFF, PC in data, no decoder; the boundary rule; "before the
     start") and the AArch64 def-use analysis (D-117 items 1-9: functions,
     $d/$x, register states, ADD/ADR/LDR literal defining forms, calls kill
     x0-18/x30, leaders and BR, greatest fixed point with TOP for
     uncomputed predecessors, MAX_PASSES=64).
   - Entry point: `sharedflow::address_clause(&objs, sets) -> (Vec<Finding>, u64)`,
     called from `dynverify.rs` in the Shared branch after `static_checks`.
     `pub(crate) fn scan(u: &dyn Unit, preemptible) -> Scan`.
   - Messages: "the address clause (D-117): {reloc} at {sec}+{off} of {obj}
     names {T}+{a}, inside the preemptible '{def}' ..." and
     "the address clause (D-117 analysis): {func}+{at} of {obj} accesses|computes ...".
   - Unit tests in sharedflow.rs: `d117_flow_classes`,
     `d117_load_store_forms`; ignored `oracle_lists` (env SHAREDFLOW_LIST,
     SHAREDFLOW_OUT, SHAREDFLOW_X86) prints the per-object verdicts.
   - **Compared with `tests/shared-design/exp3/s1d.py`** (run from its own
     directory), identical output (every object, hit, pre, words, funcs):
     sys.lst 11,699 members (pre=51, rule_objs=26, ana_objs=28, rule=267,
     ana=188, words=123,763, 37 objects refused; mine 0.09 s test time,
     586 MB RSS for the cargo run; oracle 0.59 s); rebuilt corpora
     author-pic 214 objects/0 refused (words 381,784), author-pie 104/6,
     cx-a64 170/1, adv-fpie 24/2, sup+gap 29/4; x86 x-sys 4,050/0,
     x-distro 5,184/3. Two disagreements found and decided from the spec
     (getsgnam.o, sarray.o: TOP for uncomputed predecessors; brk.o alias
     name: symbol-table order). NOT compared: abseil, busybox, arpack and
     the x86 author corpora (not rebuilt); census objects (sources gone).
     Scratch with build.sh, corp/, lists, oracle.*.txt, mine.*.txt, cmp.py:
     `.build/target-wp-121/s1d/` (81 MB).
4. **P-D23's mutation table**: DONE (6d59174). Rows in `tests/shared.rs`
   (`p_d23_rows`, `p_d23_w28_is_not_caught`, `p_d23_i386_is_outside_stage_3`).
   HOLE found and fixed in the same commit: `host_import` read the output's
   .dynsym/.gnu.version (40454ce, merged in int/batch-5), so W23 (lld) and
   W42 (mold) passed; now from `Sets.imports` (+`versioned`) and L-96's
   `start_stop()` in shared.rs; `Sets.dropped` -> P-D17 finding. P-D8 small
   edit in dyncheck.rs (strong .symver ref unversioned, no definer in order).
   W21 sources fixed (address in code); W22 new (mold link); spec edits
   P-D17, P-D18, P-D23 sentence + rows W19 W21 W22 W23 W34 W38 W40 W42.
5. **§8.1 sizing**: DONE (record §8): cxx-large-g as if -shared, 3,499
   objects, 2.49 s, 308 MiB peak, pre 0 (bench built -fdata-sections).
   Oracle also compared on abseil, arpack, busybox, x86 author corpora:
   all agree (record §6 table). Next: commit, merge int/batch-5, gate.
6. **P-D17 "non-alloc not checked" vs §5.8**: re-read; they agree (§5.8 gives
   0 for Shared: no copy, canonical PLT only ET_EXEC; P-D17 says so since
   part 1). Host imports (no shared definer) fall to §4.4: no edit.

## Tonight's priority fixes (all merged into int/batch-5; NOT yet in the record)

Each needs a paragraph in `spec/landed/plumb/WP-121.md` (rule, cause, test):
- 40454ce (1) P-D17 view A / P-D9 + P-D7: an unversioned import no shared
  input defines is the host's without `-z defs` (D-118);
  `nonpic-analysis-format`. (2) Y4: an output local in a non-alloc section
  is unchecked (GNU ld keeps Clang's `.comment` `$d` where the merged
  empty string lands); `nonpic-analysis-before-start`; edit in
  symbols.rs. (3) P-D22: DT_FLAGS_1 NODELETE/NOOPEN/INITFIRST/INTERPOSE/
  ORIGIN iff the `-z` word (D-103); `refused-shared-later`. (4)
  `SharedLine` gains `z`, `zdefs`, `flags_1()`. Tests: tests/shared.rs
  `s12_5_reference_link_tests_verify_clean` (case `refs`),
  `p_d22_flags_1_follows_the_z_words`.
- 4ce3cc2 (A) P-D4 (WP-131 V5): the line's kind alone owes PT_DYNAMIC from
  mortar only; a reference's `-no-pie` line without shared inputs is
  static unless PT_INTERP/.dynsym/.dynamic/dynamic record/a loaded .so;
  lld's and mold's PT_INTERP-without-PT_DYNAMIC exec crashes (measured
  natively on AArch64) and stays P-D4, `reloc/x64-gotpcrelx-slot.test`
  gained `xfail(lld,mold)`. Test: tests/dyn.rs
  `s12_p_d4_reference_without_shared_inputs` (case `nostd`). (B) P-D22:
  mortar's `.dynamic` in D-102's order (`D102_ORDER`, `d102_order()`),
  D-30's order no longer judges Shared; mortar's libt.so (WP-123) = GNU
  ld's order, 0 findings. Test: shared.rs unit `p_d22_d102_order`.
- cede841 V5: P-D8 skips a weak import whose definer is not DT_NEEDED
  (`cxx-weak-only-libc`). V3: P-D5 lookup by name + version index
  (`dynout::gnu_lookup_by`), P-D7 any global of the name at the value,
  P-D8 VERSYM_HIDDEN allowed on Verdef-indexed definitions in Shared
  (`hidden_definition`), new P-D22 "two definitions without
  VERSYM_HIDDEN". V4 = W20 (mold). Tests: `refs` gains libsv.so,
  libwo.so; `p_d22_hidden_version_is_kept`.

## Record `spec/landed/plumb/WP-121.md`: what it lacks

§6 "Not done (part 2)" must become part 2's sections: the address clause
and sharedflow (design, the s1d.py numbers above, the two decided
disagreements, what was not compared), P-D23's rows table with NOT-caught
rows, §8.1 numbers, the eight fixes above, spec edits by id (P-D4, P-D5,
P-D8, P-D17 view A, P-D22 x3, Y4), files touched outside the list
(dyncheck.rs pd4/pd8/pd7/pd5 small edits, dynout.rs, symbols.rs, the
x64-gotpcrelx-slot.test marker), and the hand-back items of the brief
(what the first mortar library exercises that references could not).

## Open items of part 1's record still open

- dyncheck's mortar-format D-30 rows vs D-102 (order done in 4ce3cc2;
  the "6 DT_NULL" and other mortar-only D-30 rows not re-checked for
  Shared).
- WP-123 will forward more stage 3 findings on mortar libraries: decide
  each from the spec (rule too strict / reference anomaly A46+ / expected
  row), a test that fails before, whole linktest lane 0 FAIL.

## Commands (from the worktree; never set CARGO_TARGET_DIR yourself)

    unset CARGO_TARGET_DIR; source scripts/env.sh   # target .build/target-wp-121
    MORTAR_REQUIRE_TOOLS=1 cargo test -p mortar_plumb --test shared            # rows, refs, forms
    MORTAR_REQUIRE_TOOLS=1 SHARED_DEBUG=rows cargo test -p mortar_plumb --test shared p_d23
    MORTAR_REQUIRE_TOOLS=1 cargo test -p mortar_plumb --lib --test dyn --test x64 --test x86 --test spie --test tls
    cargo clippy -p mortar_plumb --all-targets -- -D warnings
    MORTAR_REQUIRE_TOOLS=1 cargo xtask linktest [--filter dyn/shared]   # whole lane ~100-160 s, background
    .build/target-wp-121/debug/xtask plumb X.so --map X.map --map-format gnu|lld|mold|mortar --line line.txt
    MORTAR_REQUIRE_TOOLS=1 cargo xtask ci --quick > log 2>&1   # gate, background, want [ci] ok, 0 differ

## Traps met

- `cargo fmt` turns `src![...]` lists one-per-line: a python string
  replace on the old one-line form silently does nothing (sv.c was "not
  found" in every test).
- Rows WIP lived in tests/shared.rs while priority fixes also edited it:
  split with `git diff > patch; git checkout file; ...commit...; git apply -3 patch`.
- Compilers/assemblers remove the form a row needs (W19, W33, W34): check
  `readelf -r` of every row object.
- `known_anomaly` rows in `judge`: a mold library always needs "A3"
  (`row()` adds it); GNU ld's `libflags.so` gives A10.
- The rows judge requires each expected id to be FOUND, not only allowed.
- A64 `rd=31` in decoded instructions: guard every register write.
- `xtask plumb` on a mortar `-shared` map from WP-123 gives M9 for
  `-shared`/`-soname` until WP-123's opts.rs (ACCEPTED rows) lands.
- Monitor/`sleep` limits: run long things with run_in_background and wait
  with `timeout N bash -c 'until grep -q ^EXIT log; do sleep 10; done'`.
- Scratch only under `.build/target-wp-121/` (y4/, v3/, v4/, b123/, nodyn/
  hold tonight's reproductions; wip/ holds patches).
