# WIP: WP-044 (M2-DYNSYM)

Done: plan + chunk writers in `crates/mortar_synth/src/{dynsym,gnuhash,dynstr,version}.rs`;
oracles in `crates/mortar_synth/tests/dynsym.rs` (fixture rows, refusals, schedules, glibc
lookup at 0..10^5, reviewer gnuhash model 27/27 EQUAL, lld order + .gnu.hash 22/22 links,
verneed model + lld decoded equality). Signature change: `dynsym_plan` takes `src`, `pool`,
`diag` and returns `Result`.

Next: merge main (WP-051 landed), run its dynsym cases, the record
`spec/landed/dyn/WP-044.md`, the gate.
