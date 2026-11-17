# WIP — WP-121 part 2 (plumb stage 3, shared objects)

Branch wp/121-plumb-shared; int/batch-5 merged (fcef136 lineage).

Done in part 2:
- SharedLine takes the -B family from Line.symbolic (DT_SYMBOLIC/DF_SYMBOLIC kept: dynverify::SYMBOLIC is a message).

Next:
- Review judge's A44 resolution against §10.2 / P-D22.
- P-D4 RELRO end from the line's page sizes (dyncheck.rs).
- P-D17 address clause + sharedflow.rs; compare with exp3/s1d.py.
- P-D23 mutation table (dynmutate.rs).
- §8.1 sizing on the largest AArch64 bench link.
- Record, merge int/batch-5, final gate, delete this file.
