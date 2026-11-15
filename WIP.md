# WIP — WP-028b paused for host migration (2026-11-15)

Branch `wp/028b-plumb-2b`, base 157adee. The code **compiles**, and
`cargo test -p mortar_plumb` passes in full (lib 24, corpus 5, eh 1,
hostile 4, mutation 14, tls 9). `cargo clippy -p mortar_plumb
--all-targets` and `cargo fmt` are clean.

**Gate not finished.** `MORTAR_REQUIRE_TOOLS=1 cargo xtask ci --quick`
passed `[ci] fmt` and `[ci] clippy`, then was stopped in `[ci] test`
when the migration began. Up to that point there was no FAILED or panic.

## Scope items

| item | state |
|---|---|
| 6 S7 `e_entry = 0` | done (0e40f05) |
| 1 IFUNC/IRELATIVE (F1–F4) | done |
| 2 non-alloc relocations, tombstones, R-9, compressed | done |
| 4 properties (P1, P2) | done |
| 5 requests (synth R-P1..R-P7, layout §20.3, a64 §8.7, image R-9) | done; marked "*Applied by WP-028b*" in each spec. Two parts remain: R-P3 is partly applied, and layout §20.3 still lacks S19 and the L-13 clause |
| layout rules S2 (mortar), S13–S18 | done |
| 7 x86-64/i386 in L3 | measured, not implemented: the measurements are in `spec/plumb.md` §4.5 and the follow-ups in the landed note |
| 3 build id (B1, R-8) | done: `src/buildid.rs` (about 470 lines with its tests), `spec/plumb.md` §6.7, `image.md` R-8 marked |

`spec/landed/plumb/WP-028b.md` is written: evidence, mutation rates,
times, contradictions and follow-ups with `file:line`. All experiment
results are in it and in `spec/plumb.md`; none is held anywhere else.

## Next steps

1. `source /Volumes/AgentWork/mortar-helper/scripts/env.sh`, then
   `MORTAR_REQUIRE_TOOLS=1 cargo xtask ci --quick` in the background. It
   must end `[ci] ok`.
2. If L3 (`cargo xtask linktest`) fails on a new S13–S18 or P rule, do
   this on a reference linker's output: check it against §10.2, then
   either classify it in `src/anomaly.rs` and add it to
   `tests/link/anomalies`, or fix the rule.
3. Put the gate result in the landed note's Evidence section. Delete this
   `WIP.md`, commit, and hand back. The hand-back items are listed in the
   landed note.

## Left on disk

`/Volumes/AgentWork/mortar-helper/.build/scratch-wp028b/` holds test
logs (`t*.txt`, `gate.txt`), `main-src/` (a git archive of 157adee),
`tgt-main/` and `tgt-new/` (release target directories used for timing).
All of it can be deleted. `/tmp/mortar-wp028b-x` has been removed. Also
delete the kept linktest run
`/Volumes/AgentWork/mortar-helper/.build/target-mortar/linktest/run-94347`
(by explicit path).
