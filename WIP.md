# WIP WP-052 (static-pie)

Done:
- static-pie links on AArch64 and x86-64 (layout target.rs, resolve linker.rs, front.rs has_dynamic,
  dynlink.rs DynKind::StaticPie, synth got.rs static_pie/irelative_records, rela.rs, dynrel.rs, link.rs).
- D-47 text-relocation in the StaticPie column (scan driver dynamic_error); --export-dynamic refused.
- tests/link/dyn/spie/ (12 tests) + xfail lines (verifier request: P-D1 outside stage 3).
- unit tests: dyngot s70_static_pie_irelative_in_rela_dyn, dynrel stray IRELATIVE.
- merged main once (WP-082).
- corpus: review-m2 corpus 94 same / 2 DIFF (aarch64 trad TLSGD refused, as M1 static) / 4 CC-FAIL;
  static tests as static-pie: aarch64 21 same + 1 GNU ld defect, x86_64 20 same + 2 all refuse.

Next: static-identical; linktest dyn/ after the merge; record spec/landed/dyn/WP-052.md; gate; delete WIP.md.
