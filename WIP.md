# WP-031 WIP (deleted in the final commit)

## Done
- `crates/mortar_live/src/gc.rs`, `read.rs`: the mark (facts, defs, eh edges, rounds).
- `LiveInputs.roots`, `LiveOptions.keep_exported`; `mortar_resolve::Outcome.roots`.
- cli: `--print-gc-sections`, `--no-print-gc-sections`, `--gc-keep-exported` land in
  `Plan.shaping.{print_gc_sections,gc_keep_exported}`.

- groups live together (gc.rs file_facts, Walk::walk); unexamined sections are roots.
- scan: a global defined in a kept, non-live section is `dead` (mortar_scan/src/view.rs Defined arm).

- non-debug sections' relocations are edges (EDGES bit, gc.rs): GNU marks through notes
  (`.note.stapsdt` -> `.stapsdt.base` in libstdc++); found by tests/gc.rs.
- `--print-gc-sections`: mortar_live::print_gc_sections, Invocation.removed, main.rs; `name[sig]`.
- tests: crates/mortar/tests/gc.rs (12 tests), pipeline.rs last-one-wins, refusals.rs I-5 removed,
  linktest layout/gc-sections un-XFAILed.
- differential harness: /tmp/mortar-wp031-1342893/diff/{run.py,cmp.py} (results.json).

- image: a debug reference to a dead merge piece is the tombstone (`Tables::piece_live`,
  mortar_image reloc.rs/link.rs); test i47_debug_references_to_dead_merge_pieces_are_tombstones.
- spec: image.md I-2/5/6/7/10/12 amended (E14 Q-results), resolve.md §4.7 roots.
- differential classified (66 configs): vs bfd only `.group`, comdat losers, and COMDAT-keeper
  differences (resolve.md §6.1 ruling C; traced on cxxfull-g++-O2: 204 via keeper, 5 losers);
  vs lld size-0, `.stapsdt.base` (lld drops), lsda lld keeps (Q4); vs mold size-0, mold's
  conservatism (every mortar-only removal is also bfd's and lld's), mold's libc .text removals.

## Half-done / next
- merge main f0f1dcc (WP-027d: gc row refused + options.rs audit, debuggers.rs gc variant;
  WP-029b x86-64: differential under qemu with C++ EH + TLS).
- rerun matrix (tmpl fixed); protoc differential; cost; byte-identical no-gc; audit; record.

## E14 experiments (subagent, /tmp/mortar-wp031-1342893/e14, sources q*.s/c)
- Q1 notes: kept by all (alloc+nonalloc, dead object too); note in a dead group: GNU/lld drop, mold keeps.
- Q2 non-alloc w/o relocs: kept by all in live objects; GNU drops ALL non-alloc of an object with no
  marked alloc non-note section (`.eh_frame` counts as marked); lld/mold keep.
- Q3 groups: GNU + lld keep the whole group if any member live; mold per section.
- Q4 dead fn's `.gcc_except_table.f`: GNU/mold remove, lld keeps. DW.ref._ZTIi only from it: lld keeps,
  GNU/mold remove. Personality via CIE of a dead-only FDE: GNU removes (with CIE), lld/mold keep.
- Q5 `.init_array` in COMDAT group kept by all (+ targets; GNU/lld keep other members, mold not);
  `.init_array.N`, preinit, fini, .init/.fini, `.ctors*`, `.dtors` kept by all.
- Q6 start/stop: GNU nostart-stop-gc default; lld 19/22 start-stop-gc default (undefined __stop_ error);
  mold ignores option, roots EVERY C-identifier section. Unreferenced C-ident section: GNU/lld remove.
- Q7 retain: gcc14/clang19/22 emit R; all keep; output flag kept by GNU/lld, mold strips.
- Q8 roots: -u all; --undefined-glob GNU unknown, lld extracts no root, mold odd; --require-defined
  GNU/mold root (lld no option); -e all; --export-dynamic static: GNU nothing, lld19 roots all,
  lld22 nothing, mold roots all + .dynsym; --gc-keep-exported GNU only: default-vis globals.
- Q9 print: GNU stderr `<ldpath>: removing unused section '<sec>' in file '<file>'`; lld/mold stdout
  `removing unused section <file>:(<sec>)`; GNU skips size 0, lists NOBITS and whole merge sections;
  never .eh_frame; order input then header (mold nondeterministic). GNU also lists `.group`, names
  group members `sec[sig]`.
- Q10 archive member all collected: still loaded by all (dup definition collides).
- Q11 merge: no gc all keep unreferenced strings; gc: GNU keeps whole live section, lld/mold drop pieces.
- Q12 .stack_sizes: GNU/lld remove dead entry; mold keeps (addr 0, sh_link 0).
- Q13 non-alloc non-debug ref to collected: no diagnostic anywhere; GNU removes ANY non-alloc section
  with relocations (!); lld/mold keep, write 0+A. debug_info 0 all; debug_ranges 1; debug_loc GNU 0.
- Q14 code only from debug: collected by all, value 0 by all.
- Q15 dead FDE removed by all; its CIE: GNU removes, lld/mold keep.
