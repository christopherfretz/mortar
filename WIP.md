# WIP WP-045 (M2-DYNAMIC)

Done: mortar_synth/src/{dynamic,interp}.rs (plan + write, unit tests);
mortar_image/src/dynamic.rs (value resolver, DynamicChunk, _DYNAMIC, D-32
reserved words, map records). lib.rs module lines in both crates.

Next: tests/dynamic.rs in mortar_image: oracle against GNU ld outputs
(dyntags t.c variants), splice into GNU output + llvm-readelf -d, run;
record spec/landed/dyn/WP-045.md; gate.
