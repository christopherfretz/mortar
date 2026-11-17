# WIP — WP-146 (W4 compares unit-local references by content)

Done:
- spec/odr.md §8.1: W4 row + "W4's comparison (normative)"; X21 in §1's list.
- crates/mortar_odr/src/contents.rs (encoding + `differ`, 8 unit tests);
  foreign.rs `w4`/`contents` use it.
- tests/link/odr/w4-local-refs.test, w4-local-refs-mutated.test (running).
- Measurement: scratch .build/target-wp-140/wp146/m (72 equal pairs:
  425 findings before, 0 after).

Next: link tests pass; corpus before/after (legal.sh 12-TU, protoc mixed);
cost on/off; record spec/landed/meta/WP-146.md; lane + gate; delete WIP.md.
