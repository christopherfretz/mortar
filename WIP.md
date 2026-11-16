# WP-089 WIP (deleted in the final commit)

Done: static-pie judged (line.rs no longer outside; -E static-pie outside, M4);
P-D14/P-D15 in src/spie.rs; IFUNC address words that are their own IRELATIVE
slot (ifunc.rs); absolute-symbol words non-internal (reloc.rs, x64rec.rs);
piece and IPLT abs words for view 2; non-address RELATIVE check (dynverify.rs);
pd10/plt::check static-pie variant; anomalies A30-A32.
Corpus (/tmp/mortar-wp089-1027661, 112 outputs): only A30/A31/A32 left.
Order measured: x86-64 defers IRELATIVE in a range, AArch64 does not.

Next: unit tests for spie.rs; dynmutate classes spie-*; kill table + sweep;
tests/spie.rs; tests/dyn.rs rerun; cost; plumb.md §12.4; record; gate.
