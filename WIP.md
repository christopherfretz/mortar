# WIP — WP-121 part 2 (plumb stage 3, shared objects)

Branch wp/121-plumb-shared; int/batch-5 merged (fcef136 lineage).

Done in part 2:
- P-D4 RELRO end at the line's maxpage (spec P-D4 text, dyncheck pd4, test p_d4_relro_ends_on_the_lines_maxpage: bfd 0x2000, lld 0x3000, mold 0x3000 ends).
- A44 resolution in judge checked: correct (sets is Some iff shared.line is Some).
- SharedLine takes the -B family from Line.symbolic (DT_SYMBOLIC/DF_SYMBOLIC kept: dynverify::SYMBOLIC is a message).

Next:
- P-D17 address clause + sharedflow.rs; compare with exp3/s1d.py.
- P-D23 mutation table (dynmutate.rs).
- §8.1 sizing on the largest AArch64 bench link.
- Record, merge int/batch-5, final gate, delete this file.
