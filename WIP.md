# WIP — WP-061 stage B (the measured floor)

Done:
- `mortar_image::link::refill` (fill the jobs of dirty windows into memory),
  `with_image` split out of `write_link` (no plain-link change intended).
- `mortar_digest::Leaf::kept`.
- `mortar::floor::update` (test-only update path, §5.2 steps 1-7), example
  `incr_floor`. protoc -g thin: copy and in-place outputs byte-identical to a
  plain link of the edited inputs (build id included).
- test `the_floor_update_is_the_full_link_of_the_edited_inputs` (both paths,
  two chained edits, bytes == plain link, verifier accepts, program runs).
- bench workload `synthetic-objects-g10x` (class `on-request`), prepared in
  /tmp scratch: 8156 objects, 1,653,848,958 B in.

Next:
- record spec/landed/incr/WP-061b.md: the workloads and edits before measuring.
- measurement script (scripts/incr-floor.py), protoc overlay + virtiofs, g10x overlay.
- plain link pays nothing: bench A/B; gate; delete g10x; delete scratch.
