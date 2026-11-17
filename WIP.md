# WIP: WP-201 (verifier; deleted in the final commit)

Done (code): items 1 (ehowner.rs, reloc.rs record(), eh.rs live-CIE dead
personality now R5), 2 (A105 bfd LIVE_ZERO, A106 mold LIVE_AT_ZERO), 4 (A104,
symbols.rs EXCLUDED_MAPPING), 5 (opts::of(map, line) + threading).
Staged tests in `.build/target-wp-143/wp201/pend/` (lane run: only
comdat-mismatch-weak-prelink still fails on bfd/lld aarch64/x86_64).

Next: settle prelink's forms; fixtures `crates/mortar_plumb/tests/ehowner/`
(gen.sh needs the objs-per-linker fix), tests/ehowner.rs, item-5 tests,
spec/plumb.md §5.7b, §10.2, §3.2a/§12.1, record spec/landed/plumb/WP-201.md,
un-pend tests, final gate.

Resume: `bash .build/target-wp-143/wp201/lt1.sh` re-runs the staged lane.
