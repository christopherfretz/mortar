# WIP — WP-043 (M2-RES)

## Done
- Q1 measured (crates/mortar_resolve/tests/oracle/q1*.sh); rule written into
  spec/dyn.md D-12 (table + rule + refusal), §15 Q1, Appendix B; resolve.md §4.4.
- mortar_resolve shared stage: src/shared.rs (N, U, D-19a, bind, D-13, §3.5,
  Q1 refusal, D-16, columns); InputsView reads dynsym/versym/verdef/verneed;
  tables.rs fills Shared rows, needed, columns, version, PREEMPTIBLE.
- Test Model can make shared objects (shlib, closure_lib, needs, set_version).

## Half-done
- nothing

## Learned, not yet written
- SonameUnit has no per-mention as_needed/group: inferred in tables.rs
  shared_link; unknown -> refused (NeededOrderUnknown). Core request.
- RefFlags FROM_SHARED/DEFINED_SHARED/EXPORTED kept, equal to the columns.

## Next
- merge main (WP-051); its xfails tagged WP-043.
- synthetic tests (tests/shared.rs); real-input oracle ports; record.
