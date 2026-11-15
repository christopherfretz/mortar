# WIP: WP-061 stage A (link state)

Deleted in the final commit.

## Done
- Spec: `spec/incr.md` IN-18a (bytes of format 1), IN-18b (normalized
  plan), content hash (IN-22), stamp directories and STAT trust (IN-22a),
  IN-12 output kinds, IN-61 hook as a library function, §7 row
  `update-not-implemented`; `spec/model.md` §7.2/§7.3 (amendments 8, 6).
- Store content hash = the spec's block tree of L128 (`mortar_input`
  now depends on `mortar_digest`).
- Image keeps build-id leaves on request (`LinkOptions::keep_leaves`,
  `Written::{leaves, sealed, file_size}`), `Front::keep_leaves`.

## Next
1. CLI rows + `Plan::incremental`, diag kinds, regenerate options.md.
2. `mortar_incr`: format (encode/decode/validate), stamp, capture,
   replay translation.
3. Facade wiring (`crates/mortar/src/incremental.rs`), replay entry.
4. Tests, gates, measurements, record `spec/landed/incr/WP-061.md`.

## Gate state
Not run yet.
