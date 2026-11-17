# WP-135 WIP

- Measured (aarch64 host, GNU ld 2.44, lld 19.1.7, mold 2.37.1): all three
  drop an input `.note.gnu.build-id` (every style, two inputs, gc, =none);
  mortar copied it. `.note.package` kept by all (and mortar);
  `.gnu_debuglink` GNU drops, lld/mold/mortar keep (split: out of scope).
- Rule: spec/resolve.md §10 row `.note.gnu.build-id` -> Excluded,
  Dropped(Excluded) (listed as discarded in the map, as GNU ld).
- Next: code at crates/mortar_resolve/src/sections.rs classify; crate test
  in crates/mortar_resolve/tests/rules.rs; link test
  tests/link/layout/input-build-id.test; gate.
