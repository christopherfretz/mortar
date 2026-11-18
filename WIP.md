# WIP: WP-228 G-1 part B4 (deleted in the final commit)

Scratch: `.build/target-wp-154/wp228/b4` (`t.sh`, `i1.sh`, `split.sh`, `keep/`).

1. DONE: merged tree 04346e34 builds; `debuggers incr_*` 7/7, the replay
   test, `split/incr-gnu-index` 8 PASS, `update_debug_bytes` 7 tests ok.
2. In progress: real `DEBUG_STRS` rows (`debug::strings` over the
   capture's rows); one no-rows test `state_without_debug_strs_appends_or_falls_back`.
3. Open: `MORTAR_WP228_KEEP` now keeps the scratch + `upd-<tag>.map`
   (the image's map); plumb over every kept dir.
4. In progress: `place` refuses a headerless `.debug_addr` file (G-1c);
   GNU sets skipped (IN-55 option (A)); tests `split5_…`, `dwarf4_split_…`;
   `place.py` dead lines before the decision records (done).
5-7. Open.
