# WP-030c work in progress (deleted in the final commit)

## Done
- 4681e52 spec rulings; 10e5f46 `PlaceFacts.exec`.
- Driver selects i386 (`crates/mortar/src/arch.rs` `LinkArch::I386`; front.rs
  refusal removed). front.rs: synthetic chunk names/headers for REL (`.rel.plt`)
  and the output class (symtab, property note, chunk headers) from the target.
- REL in fill: `crates/mortar_image/src/rel.rs` (decode, implicit addend from
  the site snapshot); dispatch points in `reloc.rs` (decode, `raw_offset` by
  entsize, addend for split targets, `RelocAt.addend: None` on REL);
  `plan.rs` `RelaTable.rel` + entsize; `layout_plan.rs` decodes REL.
- Static hello (gcc -O2) links and runs under qemu-i386, same as GNU ld.

## Learned
- `raw_offset` assumed RELA entsize: the last REL records were never picked.

## Next
- Matrix in /tmp/mortar-wp030c-3761555 (bin/lk.py): C++ exceptions,
  thread_local, Fortran, clang, -fPIE/-fPIC, gc-sections, -g.
- Scan: i386 `SHT_RELA` refused by name; same-r_offset/straddler rules;
  `mortar_live` REL addends; e_flags value shown.
