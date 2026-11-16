# WP-109b work in progress (deleted in the final commit)

- Step: 1 (the interface) committed; gate running.
- Interface: `mortar_plumb::Source` (count/path/len/read_at, files by
  path; regular-archive members found by the verifier through the
  source), `verify_from`, `equiv::check_from`; `FileSource` in
  `xtask/src/plumb.rs`. Step 1 reads every file whole through the source.
- Verdict replay: list of 3,449 calls (`linktest --keep`, all arches) in
  `.build/target-wp-109b/notes/calls.tsv`; replay program
  `.build/target-wp-109b/cmp` (base afbdb15 vs this branch's command).
  Step 1: 3,449 identical, 0 different.
- Next: step 2 (skeletons resident, bodies on demand).
