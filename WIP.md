# WP-255c2a WIP

Done (all committed): M1 `crates/mortar_image/src/clip_plan.rs` (`for_clips`,
`check_written`), M2 the bitmap in `refill_ranges` and `refill_clip_plan`
(link.rs), M3 `crates/mortar/tests/clip_plan.rs` (both tests green), record
`spec/landed/incr/WP-255c2a.md`. Gates on 2b50c76b: fmt, clippy, linktest
0 FAIL, census 716 pass, incr-update 440 PASS; quick gate and
static-identical fail only on the 12 known rows.

Next: the supervisor's ruling on ClipParts' data and the §9.1 finding.
