#!/usr/bin/env python3
"""Ground truth for the anchor gap (AArch64): pairs PIC_OBJ PIE_OBJ (same source).
bound defs = own exported data that the -fPIC twin reaches through the GOT.
For the -fPIE twin: each bound def is SYMBOL (a non-GOT reloc names it: D-117's table),
RULE (the address rule names it), or UNSEEN. Object verdict: REFUSED (any refusal: table
or rule on ANY definition) or ESCAPES (bound defs >0 and no refusal at all)."""
import sys
from elf import load_any, SHF_ALLOC, SHF_EXEC
from rule import preemptible_data, scan
GOT = {311, 312, 313, 309, 310, 300, 301, 302, 303, 304, 305, 306}
TLSIE = set(range(539, 552)) | set(range(560, 576))
def one(p):
    return next(iter(load_any(p)))[1]
def own_names(o):
    return {y.name: y for v in preemptible_data(o).values() for y in v}
def gotted(o):
    own = own_names(o); s = set()
    for tgt, rels in o.relsecs():
        for r in rels:
            if r.type in GOT and r.sym < len(o.syms) and o.syms[r.sym].name in own: s.add(o.syms[r.sym].name)
    return s
def table_named(o):
    own = own_names(o); s = set()
    for tgt, rels in o.relsecs():
        if not tgt.flags & SHF_ALLOC or tgt.name == '.eh_frame' or tgt.name.startswith('.gcc_except'): continue
        for r in rels:
            if r.type in GOT or r.type == 0 or r.sym >= len(o.syms): continue
            y = o.syms[r.sym]
            if y.bind != 0 and y.name in own and r.type not in (283, 282): s.add(y.name)
    return s
def rule_named(o):
    pd = preemptible_data(o); s = set()
    for tgt, rels in o.relsecs():
        if not tgt.flags & SHF_ALLOC or tgt.name == '.eh_frame' or tgt.name.startswith('.gcc_except'): continue
        for r in rels:
            if r.type == 0 or r.sym >= len(o.syms): continue
            y = o.syms[r.sym]
            if y.bind != 0 or y.shndx not in pd: continue
            a = y.value + r.addend
            for d in pd[y.shndx]:
                if d.value <= a < d.value + max(d.size, 1): s.add(d.name)
    return s
tot = dict(objs=0, users=0, refused=0, escapes=0, defs=0, sym=0, rule=0, unseen=0)
args = sys.argv[1:]
for i in range(0, len(args), 2):
    try: a, b = one(args[i]), one(args[i+1])
    except Exception as e: continue
    tot['objs'] += 1
    bound = gotted(a) & set(own_names(b))
    if not bound: continue
    tot['users'] += 1
    tn, rn = table_named(b), rule_named(b)
    refused = bool(table_named(b) & set(own_names(b))) or scan(b)[0] > 0
    un = []
    for d in bound:
        tot['defs'] += 1
        if d in tn: tot['sym'] += 1
        elif d in rn: tot['rule'] += 1
        else: tot['unseen'] += 1; un.append(d)
    tot['refused' if refused else 'escapes'] += 1
    print('REFUSED' if refused else 'ESCAPES', args[i+1], 'bound', len(bound), 'unseen', ' '.join(sorted(un)))
print('TOTAL', tot)
