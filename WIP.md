# WIP: WP-214 third part (seam 5 in format 5)

Brief: `briefs/wp-214-rulings.md`, last section. Seam 5 picked as 1f2eb933.

Done (committed): format 5 holds FRAGS by (out, class, hash) with refs
recounted from FRAG_USE (56), no `.debug_*` rows; FILES eh columns;
EH_RECORDS/FRAG_AT gone from format 5 and from the seam types;
KEY_REFS.demand; SLOTS.refs; RESOLVED.facts; FileRanges.first_global;
LayoutOf seam-5 accessors; `check_capture` (state-collision) and
`eh_columns` (state-limit); `live_keys` (dead keys) in normalize; new
local keys' ordinal in `intern_patch`. Tests: seam5_rows_round_trip_and_patch,
capture_collision_and_limit_are_refused, dead_key_dropped_by_compaction,
new_local_keys_get_their_ordinal.

Next: size table (protoc: size2.sh style with the format-4 state in
`.build/target-wp-116/wp214/`; clang via `size.sh cxx-large-g`), then the
full gate (`gates.sh`), then the record §10 in
`spec/landed/incr/WP-214.md`, delete this file.
