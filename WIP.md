# WP-120 work in progress (deleted in the final commit)

Briefs: /Volumes/AgentWork/mortar-helper/briefs/m4-shared-common.md, wp-120.md.
Merge `origin/main` (not local main) before the final gate.

## Done
- Measure: lld 19.1.7 vs lld 22.1.8 on bsym2 (D-112 table): identical on all
  17 lines x 2 arches; lld 22 identical to the design's result.txt.
  Scratch: .build/target-wp-120/bsym19/{run.sh,result.txt}.

## Design (decided)
- `host <driver> <args>` step: pair part = steps from the first `host` on,
  run once per pair (library linker A, host linker H) in `<unit>/<A>` (A=H)
  or `<unit>/<A>+<H>` (copy of A's dir after A's library part).
  Pairs: reference R: (R,R), (R,bfd); mortar: (bfd|lld|mold, mortar) for
  passing references FIRST, then library part, (mortar,mortar), (mortar,bfd).
  Every pair-part run compared (stdout + end) with the bfd/bfd pair.
- `shared-*` fixtures may hold `link`/`check` steps: run per linker, cached
  per (name, arch, linker); copied into the linker's dir (no `.linktest-*`).
- Native runs: env via /usr/bin/env so LD_PRELOAD never reaches the sh wrapper.
- Census: `shared*` variations (Pic flavour), counted, refusal passes.
- Realbuild: Comparison counts fixed `shared` lines.
- spec/harness.md rules LT-1.. .

## Next
- (update as you go)
