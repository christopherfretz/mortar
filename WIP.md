# WP-073 work in progress (deleted in the final commit)

## Done
- Harness: `meta <description> <in.o> -o <out.o>` build step (format.rs
  StepKind::Meta, engine.rs meta_step, target.rs ArchTools.objcopy,
  meta.rs add_with).
- Diag kinds of the record checks (model.rs, render.rs, diag.md §3.1),
  the record place form (Place::Record, diag.md §2.3), linktest/odr.rs
  kinds and the `in ` relation.
- The 13 model cases: tests/link/odr/model-*.test (14 files).
- Engine: crates/mortar_odr/src/records.rs (kinds 1/5/3/2, O-25, O-31,
  O-32), register.rs checks 5..12 and 16, lib.rs switches/stats,
  mortar/src/lib.rs StoreConfig.containers under strict only.
- records-legal.test: §3 legal rows silent under strict.

## Half-done
- (none)

## Learned
- `#@ diag:` applies to every mortar link of a test: one mode per test.
- g++ -O2 const-propagates a tiny function away: use noinline+noipa for
  anchored kind-5/kind-2 rows in tests.

## Next
- CHECKPOINT hand-back.
- Robustness (mutation corpus under strict, duplicates, forged counts,
  mixed minor versions), §12.4 mutation tests + kill table.
- Kind 2 flag tests (PARTIAL, EXPLICIT_SPEC+IMPLICIT_INST, USES_INTERNAL),
  isa, abi violation tests.
- Cost: bench, exp7 generator, scaled, O-45 per kind, 0.3 µs measurement.
- spec/landed/meta/WP-073.md, gate, delete WIP.md, rm scratch.
