# WP-128 WIP (deleted in the final commit)

Scratch: /Volumes/AgentWork/mortar-helper/.build/target-wp-141/wp128/ (m1 flags, m4 exclude-libs, m5 -init).

## Measured so far
- D-103 -z flags: Shared all three agree with the table. Executables: GNU ld writes
  INTERPOSE and ORIGIN only (nodelete/nodlopen/initfirst dropped; no-pie writes an
  empty FLAGS_1); lld and mold write all five. static: nothing written by any.
  No `no-` forms in any reference (unknown -z, warned).
  -> D-103 keeps executables at M2 status (refused): rows go SHARED_ONLY. cli.md
  sentence "in an executable GNU ld writes the same flags" is wrong: correct it.
- -init: local symbol, symbol only in a shared lib, archive member not extracted:
  all three omit the tag (mortar: undefined-init-symbol). -init does not extract.
  data object: all three write its address. gc: GNU ld, lld keep hidden fn; mold W22.
  mortar exe -init + --gc-sections: INTERNAL ERROR today (not a root) -> fix (roots).
- --exclude-libs: basename of the archive; a path in the option matches nothing;
  `,` and `:` separators; ALL case-sensitive; `libfoo` (no .a) matches in GNU ld only
  (STOP cell -> refused by name meanwhile). .symtab: GNU ld/lld LOCAL DEFAULT, mold
  LOCAL HIDDEN. No dynamic records for excluded keys. Exe: exclusion removes -E and
  --export-dynamic-symbol exports; a DSO reference to an excluded key: GNU ld, lld
  refuse; mold links a broken program.

## Next
- code: rows (cli), dynlink flags, exclude-libs (resolve), dynamic-list, init roots.
