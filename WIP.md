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

## Next
- tests/census.rs (t1/t2/t3 flag matrix; full corpus via env var).
- Q14/X-55 GNU ld COMMON+LCOMMON measurement; spec edits (x64.md, arch.md §3.6).
- timing example; fmt/clippy; gate; landed record; stage B after merge main.
