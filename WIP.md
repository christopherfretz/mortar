# WP-117 work in progress (deleted in the final commit)

- Branch wp/117-shared-front, from the tree with WP-116, WP-112, WP-109b s1.
- Decisions: `-shared` stays `refused` at the CLI (front's arch_status gives
  M1 for Shared: a static-path image; record §G.4) with §G.4's text as its
  note; Shared-only refusals (cli §4.2) in `Parsed::refused_uses`.
- Measurements: `.build/target-wp-117/measure/run.sh` (s1d.py over kiln+hod,
  clang-19, gcc, gfortran AArch64 objects).
- Steps: [ ] CLI rows+refusals [ ] opts.rs [ ] resolve export/preemptible/D-118
  [ ] scan class cells, bound.rs, driver hook [ ] tests [ ] record [ ] gate
