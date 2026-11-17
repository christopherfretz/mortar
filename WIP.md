# WIP: WP-175 (a NONE relocation overlaps nothing)

- [x] fix: crates/mortar_a64/src/table.rs (NONE 0/256 window empty; TLSDESC markers keep their word)
- [x] unit tests: a64 relax.rs s3_4_none_overlaps_nothing (+ s8_4_pattern_fails updated); x64, x86 pins
- [ ] spec arch.md §3.4 sentence
- [ ] measure NONE naming a symbol (gc, undefined, archive) x 4 linkers x 3 targets
- [ ] lane: partial step (harness.md, xtask linktest), tests/link/reloc/none-overlap*.test; update tls/gd-trad-static-refused.test overlap.S case
- [ ] item 4 verifier answer; item 5 other shapes
- [ ] gates: ci --quick, linktest, census; static-identical
- [ ] spec/landed/arch/WP-175.md
