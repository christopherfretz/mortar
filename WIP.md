# WP-027c WIP (delete in the final commit)

## Done
- join: crates/mortar/src/image.rs (Front -> mortar_image::link::Link), lib.rs writes the
  output, -Map / -M, I-66 stale output removal, Outcome::Linked; -o, -Map, -M rows Supported.
- Store::blob_of_model, Store::inodes (direct recheck handle).
- fixes: front.rs shstrtab plan was the estimate's (sorted) names; S-122 header case (synth);
  OSABI GNU rule (link.rs structure + SymtabPlan::gnu_symbols); internal syms get table
  visibility (syms.rs, __rela_iplt_* local hidden).
- linktest: L3 known merge-piece filter (xtask plumb::known_merge_piece); 25 rows flipped.
- layout: cursors never move back (address.rs); L-159 empty-only output sections have no
  chunk (chunk.rs + check.rs); L-160 body aligned to island.
- rulings: missing file = cannot-open alone (walk.rs finish_checks); SHF_COMPRESSED refused
  (resolve sections.rs); --gc-sections row Supported (liveness refuses when final value on).
- tests: crates/mortar/tests/image.rs (moved from mortar_image/tests/link.rs),
  tests/pipeline.rs (program matrix vs GNU ld), tests/support/plumb.rs.

- thunks: link::patches writes Patches chunks (were zeros: a far call crashed); map lists
  thunk/patch chunks; erratum patches refused by name (W4' not handed to the patch).
- tests/pipeline.rs: program matrix, far call, determinism (stores x threads x seeds x
  repeat), time-phases, -M, gc-sections last wins, I-110, over-existing output, entry
  warning, bare build-id, -gz.

- erratum 843419 fixes were never written (layout computed them; fill wrote the unfixed
  code and zero patches): LinkSites::erratum_fixes + Link::fixups; test with a forced patch.
- xfail reasons/owners; linktest mortar 29 PASS 6 XFAIL. Stale tests (refusals, front_diff,
  lib) updated to the writing pipeline.

## Next
- protoc; debuggers; option rows; bench; record.
