# WIP — WP-250 (mortar_plumb: spec/image.md I-47a, the reference columns)

(The base 70c8021c carried WP-249's WIP.md; this file replaces it and is
deleted in WP-250's final commit.)

Done: debug.rs i47a/gnu_kept/kept_copy (Via::KeptCopy); fate.rs keepers;
locate.rs non-alloc bytes + the §5.7a tie-break (§3.3 steps 3-4);
tests/macro_import.rs + fixtures (gen.sh); spec/plumb.md §3.3, §5.8, §7.2, §10.6.

Next: run every mortar_plumb test; fix tests relying on the old GNU/mold
columns; record spec/landed/plumb/WP-250.md; full gate.
Scratch: .build/target-wp-156/wp250/scripts (t.sh <test>, bx.sh, wl.sh,
probe.sh, rprobe.sh); each prints a final line.
