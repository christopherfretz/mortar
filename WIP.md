# WIP — WP-121 (verifier stage 3 for shared objects, P-D16..P-D26)

Branch `wp/121-plumb-shared`. Brief: `/Volumes/AgentWork/mortar-helper/briefs/wp-121.md`.

## Done
- map: `OutputKind::Shared`, `DynRecords::shared: Option<SharedRecords>`,
  `Symbolic` (map/mod.rs, map/mortar.rs: outside my list, named in hand-back);
  `maprec::{read_shared, end_shared, write_shared}` with tests.
- shared.rs: SharedLine, version script / dynamic list reader + fnmatch,
  export set (D-110), preemption (D-112), P-D16, P-D19, P-D20-P-D22,
  P-D25, P-D26 (relaxation, DF_STATIC_TLS, TLSDESC table), dyncheck filter,
  interposer, view B helpers. Unit tests.
- dynverify.rs: Shared branch wired (sets, P-D3, static checks, view B).
- loader.rs: interposer in `bind`; defined TLS interposed in `apply`.
- outside my list: x64rec.rs (kept GD/LD compare; DTPOFF in a .so is
  DtpRel), reloc.rs (`slot_ok` GNU TLSDESC in .got.plt; `Engine::shared`).
- tests/shared.rs: forms (6 line forms x 3 linkers x 2 arches), more.

## Next
- P-D18 (IFUNC one address), never-panic tests, mutations, Part 2.

## Open
- P-D25 accepts from a reference a record naming a PROTECTED key, or any
  bound key with DT/DF_SYMBOLIC under -Bsymbolic (GNU ld's form): spec edit.
