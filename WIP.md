# WIP: WP-207 (deleted in the final commit)

Done:
- experiments in tests/incr-design/ (README lists them); outputs in
  .build/target-wp-116/wp207/: model_region.out, crash_model.out, extraction model run (0 of 1200 hops differ).
- make/ninja probe: torn fresh output not relinked by make; epoch and missing relinked by both.
Half-done:
- fillers job (run-fillers.sh) waits for load < 12; outputs wp207/fillers-{a64,x64,i386}.out.
Learned, not yet written:
- operator 23:11Z: 1.6-1.8x accepted on premise of a measured win (gate stays at U-10);
  256 MiB in place "fine for now" + follow-up reflink package after U-10 (policy constant);
  split DWARF w/o pubnames yes for v1 (one sentence); --odr=warn default; no mixed -std.
Next:
- spec/incr.md amendments (IN-77/45/21, IN-71b, IN-72, IN-75, IN-49a, IN-78, IN-79, IN-18c-e, §7, §10),
  then spec/landed/incr/WP-207.md, WP-202 pointer, cargo tests, delete WIP.md.
