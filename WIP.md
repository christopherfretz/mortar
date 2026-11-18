# WIP: WP-225 (U-10, the join)

Done: Step 0 (aa4ad63d). M1: e8e3daff (G-13 knob), 4539fdaf (the flip,
Start::Update, U1-U3, lane.py's line), 51676760 (the record's M1
section: findings, static-identical rows, cost). Handed back at M1
(context budget).

Blocked on a ruling: the dry run is gone from plain builds, so the
L10 lane (`cargo xtask l10 --quick`, in `xtask ci`) and the IN-63a
oracle lane (`crates/mortar/tests/incr_oracle.rs`) FAIL "the report
has no `delta end`". Options in the record (M1, 0.7's items).
static-identical: 12 incremental rows differ (IN-34a pads), baseline
not updated.

Next (M2, path (A)): `update::image_link` (the whole front over the
new inputs, P_U replayed), G-2's `pub fn` at the end of `capture.rs`
(own commit), refill, update_map, plan_patch, write_with with the w5
closure, the (d) check after every hop, the named M2 tests; G-10
(`--defsym` symbol operand -> (layout)), G-7 (-Map/-M/--print-map ->
(image)), G-15 (diags), G-6 (state5). Start in `update/mod.rs`
`steps()` where it returns `(resolve)`.

Resume: scripts in `/Volumes/AgentWork/mortar-helper/.build/target-wp-155/wp225/`
(`t.sh <log> <cargo test args>`, `gate.sh`, `cost.sh`, `m1.sh`);
`bin/mortar-base` is the base's release build.
