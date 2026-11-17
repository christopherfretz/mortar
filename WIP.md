# WIP — WP-129 (metadata in `-shared`)

Brief: /Volumes/AgentWork/mortar-helper/briefs/wp-129.md. Branch
wp/129-meta-shared, rebased on int/batch-9 (596a946, WP-134 merged).
Scratch: /Volumes/AgentWork/mortar-helper/.build/target-wp-145/wp129/.

## Done
- Part 1: rule (mortar_odr::reloc_model, front), diag form, tests
  (mt26-reloc-model, mt26-reloc-model-hod), (hod) family + HodRows probe,
  spec (meta/dyn/diag/harness/status).
- Part 2: O-60 (dynamic.rs NONPREEMPTIBLE, front odr_output for Shared),
  fixture shared-o60 + o60-* + o24-shared-output tests (xfail: P-M2).
- Part 3: MT-32 tests (mt32-keep-meta-shared[-strip]); measured PT_LOAD.
- Record spec/landed/meta/WP-129.md (producers section pending).

## Next
- producers baseline (running), static-identical rows, gates x2, lane.
