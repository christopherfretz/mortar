# WP-027c WIP (delete in the final commit)

## Done
- join: crates/mortar/src/image.rs (Front -> mortar_image::link::Link), lib.rs writes the
  output, -Map / -M, I-66 stale output removal, Outcome::Linked; -o, -Map, -M rows Supported.
- Store::blob_of_model, Store::inodes (direct recheck handle).
- fixes: front.rs shstrtab plan was the estimate's (sorted) names; S-122 header case (synth);
  OSABI GNU rule (link.rs structure + SymtabPlan::gnu_symbols); internal syms get table
  visibility (syms.rs, __rela_iplt_* local hidden).
- gcc -static hello links and runs.

## Learned, not yet written
- plumb in_section inclusive-end bug + S-17: both fixed on wp/028c-plumb-x64 8804ebe.
  Filter in harness: R1 whose relocation target is in an SHF_MERGE input section.

## Next
- harness: plumb filter; flip XFAIL rows; move mortar_image tests/link.rs to crates/mortar/tests.
