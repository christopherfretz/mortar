# WIP: WP-255c (StoredRows)
- Done: M1 table, M2 `crates/mortar/src/update/stored_rows.rs`, M3 `crates/mortar/tests/rows_stored.rs` (green); record `spec/landed/incr/WP-255c.md`.
- Next: the mutation tests (copy the state bytes, `mortar_incr::state5::read::decode` -> flip `RESOLVED.value` / `SECTIONS.place_offset` / `CHUNKS.addr` -> `write::encode` -> `open_bytes`; `check` must fail per flip).
- Resume: `cargo test -p mortar --test rows_stored -- --nocapture`.
