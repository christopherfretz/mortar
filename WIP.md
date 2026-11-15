# WP-027a WIP (paused for host migration)

Branch `wp/027a-front`, cut from main 0f4b08a. I have not run `git merge main`.
The facade lib and binary compiled at 393c759. The new test files
(`crates/mortar/tests/support/mod.rs`, `tests/front_diff.rs`) have never been
compiled. Delete this file before the WP lands.

## Done (committed)
- **S-6 join**: `mortar_resolve/src/sections.rs` (`placed_whole`, `MAX_MERGE_ENTSIZE`)
  and the demotion loop in `resolve.rs` `section_rows`. A Merge row is
  demoted to Regular when it must be placed whole.
  - Still to do: the spec text (synth.md S-6, resolve.md §10), the merge record and a test.
- **LayoutDiag**: `mortar_layout/src/diag.rs` now carries `key` and `at`
  (`on_chunk`).
  - `fixed.rs` `chunk_at`/`unproved_refusal` now key the 843419 unproved
    refusal at (90,0,chunk).
- **mortar_scan `Walker`** (`src/walk.rs`, new file only). It reuses the scan's
  view for branch sites and erratum words.
- **Facade**:
  - `crates/mortar/src/front.rs`: `front(plan, Env{cwd,pool}, &Store, &Collector, &mut Timer) -> Option<Front<'s>>`,
    the whole pipeline down to `Layout::validate`. It also re-runs `place`
    for the exact shstrtab size and for the R-L5 symtab-with-patches size.
  - `Front::sections() -> Vec<OutRow>`.
  - Helper modules: `sites.rs` (`LinkSites`, the `Sites` implementation for
    thunks and 843419), `syms.rs` (`CoreSyms: SymSource`), `proof.rs`
    (`A64Proof`), `timing.rs` (`Timer`, wrapping `mortar_image::phases::PhaseTimer`).
- **`lib.rs` `invoke`/`invoke_in`**:
  - The front runs when every refused option used is `-o`.
  - Option refusals are emitted after the front runs. They are errors, so emitting them earlier would trip the front's `has_errors` stop.
  - A successful front ends in `Refused{Feature,"writing the output file"}` at (100,0,0).
  - A bad `MORTAR_SCHED_SEED` gives `SEED_KEY` (10,0,0).
  - `--threads` sets the pool size.
  - `Invocation.phases` carries the `--time-phases` lines; `main.rs` prints them after the report.
- **CLI rows flipped to Supported** (`mortar_cli/src/rows.rs`): -m, -e, -s, -S, -x, -X, --build-id,
  --eh-frame-hdr, --no-eh-frame-hdr, --no-gc-sections, -l, --start-group,
  --end-group, --as-needed, --no-as-needed, -Bstatic, -L, --hash-style,
  --threads, --time-phases, --fix-cortex-a53-843419.

## Experiment results
- The GCC 14 `-static` C link line runs through the front to a validated Layout
  in about 144 ms wall (release build). Per phase:
  - input: 37 ms
  - resolve: 31 ms
  - split: 18 ms
  - synth: 13 ms
  - layout: 33 ms
  - peak: about 20 MiB
- Oddity: the `total:` line showed a peak of 19.3 MiB, lower than the layout
  line's 20.2 MiB, even though VmHWM should never fall. Not investigated.

## Half-done / broken now
- These tests are stale and fail. They expect the old "linking" refusal, its
  note text has changed, and `-o` with a missing a.o now reaches input:
  - `crates/mortar/src/lib.rs` tests: `a_link_is_refused_not_faked`,
    `requests_are_answered` (LINKING const), `errors_come_in_argument_order`.
  - `crates/mortar/tests/cli.rs` (both refusal tests).
- `crates/mortar_cli/tests/table.rs::statuses` must list the new Supported rows.
  The check "a mortar-own row is refused" must allow `--time-phases`.
  Regenerate `docs/status/options.md`.
- The differential harness (`tests/support/mod.rs`, `tests/front_diff.rs`,
  `#[ignore]` `explore_all`) was written but never compiled or run.
  - Next: `cargo test -p mortar --test front_diff -- --ignored --nocapture`.
  - Read the tables, explain each difference by a named rule, then turn them into assertions.

## Next steps (in order)
1. Fix the stale tests above and the statuses list. Each flipped row needs a test.
2. Run the differential: GCC C/C++ first, then Clang, then gfortran. Fix or record every difference.
3. Add a determinism test (`par::check_schedules`), refusal-text tests, and a bad-seed test.
4. Remaining joins: S-70 `__rela_iplt_*`, S-93, S-98, R-L*/R-M* spec marks,
   `.comment` S-100, `EXTENT_BACK`, s3_target tests, rows.rs:103, `--build-id=0x`.
5. Timing at 1/4/8 threads against mold/lld. Gate: `MORTAR_REQUIRE_TOOLS=1 cargo xtask ci --quick`.
6. Landed record `spec/landed/pipeline/WP-027a.md`. Clean up `.build/scratch-wp027a`.

