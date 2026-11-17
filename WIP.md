# WIP — WP-121 (verifier stage 3 for shared objects, P-D16..P-D26)

Branch `wp/121-plumb-shared`. Brief: `/Volumes/AgentWork/mortar-helper/briefs/wp-121.md`.

## Done
- map: `OutputKind::Shared`, `DynRecords::shared: Option<SharedRecords>`,
  `Symbolic` (map/mod.rs, map/mortar.rs: outside my list, named in hand-back);
  `maprec::{read_shared, end_shared, write_shared}` with tests.

## Next
- shared.rs: SharedLine from the words; version script / dynamic list reader
  and matcher; export set (D-110), preemption (D-112); static rules.
- dynverify.rs: Shared branch (P-D16, P-D3 records, views A/B).

## Open
- dyncheck::check (WP-122's) is executable-shaped; for Shared: filter or skip.
