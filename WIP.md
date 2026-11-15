# WP-071 work in progress (deleted in the final commit)

## Done
- mortar_diag: meta-malformed, meta-version kinds; diag.md §3.1 rows moved.
- mortar_core: SectionKind::Meta (no-output), Resolution.meta, check rule.
- mortar_meta: predicate, fnv1a64, reader, writer, anchors; format.rs,
  mutate.rs tests.
- mortar_resolve: MT-6 classification, Consumed, Resolution.meta, the
  not-a-container warning; T-MT-6 (rules.rs s10_mason_containers).
- mortar_input: A-8 (StoreConfig.containers, Hooks.on_read, headers-first
  for top-level objects, clipped reads, merge never bridges a container;
  archives: members' containers never resident, but Chain/head/tail reads
  may read container bytes -> finding); T-MT-42 (tests/t_mt_42.rs).

## Next
1. xtask `meta write` / `meta dump`; T-MT-3 (ld, lld, mold drop it).
2. End-to-end: link with and without a container is byte-identical, no
   warning.
3. Experiment 7 + bench before/after (protoc, 1 thread, median of 15).
4. Record spec/landed/meta/WP-071.md; gate; delete WIP.md.
