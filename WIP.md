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

- 70c18070 + 61f63b23: THE FIRST REAL UPDATE (ruling M2a-1 (b)): in
  `run`'s `image_link` closure, `reference_of` (path check; refill of the
  file's sections + every non-body chunk; the reference of (d) = capture
  of the front with the refill's leaves/build id; canon compare with the
  stored state, META/STAT/LEAVES/one FILES row aside, else `(layout)`)
  and `write_identity` (`write_with` in place, W5 = the reference whole).
  U1 opens the output read-write when it can. `--incremental=update`
  writes in place at every size. Test `first_update_same_size_body`
  green on AArch64 and x86-64.

Deviations to state (record): no `delta::layout`/`resolve`/`refill_set`
call yet: the canon comparison stands in for them (stronger than the
placement comparison); a W5 I/O failure maps to `output-changed` and the
driver then links in full (IN-5 says the link fails).

Next (in order):
1. The lane scenario the ruling asks for: one `incr-update` scenario, a
   same-size body edit of one object, AArch64 + x86-64 (tests/incr-update/;
   not read yet).
2. Call the packages' steps in the identity case (`extract`, `resolve`,
   `delta::layout`, `refill_set`) and keep the canon check.
3. A W5 failure: fail the link (IN-5), not a full link.
4. M2b (sizes within slack, the patch G-5).

Resume: scripts in `/Volumes/AgentWork/mortar-helper/.build/target-wp-155/wp225/`
(`t.sh <log> <cargo test args>`, `gate.sh` whole gate (GATE-DONE),
`gq.sh` quick gate + incr-update (GQ-DONE), `c2.sh` fmt+clippy, `cost.sh`);
`bin/mortar-base` is the base's release build.
Do not edit `crates/mortar_image/src/`, `mortar_incr/src/state5/`,
`update/debug.rs`, `update/live.rs`.
