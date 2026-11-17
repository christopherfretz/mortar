# WIP: WP-136 (placeless symbols)

- Root cause: `LinkTables::sym_value_of` (`crates/mortar_image/src/link.rs`)
  ended `entry.or(s).unwrap_or(0)`; `live` came from liveness alone, so a
  Kept-but-placeless section (empty Merge, `.note.gnu.property`) was live
  with value 0. Dropped sections: live=false -> fill MissingTerm (internal).
- Spec written: resolve.md §10.1 P-1, synth.md S-6 (empty), model.md §4.5
  invariant, image.md I-124 and the §4.4 event row.
- Next: mortar_core::place; resolve placed_whole size 0; scan view dead;
  report for globals; image Valued; dynsym export filter; tests; matrix
  after; item-7 scan; gate; record spec/landed/resolve/WP-136.md.
- TODO verify resolve.md §10.1 measured table against before.tsv (x86_64, i386).
