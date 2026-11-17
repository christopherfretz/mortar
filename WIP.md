# WP-109b work in progress (deleted in the final commit)

- Step 1 (the interface): 4fc2cdf, gated, replay 3,449 identical.
- Step 2 (skeletons resident): `crates/mortar_plumb/src/store.rs` holds per
  object the header, section table, names, symtab/strtab/shndx, groups and
  `.mason.meta`; bodies read by position through the Source (a 64 MiB
  section cache, the last 2 objects whole when a phase loads them).
  Replay: 3,449 identical. Gate: running. Measurements: next.
- Malformed-input tests: `store.rs` (members by position == whole, lying
  headers), `tests/source.rs` (failing reads, later failures, files cut).
- Next: measure the four links (notes/link.sh, notes/plumb.sh), then step 3
  (tuning per phase), step 4 (--equiv sharing one store), §8.1, record.
