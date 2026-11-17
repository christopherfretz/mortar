# WIP: WP-136 (placeless symbols)

Done (committed): spec P-1/§10.1, model §4.5 invariant, image I-124,
layout L-162; mortar_core::place; Valued in image; scan/report; dynsym
filter; link tests place-empty-merge, place-note-property,
place-dropped-global (all PASS on 3 targets; fail with mortar-before).

Running: after2 matrix + item-7 scan (scratch bg.log); static-identical
waits for load.

Next: static-identical (--update for new AArch64 rows), gate, lane,
record spec/landed/resolve/WP-136.md, delete this file.
