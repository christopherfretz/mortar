# WIP — WP-041 (M2 step 0)

Done: Q7 (IPLT word per arch: `Facts.iplt_word`; AArch64 S+A, x86-64 0, i386 REL resolver).
Half-done: 0a/0j edits in the tree, uncommitted (mortar_core idset.rs, resolution.rs, check.rs, tests/tables.rs; mortar_resolve tables.rs).
Learned, not yet written: GNU ld 2.44 AArch64 static writes `.plt`'s start into IPLT words (resolver in r_addend), not S+A; Q7 ruled S+A (a64 §6.2). Question for supervisor about x86-64 (X-39, plumb F5(3) expect 0).
Next: 0g `cargo xtask static-identical`, baseline cut on top of the Q7 commit.
