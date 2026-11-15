# WP-071 work in progress (deleted in the final commit)

## Done
- mortar_diag: the 13 kinds of main's diag.md §3.1 table (gate fix; the
  same text as WP-072's uncommitted work, minus its odr-contents-mismatch).
- mortar_core: SectionKind::Metadata -> Meta, a no-output kind (A-2).
- mortar_meta: predicate, fnv1a64, reader (read.rs), writer (write.rs),
  anchors (anchor.rs); tests/format.rs (T-MT-10..14, 25, 26, 35, 36, 38),
  tests/mutate.rs (property test).

## Next
1. mortar_resolve: classification row (sections.rs classify, before the
   SHF_EXCLUDE row), state() Meta -> Dropped(Consumed), Resolution.meta
   list, MT-6 warning once per file (problem.rs What, tables.rs mapping),
   T-MT-6.
2. mortar_input: A-8 byte rule (store.rs/staged.rs), StoreConfig switch,
   test-only read hook, T-MT-42.
3. xtask meta write/dump, T-MT-3; experiment 7; record; gate.
