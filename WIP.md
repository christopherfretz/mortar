# WIP: WP-164 (verifier: shared object's metadata; stripped outputs)

Done and committed: part 1 (stripped), part 2 (P-M2 Shared), part 3
(P-M1 in no segment), meta.rs Shared mutations (fixtures lib.so/lib.map),
spec text (P-D22, P-D9, GOT base, §7.2, §13.1/13.2, odr.md O-60), the
sweep case `shared.table` (in QUICK_CASES).
Running: the sweep of shared.table on both targets (wp164/sweep.log).
Next: §12.3 text for the new case with its numbers; static-identical
--update (shared-stripped, exec-stripped); git merge main; gate + lane
in the background; record spec/landed/plumb/WP-164.md; delete this file.
Found on the way (not mine): IFUNC export in -shared: bfd F4 under
-Bsymbolic, lld A20, mold view B (unstripped too); mold x86-64
-no-pie tbss-only PT_TLS outside PT_LOAD -> S9.
