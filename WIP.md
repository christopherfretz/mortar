# WIP — WP-029b mortar_x64 (stage A)

## Done
- mortar_arch: `SymValue.tls`, `SectionOut.exec` + `with_exec`.
- crates/mortar_x64 source: table, forms, relax, apply, synth, lib.
- Tests: table.rs, x64.rs, relax.rs, clip.rs (pass); differential.rs
  (§15 steps 1-4 in lld's and GNU's layouts, GNU converted types; passes,
  0 mismatches, every qemu run correct; gd-large not run: lld W4-corrupt).

## Half-done
- nothing in flight.

## Learned
- lld writes a hidden undefined weak as an undefined LOCAL in the symtab.
- GNU ld --emit-relocs keeps TLS types on relaxed sites; converts only
  GOTPCRELX/REX_GOTPCRELX (to PC32/32/32S; G4's PC32 sits at P-1).
- a.c's static IFUNC resolver must not call libc (getenv crashes).

- census.rs: default 192 objects; full (MORTAR_X64_CENSUS=full) 1272
  objects, 49416 records: no refusal in static; static-pie only
  Incompatible ReadOnly(64)/RecompilePie(PC32 weak0, 32, 32S).
- Q14 measured (GNU: COMMON+LCOMMON → small, .bss); x64.md X-55/Q14/§15,
  resolve.md §3.3, layout.md L-88, arch.md §3.6 updated.

- examples/apply_bench.rs: x64 scan 121 M/s, apply ~100 M/s; a64 apply ~78 M/s.

## Next
- gate stage A (running); then `git merge main` (WP-027c, WP-028c on main)
  and stage B per brief items 5-8; landed record at the end.
