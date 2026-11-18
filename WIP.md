# WIP: WP-217 (U-5) per-file symbol tables

Base a828a3de (compiles: `cargo check --workspace --all-targets` ok).

## Plan
- Half (1): `mortar_synth::symtab` IN-49a form in an incremental-mode full
  link: `PatchFacts.room` (set in `front.rs` from `env.slack`, one line),
  `Seg::Fill` segments (block fillers, strtab region slack, tail reserve),
  fillers written by `write_symtab`/`write_shndx`, `with_patches` by a
  general relayout; accessors `incr_blocks`, `incr_global_area`,
  `incr_tables` for U-2/U-6. File slack: `mortar_layout/src/chunk.rs`
  `.symtab`/`.strtab` take `slack.tail(size)` (one line).
- Half (2): `symtab_incr::plan`.

## Done
(nothing yet)

## Next
Half (1) in `crates/mortar_synth/src/symtab.rs`.
