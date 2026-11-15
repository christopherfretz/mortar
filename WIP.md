# WIP — WP-041 (M2 step 0)

Done: Q7 (ed635c2). 0g `cargo xtask static-identical` (xtask/src/identical.rs, digest mode in
xtask/src/linktest/engine.rs, baseline tests/identical/baseline.tsv: 150 links / 40 tests, ~6-21 s;
ci step `static-identical`). Main's mortar (d86a0cd) vs the baseline: 42 AArch64 outputs differ (Q7), nothing else.
0a/0j: FileLive::Shared{needed,used}, IdSet, Resolution dynamic columns + bind(), check rule 7, model.md §3.3.
Learned, not yet written: GNU ld 2.44 AArch64 static writes `.plt`'s start into IPLT words (resolver in r_addend);
Q7 ruled S+A. x86-64 kept 0 (X-39, plumb F5(3)).
0b: Slots needed/verneed/dynstr/dynsym_unhashed/gnu_hash_buckets; CopyAlloc{place,align,alias}; DynStr, VerIndex, CopyPlace; Demands.weak_dyn; check rules.
0i: mortar_synth::dynsym (DynsymPlan, dynsym_plan stub); S-3 chunk list M2 rows + DynBss=24, BssRelRo=25, D-34 headers.
0h: mortar_synth::dynstr (DynNames, dynstr_plan stub); DynStr lives in Slots (0b).
Merged main 3f4cfe2 (4e76279; merge 2 of 3): S-69 consistent (my RELA bullet + 030c's REL paragraphs). Baseline re-cut:
167 links; i386: 33 rows exit 1->0 (end/output/map/stderr), 2 stderr-only (diag/duplicate, diag/undefined), 17 new rows;
x86-64 none; AArch64 one: reloc/data-pointers line 10 (output, map) — from main's 31175df (got_base_defined: the
_GLOBAL_OFFSET_TABLE_ definition now demands GOT_BASE on every arch, so .got gains its 8-byte header word; main's
own binary shows the same change). No other previously recorded digest moved.
Next: 0c (mortar_arch), 0k (mortar_scan weak.rs + diag name), 0e (dynamic_guard), docs/status/m2.md, spec/landed/dyn/WP-041.md.
