# WP-032a work in progress (deleted in the final commit)

## Done (commits on wp/032a-perf-input)
- f09e738 `MORTAR_TIME_STEPS=1`: sub-step times and work counts of phases 20
  and 30 on stderr (`crates/mortar/src/front.rs` `print_steps`;
  `mortar_input::load::load_steps`, `mortar_resolve::Outcome::steps`,
  `StoreStats::wave_ns`).
- e5d748f F1 resolve hash index (`mortar_resolve/src/index.rs`): protoc
  resolve 35.7 -> 17.7 ms (1T), 22.1 -> 9.0 ms (8T), golden identical.
- c12c3af supervisor side item: `input-changed` compares `st_nlink` too
  (s11_4 flaked 16/20 under overlayfs /tmp: coarse ctime); spec 11.5 amended.
- d2d56ea `find_symbol_table` one sh_type-only pass (noise-level).
- (this commit) F3 facts keep names + signatures (`ObjectKeys`); the
  interner's two passes iterate them: protoc input 1T 103.5 -> 88.1 ms med.

## Next
- wave C/B of the staged store (`store.rs` `read_batch`, `fill`): measure
  scratch-copy volume vs pread vs faults; dead members' bulk (30 MB) is an
  architectural proposal (resident set decided before resolve).
- resolve per_file / d1; profile table 1/2/4/8/12; mold/lld comparison;
  unsafe table (mmap: prototype in /tmp proto/); xtask bench rows; gate.
