# WIP — WP-221 (U-9), step 2

Done: rulings applied (817d7922 ruling 1, 865e056f F-3, b04204b4 seam 7,
b6ebd81e diags+emit, ad665676 tests incl. dormant rows). Record step-2
section written (spec/landed/incr/WP-221.md §2.x, §X8), A+ numbers pending
(placeholders PROBE_RESULTS, CAPPED_FREQ).

UNCOMMITTED PROBE (never commit): crates/mortar_scan/src/{events,report,lib}.rs
and crates/mortar/src/lib.rs (MORTAR_WP221_PROBE). Revert with
`git checkout crates/mortar_scan/src crates/mortar/src/lib.rs` after
.build/target-wp-156/wp221/aplus/run.sh finishes (FINAL APLUS DONE).

Next: fill A+ numbers; revert probe; full gate; delete WIP.md; hand back.
