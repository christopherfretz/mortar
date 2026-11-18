# WIP: WP-225 (U-10, the join)

Done: Step 0 (record "Step 0"); rulings in
`/Volumes/AgentWork/mortar-helper/briefs/wp-225-rulings.md`.

M1 (in progress, on this branch):
- e8e3daff G-13: `mortar_incr::testhook::small_output_bound`
  (`MORTAR_INCR_SMALL_OUTPUT`, testhooks only).
- the flip (uncommitted while its tests go green): format 5 at `S`
  (`incremental.rs` `write_state`/`state5_bytes`), U1 on format 5
  (`check`: the flag first, `state5::open`, the mortar version,
  `output_matches5` through P's descriptor, `boot_check`), `small-output`
  with the knob, `Start::Update` -> `update::run` (U2 `detect`, U3, the
  debug condition, `(live)`, then `(resolve)`), `lib.rs` handles it.
  Format 4 only: the side state `<S>.format4` (`testhook::state4`,
  `MORTAR_INCR_STATE4=1`; `mortar::invoke_state4`), and the legacy mode
  (`mortar::invoke_legacy4`, `replay_link`: no format-5 data, no IN-34a
  pads) for the format-4 replay's tests. The floor reads/writes the side
  state. lane.py: one line (knob + side state).

Next: tests green (crate tests: incremental, incr_review, update_write,
state5_capture, streams), then `cargo xtask l10 --quick` (L10's release
subject loses its dry run: see the open question below), incr-update,
the quick gate, static-identical (incremental rows change by IN-34a
pads: say which), the cost of the incremental full link (protoc).

Open: L10 runs the RELEASE mortar with `--incremental-report`; after
the flip a release build has no format-4 state, so no dry run, and the
judge's oracle FAILs "no delta end" for reasons small-output /
update-not-implemented. Needs a ruling (L10 on the hooked binary with
MORTAR_INCR_STATE4=1, or the judge's rule) unless it passes.

Resume: `git diff` in this worktree; scripts in
`/Volumes/AgentWork/mortar-helper/.build/target-wp-155/wp225/` (`t.sh
<log> <cargo test args>`, `before.sh`; `before.log` = the "before").
