# WIP: WP-225 (U-10, the join)

Done: Step 0 (aa4ad63d). M1: e8e3daff, 4539fdaf, 51676760. M1b:
fdd8c19c (L10 and the IN-63a oracle on the testhooks build with
`MORTAR_INCR_STATE4=1`; `spec/harness.md`'s sentence), M1-3's byte
comparison (record, "M1b"). G-2: 9719929e (`capture::file_rows`, own
commit) + the clippy fix of its test. Gate results in the record.

M2 NOT started (handed back for context budget). Needs a ruling first:
G-2's INDEX and KEY_REFS rows (record, "G-2": `tables::index_file`
private; `rows5`'s per-file closure). Without it, U4's `ChangedFile`
lacks `index` (U-8's references) and the U-3 rows.

Next (M2, path (A), in this order):
1. `update::run(session, parsed, cwd)` (crates/mortar/src/update/mod.rs
   `steps()`, where it returns `(resolve)`): it needs the store, the
   collector, the pool and the timer to run the front: widen `run`'s
   arguments (mine; the `Start::Update` arm in lib.rs:359 is the join's
   hunk) and return either an `Updated` (the Invocation's pieces) or the
   reason.
2. `image_link`: `front::front(plan, Env { replay: Some(P_U hook), .. })`
   over the new inputs; the U4 adapter: `capture::file_rows` for the
   changed files -> `ChangedFile` (sections renumbered from the state's
   `SECTIONS` count, keys by name from `f.inputs` symbol names, strings
   appended to `ChangedFiles::strings` at `view.count(kind::STRINGS)`).
3. U5 `extract`, `resolve` (M2: a resolution change -> its own reason),
   U6 `live` (empty), U7 `delta::layout`, `defined::recompute`,
   `symtab_incr::plan`, U8 `refill_set`, U9 materialize P_U (G-3), the
   EhPlan (G-4), `mortar_image::update::refill`, `update_map` (+ `arg`
   records, G-7; `-Map`/`-M`/`--print-map` -> `(image)`), the
   `StatePatch` (G-5), `plan_patch`; U10 `write_with` with the w5
   closure. Fall-backs by name BEFORE the first write for all else.
4. The in-crate (d) `state_equals_capture_of_its_placement` after every
   hop; the named M2 tests (record 0.5).
Do not edit `crates/mortar_image/src/update.rs` (WP-255a),
`mortar_incr/src/state5/`, `update/debug.rs`, `update/live.rs`.

Resume: scripts in `/Volumes/AgentWork/mortar-helper/.build/target-wp-155/wp225/`
(`t.sh <log> <cargo test args>`, `gate.sh` (whole gate, final line
GATE-DONE), `gate2.sh`, `cost.sh`, `m1b.sh`, `bytes.sh`, `cmp.py`);
`bin/mortar-base` is the base's release build.
