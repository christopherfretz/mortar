# WIP — WP-032c (image phase; non-alloc checks moved to fill)

Done: image step timers (`MORTAR_STEPS=1`, `link::write`); one symbol read per
record in fill (`Tables::record_sym`, `LinkTables::sym_value`, ShiftTables caches
the record's `SymValue`).

Next: profile again (perf, 1 thread protoc); item (a) merge_piece_at index;
item (b) non-alloc scan moved to fill (spec I-14 amendment).

Golden hashes (base 38b6a2b): /tmp/mortar-wp032c-1507987/golden-base.txt
