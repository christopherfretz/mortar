# WIP WP-177 (deleted in the final commit)
- mechanism: arch scan reports DiscardedSection for every non-debug dead target; driver (walk_unit) drops DiscardedSection/ComdatMismatch events in .eh_frame/.gcc_except_table whose owner is Dead (ScanInputs.eh: mortar::eh_owners::Owners over merge.eh + eh_relocs + liveness).
- fill: .gcc_except_table field against comdat_only undefined key -> 0 (reloc.rs).
- report: notes (group, kept copy, owner function, prelink).
- TODO: unit tests (merge eh owners, scan driver, arch), objcopy step, lane tests, spec edits, measurement, verifier answers, gates, record.
