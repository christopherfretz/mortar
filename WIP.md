# WIP — WP-041 (M2 step 0)

Done: Q7 (ed635c2). 0g `cargo xtask static-identical` (xtask/src/identical.rs, digest mode in
xtask/src/linktest/engine.rs, baseline tests/identical/baseline.tsv: 150 links / 40 tests, ~6-21 s;
ci step `static-identical`). Main's mortar (d86a0cd) vs the baseline: 42 AArch64 outputs differ (Q7), nothing else.
0a/0j: FileLive::Shared{needed,used}, IdSet, Resolution dynamic columns + bind(), check rule 7, model.md §3.3.
Learned, not yet written: GNU ld 2.44 AArch64 static writes `.plt`'s start into IPLT words (resolver in r_addend);
Q7 ruled S+A. x86-64 kept 0 (X-39, plumb F5(3)).
Next: 0b/0h/0i (Slots fields, DynStr, dynsym/dynstr stubs, chunk rows, check.rs relaxations).
