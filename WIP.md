# WIP: WP-225 (U-10, the join)

Done: Step 0 (aa4ad63d). M1: e8e3daff, 4539fdaf, 51676760. M1b:
fdd8c19c. G-2: 9719929e. Batch 16 merged by the supervisor (931d1350;
builds, `--test incremental` green). M2a so far (record, "M2a"):
- aeaac86e G-2b: `capture::rows5`'s per-file closure -> `fn file5` (pure
  move); f3105bcc G-2a: `tables::index_file` `pub(crate)`.
- 95313541 step 1: `update::run(session, parsed, cwd, &Ctx) ->
  Result<Updated, IncrReason>` (Ctx = plan, store, policy, pool; lib.rs's
  `Start::Update` arm returns `Updated::lines` as the Invocation's
  `incremental`); `image_link(ctx, cwd, replay, go)`: the whole front over
  the new inputs into its own collector (any diagnostic -> `(diags)`),
  `go(&Front)`. New guards before the front: map/-M/--mortar-map/
  dependency file -> `(image)`; more than one changed file/path or a
  changed member -> `(resolve)`.
- ddb7e6ae `file_rows(.., copies)` fills `refs/commons/copies/groups`
  from `file5`.

NO update finishes yet: `run` still ends `(resolve)` inside
`image_link`'s closure (crates/mortar/src/update/mod.rs, `run`).

Next (M2a, in this order; every line inside the `image_link` closure):
1. U4: `Link` from the front (as floor.rs:423), `Keys::new`,
   `format::Strings` started at `view.count(kind::STRINGS)`;
   `capture::file_rows(&link,&keys,&f.layout,&mut strings,&[file],copies)`
   -> `ChangedFile` (`first_section` = the file's stored first section
   when its section count and keys are equal, else `(layout)`; `keys` by
   name from the symbol table; `index` via a new `pub fn` in capture
   calling `tables::index_file`). Check: front file `file` has the state
   row's path (else `(resolve)`).
2. U5 `extract`, `resolve` -> any `GlobalChange`/new key besides the
   file's own locals -> `(resolve)`.
3. U7 `delta::layout` with the file's `SectionChange`s; ANY of moved,
   killed, revived, holes, regions_opened, growth_steps, eh, frags,
   slots_* non-empty -> `(layout)`. Then check P_U == `f.layout`: every
   chunk of `delta.placement.chunks` equal to the front's, every section
   of the file `place_of` == `f.layout.placement.place[id]`; else
   `(layout)` (this is G-3 for the identity case: no replay needed).
4. `defined::recompute` (must be empty, else `(layout)`), `symtab_incr::plan`,
   U8 `refill_set`, U9 `with_link` + `refill(.., previous = session's
   output descriptor, leaves = view.leaves())` (model:
   tests/update_image.rs `run_refill_with`).
5. RULING ASKED (record "M2a"): the state. Either G-5's `StatePatch` +
   `plan_patch` + `state5::patch` as w5 (large), or for M2a the capture
   of path (A)'s front (P_U == its layout) written whole through
   `write_state_whole` as w5 (correct by construction; G-11's (d) then
   checks the META binding only).
6. U10 `write_with`; the `incremental: update ...` line; then the named
   test `first_update_same_size_body` and (d).

Resume: scripts in `/Volumes/AgentWork/mortar-helper/.build/target-wp-155/wp225/`
(`t.sh <log> <cargo test args>`, `gate.sh` whole gate (GATE-DONE),
`gq.sh` quick gate + incr-update (GQ-DONE), `c2.sh` fmt+clippy, `cost.sh`);
`bin/mortar-base` is the base's release build.
Do not edit `crates/mortar_image/src/`, `mortar_incr/src/state5/`,
`update/debug.rs`, `update/live.rs`.
