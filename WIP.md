# WIP — WP-032c (image phase; non-alloc checks moved to fill)

Done: image step timers (`MORTAR_STEPS=1`, `link::write`); one symbol read per
record in fill; item (a) merge_piece_at index; item (b) non-alloc relocation
checks moved from the scan to fill (spec/image.md I-14 amended per the
supervisor's ruling; `crates/mortar/tests/deferred.rs`).

Next: profile again; further image fixes only above ~3%; protoc end to end
against mold and lld; the record `spec/landed/perf/WP-032c.md`; the gate.

Golden hashes: /tmp/mortar-wp032c-1507987/golden-merge.txt (merge 2f79948,
including the x86-64 `-g` workload x64-g).
