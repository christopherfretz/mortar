# WIP — WP-046 (M2-RELOC)

Branch `wp/046-m2-reloc`, cut at main e910975. Deleted in the final commit.

## Done
- scan: D-25 per-key weak classes (`weak.rs` `classes`), View reads
  `weak_dyn` per key; refused weak keys' records skipped by the walk.
- scan: D-47 `Incompatible(ReadOnly)` -> `text-relocation` when `dynamic`.
- scan: D-18/D-42 per-key COPY/CANON_PLT refusals (`copies.rs`).
- arch `class.rs`: D-43 (Pie Pc vs shared data = COPY), D-44 (Pie
  Pc/Lo/AbsNarrow vs shared function = canonical-plt-in-pie).

## Next
- synth: copy plan, dynamic GotPlan, `rela.rs`.
- image: `dynrel.rs` record writer, fill call sites.
- tests: fixture arch, oracle tests vs GNU ld / lld, thread independence,
  10^6 RELATIVE timing.
