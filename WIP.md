# WIP — WP-061 stage B (the measured floor)

Done:
- `mortar_image::link::refill` (fill the jobs of dirty windows into memory),
  `with_image` split out of `write_link` (no plain-link change intended).
- `mortar_digest::Leaf::kept`.
- `mortar::floor::update` (test-only update path, §5.2 steps 1-7), example
  `incr_floor`. protoc -g thin: copy and in-place outputs byte-identical to a
  plain link of the edited inputs (build id included).

Next:
- test in crates/mortar/tests/incremental.rs (both paths, cmp with plain, verifier).
- workloads written into the record before measuring; synthetic-objects-g10x.
- measurement script, phase attribution (layout = replay cost), RSS.
- plain link pays nothing: bench A/B.