## Session 2 (taken over after migration; paused again for a host restart)
- Did `git merge main` (239aeba) — merge 1 of 2 used. Clean textually.
- Fixed the join in `crates/mortar_scan/src/walk.rs`: `Sec::open` now takes
  `&mut Bufs`; `Walker::walk` runs `walk_in` inside `with_bufs`.
  `cargo build -p mortar --all-targets` is green after that.
- Found (not yet fixed), WP-021b violations in the facade:
  - `crates/mortar/src/lib.rs:180` uses `StorePolicy::Resident`; must be the
    default (Staged).
  - `crates/mortar/src/sites.rs` `code()`: `s.data().ok()?` and
    `walker.walk(..).ok()?` turn a read failure / non-resident `.text`
    (>64 KiB is NOT resident under Staged) into "no erratum sites". Plan:
    change `mortar_layout::fixed::Sites::code` to return
    `Result<Option<OwnedCode>, LayoutDiag>` (update NoSites and
    `crates/mortar_layout/tests/fixed.rs:86`), add a `Walker` method that
    reads section contents via scan's `contents` (copy_to on NotResident)
    into a caller buffer, and propagate errors.
  - `front.rs` `property_plan`: `if let Ok(data) = s.data()` silently skips;
    must report (SHT_NOTE is resident; NotResident there = internal error).
- Next concrete step: write `spec/landed/pipeline/FRONT-INTERFACE.md`
  (the `Front<'s>` struct in `crates/mortar/src/front.rs:97` is the value;
  field types: merge `Phase` (mortar_merge/src/phase.rs:83), `Order`
  (order.rs:26, front keeps `order.map` in Layout and `order.chunks` as
  `merged`), `Scanned` (mortar_scan/src/lib.rs:201), `Placed`
  (mortar_layout/src/run.rs:45), `Layout` (mortar_core/src/layout.rs:831)),
  commit, then the fixes above, then WIP's earlier "Next steps".

## Session 3 (third agent)
- d0e347e: FRONT-INTERFACE.md published (spec/landed/pipeline/); `Front.merged`
  replaced by `Front.order: Order` (image needs `order.order`); facade store is
  `StorePolicy::from_env()` (Staged default).
- Fixed: `Sites::code` now `Result<Option<OwnedCode>, LayoutDiag>`; sites.rs
  reads `.text` through `Walker::section_bytes` (copy_to when not resident);
  `property_plan` reports an unreadable note (malformed / internal).
- Stale tests fixed (lib.rs, tests/cli.rs, mortar_cli grammar + table,
  docs/status/options.md regenerated). The gcc -static link reaches
  "writing the output file" under the staged store.
- 485db0a: differential test `tests/front_diff.rs` (`compare`, 4 tests,
  green under both stores). Fixed on the way: `.symtab_shndx` only past
  SHN_LORESERVE (front.rs `shndx`); hidden undefined keys have no symtab
  entry (syms.rs; synth.md S-75); locals of consumed sections dropped
  (`syms::LinkSections`; S-76); S-78 default = drop `.L` only in merge
  sections (mortar_synth symtab.rs `local`; tests updated).
- Explanations found (for the record): .text/.data/.tbss sizes = same input
  set as GNU's map, order by input §10.1 + L-53 (padding); .got = lld Addr
  slots + 21 IE slots (a64 R3 is M2, S-72); .got.plt S-67; .comment/.debug_str
  S-23 (no tail merge, = lld); .rodata ±0x20 vs lld = S-24/S-25 group order
  (lld merged total 0x6fd5 vs ours 0x6ff0); .relro_padding L-64; segments L-65.
  Symtab = lld − section syms (S-79) − lld LOCAL UND (S-75) − .gnu.warning
  locals (L-33) − ≤2 eh $d (S-76); undefined weak binding: refs write GLOBAL
  from a lazy archive def, we write WEAK (S-121).
- Next: >64 KiB debug-reloc staged test; determinism; refusals; joins; timing.

## Session 4 (same agent, after a context compaction)
- 2f34523: staged-limit test (`sections_over_the_staged_limits`) and
  `the_front_does_not_depend_on_the_schedule` (check_schedules, C++ -O2 and
  the big -O0 -g program: Layout + sections + symtab + report identical).
- Next commit: `tests/refusals.rs` (exact texts: --gc-sections, undefined
  entry, bad seed, x86-64/i386/mixed machines, --time-phases rows);
  `the_supported_rows_shape_the_front` (-s, -S, -x vs lld -x, hdr, build-id,
  --no-gc-sections, -e). Fixed on the way: the entry warning came twice
  (resolve + layout; front.rs keeps resolve's; resolve.md §5.5 = layout
  §20.7 applied); `-S`/`-s` kept `.debug_str`/`.debug_line_str` (Merge
  kind; resolve sections.rs `is_debug`; resolve.md §10 row); cli stripped
  `0x` from `--build-id=0x…` while synth expects it (every 0xHEX failed);
  now cli checks I-70 itself (`0x` case-sensitive, even digit count; GNU
  ignores `0X`, lld errors, mold accepts); machine refusal at key
  (20,MAX,0) named `x86-64`/`i386`; clippy clean workspace-wide.
- Next: FRONT-INTERFACE changes (syms pub, with_sym_cx, LinkSections),
  timing at 1/4/8, par spawn share, remaining joins, record, gate.
