# WP-110 work in progress (deleted in the final commit)

## Written
- record A (census), B (inventory); skeleton for D, E, appendices.
- dyn.md §17 drafts outside the tree, not yet inserted:
  `.build/target-wp-110/s17-part1.md` (D-100..D-103),
  `.build/target-wp-110/s17-part3.md` (D-119..D-125, D-130).
- experiment tree `.build/target-wp-110/exp/`, `run-all.sh WORK`.

## Rule numbering (dyn.md §17)
D-100 kind column; D-101 entry; D-102 .dynamic; D-103 DF_1 options;
D-110 export set; D-111 dynsym entries/imports/weak undef; D-112 preemption;
D-113 model column; D-114 version scripts applied; D-115 verdefs; D-116 dynstr;
D-117 relocation classes; D-118 undefined/-z defs/needed; D-119 TLS; D-120 PLT;
D-121 IFUNC; D-122 no COPY/canon; D-123 layout; D-124 gc roots; D-125 tools;
D-126 ODR; D-127 incr; D-130 i386. §18 requests (R-*, P-R*).
Wrong outputs continue §13.2: W19 bfd a64 LE local; W20 mold exported IFUNC;
W21 bfd/mold IFUNC inequality; W22 mold -init gc; W23 lld start/stop+vers;
W24 lld .ctors; W25 bfd i386 clang -fno-pic eh_frame; W26 lld i386 IFUNC;
W27 mold i386 hidden IFUNC; W28 bfd i386 -shared -static C++; W29 mold i386
-shared -static C; W30 -Bsymbolic + exe COPY (all, tools part d).

## Verified
- areas census, dyn, tools done (reports in .build/target-wp-110/work/).

## Next step
- export report -> D-110..D-118, resolve §9.6/§9.7, script.md §9; insert
  §17 + §18 into dyn.md; record D, E, appendices; run-all.sh `design`.
