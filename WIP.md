# WIP — WP-063 (delta, refill set, dry run)

Done:
- format v2 (MENTIONS 32, RESOLVED 33, INDEX 34), capture, detect.rs, delta.rs, the dry run in incremental.rs.
- the oracle lane tests/incr-oracle/ (oracle.py, run.py, src/), cargo test crates/mortar/tests/incr_oracle.rs (quick),
  cargo xtask incr-oracle (full + mutations). Full run: 160 pairs pass.
- merged chunks that move move their fragments (delta.rs frag_map), found by the `os` scenario.
- spec/cli.md 3.13: the report format.

Next:
- measurements on protoc -g (report vs none vs plain; static-identical).
- spec/incr.md IN-18a (v2 tables), docs/ARCHITECTURE.md (mortar_incr -> mortar_arch), record spec/landed/incr/WP-063.md.
- gate, push once, hosted run.
