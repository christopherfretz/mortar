#!/usr/bin/env python3
"""i386how.py: per i386 object, every relocation that reaches the object's own exported
data, by kind and by how it names it (SYM: the definition's symbol; SEC: a local or
section symbol whose S+A lies inside it).  usage: i386how.py OBJ...  -> counts per (kind, how)."""
import sys, collections
from elf import load_any, SHF_ALLOC, SHF_EXEC
import s1d
NAMES = {1: 'R_386_32', 2: 'PC32', 3: 'GOT32', 4: 'PLT32', 9: 'GOTOFF', 10: 'GOTPC', 43: 'GOT32X'}
tot = collections.Counter()
for f in sys.argv[1:]:
    for name, o in load_any(f):
        pd = s1d.preemptible_data(o)
        own = {y.name for v in pd.values() for y in v}
        for tgt, rels in o.relsecs():
            if not tgt.flags & SHF_ALLOC or tgt.name == '.eh_frame':
                continue
            for r in rels:
                if r.sym >= len(o.syms):
                    continue
                y = o.syms[r.sym]
                k = NAMES.get(r.type, str(r.type)) + ('@code' if tgt.flags & SHF_EXEC else '@data')
                if y.bind != 0 and y.name in own:
                    tot[(k, 'SYM')] += 1
                elif y.bind == 0 and y.shndx in pd:
                    a = y.value + r.addend
                    if any(d.value <= a < d.value + max(d.size, 1) for d in pd[y.shndx]):
                        tot[(k, 'SEC')] += 1
for k, v in sorted(tot.items()):
    print(*k, v)
