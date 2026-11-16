# WP-086 work in progress (deleted in the final commit)

## Rulings (supervisor, after the checkpoint)
1. EH_RECORDS a decision: `EhPlan::select` + `EhPlan::from_records`; S-49
   CIE rule (own CIE if listed, else nearest earlier listed CIE of the
   same S-43 class); validity rules named failures.
2. SYMTAB_LOCALS (a): spans checked against the plan; v1 admits only the
   derived value; say what Q8 relaxes.
3. State stays version 1; fixture test (done, d0b84fe).
4. Fix GotPlan, ShstrtabPlan (+ L-86 order in check), merge Order,
   erratum fixes + symtab patch rows: one path where free, else
   compare-then-rebuild with a test that runs it.
5. Proof: valid mutation = changed bytes, verifier accepts, program runs
   same; invalid = named failure or verifier finding. Table before/after.
6. Dynamic Slots: note for WP-066 in the record.
7. Bench before/after: synthetic-comdat-static, synthetic-objects-static,
   cxx-medium-g-static. Before binary:
   `.build/target-wp-086/mortar-before-51aa582`.

## Done
- Item 1 instrument (94b9d3f), fixtures (d0b84fe).
- `EhPlan::select` / `from_records` (synth eh.rs) + tests s49_*.
- `Layout::check`: S-41 listed FDEs live and complete; L-86 index order.
- `SymtabPlan::check_spans`, `check_no_spans`.
- `Order::follow` (merge).
- front.rs: plans from the final layout after validate (eh/hdr/symtab
  compare-then-rebuild; symtab patch rows via the loop's replan step;
  spans checked; shstrtab one path; static got one path; erratum fixes
  one path after the replay, sites rebuilt when slot lists or merge map
  differ; Order follow compare-then-rebuild; sizes vs `Slots.sizes`).
- capture.rs: eh/symtab/errata refusals removed.

## Next
- Outcome assertions in the replay test; valid EH test (sibling link
  with a differing CIE of the same size).
- Specs, gate, hosted run, bench, record.
