# WIP WP-142 (deleted in the final commit)
- done (63cd1d9): S-48a (synth.md), EhPlan::exists (bare terminator), crate test,
  3 link tests (pass locally, all linkers, verifier clean).
- STOPPED on item 5: two silent mislinks found (a referenced global in an empty
  SHF_MERGE section, and in an input .note.gnu.property: mortar writes 0, links
  rc 0). Reproducers: .build/target-wp-142/p5/{emptymerge,prop,r-gms,r-gprop}.s.
  Waiting for the supervisor.
- not done: static-identical --update rows, gate, lane, spec/landed/synth/WP-142.md.
