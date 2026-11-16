#!/usr/bin/env python3
"""Candidate closures of the anchor gap (Q21), AArch64 objects.
OVER : a non-GOT code/data use through a local symbol of a section T holding an
       exported preemptible data definition (the author's over-approximation).
S1   : OVER and the object has no GOT relocation against any of its own exported data.
S2   : S1 restricted to uses whose address is <= a definition's start in T and within
       32 KiB below it (an anchor that an immediate can carry into the definition).
prints per object the flags; totals."""
import sys
from elf import load_any, SHF_ALLOC, SHF_EXEC, EM_AARCH64
from rule import preemptible_data
GOT = {311, 312, 313, 309, 310, 300, 301, 302, 303, 304, 305, 306}
TLS = set(range(512, 580))
tot = dict(objs=0, over=0, s1=0, s2=0)
for f in sys.argv[1:]:
    for name, o in load_any(f):
        if o.mach != EM_AARCH64: continue
        tot['objs'] += 1
        pd = preemptible_data(o)
        own = {y.name for v in pd.values() for y in v}
        selfgot = False; over = s2 = False
        for tgt, rels in o.relsecs():
            if not tgt.flags & SHF_ALLOC or tgt.name == '.eh_frame' or tgt.name.startswith('.gcc_except'): continue
            for r in rels:
                if r.sym >= len(o.syms) or r.type == 0: continue
                y = o.syms[r.sym]
                if r.type in GOT and y.name in own: selfgot = True
                if y.bind != 0 or y.shndx not in pd or r.type in GOT or r.type in TLS: continue
                over = True
                a = y.value + r.addend
                if any(a <= d.value <= a + 32760 for d in pd[y.shndx]): s2 = True
        fl = []
        if over: tot['over'] += 1; fl.append('OVER')
        if over and not selfgot: tot['s1'] += 1; fl.append('S1')
        if s2 and not selfgot: tot['s2'] += 1; fl.append('S2')
        if fl and '--v' in sys.argv[0:0] or fl: print(' '.join(fl), name)
print('TOTAL', tot)
