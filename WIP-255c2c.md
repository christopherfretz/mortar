# WIP-255c2c (the clip plan's Synth, Patches, body .eh_frame arms)

Done:
- the record `spec/landed/incr/WP-255c2c.md` (design, rulings R-c1/R-c2, tests, bench);
- the code in `crates/mortar_image/src/{clip_plan,layout_plan,link}.rs`;
- the tests in `crates/mortar/tests/clip_plan.rs`: 8 pass in a debug build, `cargo fmt` clean;
- the plain-link bench (protoc static, medians of 5): base 267 ms, head 263 ms.

Not done (handed back at the budget): the full gate, which is quick ci,
linktest, census, workspace clippy, the testhooks clippy,
static-identical and the release run of the clip tests. Also not done:
the "holes a real update leaves" test, which needs a front-2 hook in
`crates/mortar/src/update` (not this package's).

Resume: run `.build/target-wp-145/w255c2c/gate.sh` in the background and
watch for its `FINAL` line. The results go to `gate.log` and
`gate-<step>.log` in the same directory.
