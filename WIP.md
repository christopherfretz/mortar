# WP-073 work in progress (deleted in the final commit)

## Done
- Harness: `meta <description> <in.o> -o <out.o>` build step (format.rs
  StepKind::Meta, engine.rs meta_step, target.rs ArchTools.objcopy,
  meta.rs add_with).
- Diag kinds of the record checks (model.rs, render.rs, diag.md §3.1),
  the record place form (Place::Record, diag.md §2.3), linktest/odr.rs
  kinds and the `in ` relation.

## Half-done
- (nothing yet)

## Learned
- `#@ diag:` applies to every mortar link of a test: one mode per test.

## Next
- The 13 model cases in tests/link/odr/model-*.test, then the engine
  (crates/mortar_odr/src/records.rs), kind 1, kind 5, CHECKPOINT.
