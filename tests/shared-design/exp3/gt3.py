#!/usr/bin/env python3
"""gt3.py: the detection table for S1d (WP-110 third pass).

usage: gt3.py [--mode=..] [--calls=..] LABEL PIC_OBJ PIE_OBJ [PIC_OBJ PIE_OBJ ...]
       gt3.py [...] LABEL @PAIRFILE          (one "PIC PIE" pair per line)

Bound definitions (ground truth): the object's own exported data that the
-fPIC twin reaches through the GOT (the -fPIC compiler's statement that the
code uses the datum and must not bind it).  In the -fPIE twin each bound
definition is: SYMBOL (a non-GOT relocation names it: D-117's table refuses
it), RULE (D-117's address rule names an address inside it), ANALYSIS (only
S1d's analysis derives an address inside it), UNSEEN.  An object is REFUSED
(anything refused), or ESCAPES (bound definitions and nothing refused).
Prints per object with bound definitions, then
  TABLE <label> objs users refused escapes defs symbol rule analysis unseen
"""
import sys
from elf import load_any, SHF_ALLOC
import s1d

GOT = s1d.GOT_KINDS


def one(p):
    return next(iter(load_any(p)))[1]


def own(o):
    return {y.name for v in s1d.preemptible_data(o).values() for y in v}


def gotted(o):
    ow, s = own(o), set()
    for tgt, rels in o.relsecs():
        for r in rels:
            if r.type in GOT and r.sym < len(o.syms) and o.syms[r.sym].name in ow and o.syms[r.sym].bind != 0:
                s.add(o.syms[r.sym].name)
    return s


def table_named(o):
    ow, s = own(o), set()
    for tgt, rels in o.relsecs():
        if not tgt.flags & SHF_ALLOC or tgt.name == '.eh_frame' or tgt.name.startswith('.gcc_except'):
            continue
        for r in rels:
            if r.type in GOT or r.type == 0 or r.sym >= len(o.syms):
                continue
            y = o.syms[r.sym]
            if y.bind != 0 and y.name in ow:
                s.add(y.name)
    return s


def main():
    args = sys.argv[1:]
    opts = [a for a in args if a.startswith('--')]
    args = [a for a in args if not a.startswith('--')]
    mode = next((a[7:] for a in opts if a.startswith('--mode=')), 'flow')
    calls = next((a[8:] for a in opts if a.startswith('--calls=')), 'keep')
    label, rest = args[0], args[1:]
    pairs = []
    for a in rest:
        if a.startswith('@'):
            for line in open(a[1:]):
                p = line.split()
                if len(p) == 2:
                    pairs.append(p)
        else:
            pairs.append(a)
    if pairs and isinstance(pairs[0], str):
        pairs = [pairs[i:i + 2] for i in range(0, len(pairs), 2)]
    t = dict(objs=0, users=0, refused=0, escapes=0, defs=0, symbol=0, rule=0, analysis=0, unseen=0)
    for a, b in pairs:
        try:
            pa, pb = one(a), one(b)
        except Exception:
            continue
        t['objs'] += 1
        bound = gotted(pa) & own(pb)
        if not bound:
            continue
        t['users'] += 1
        h = []
        r = s1d.analyse(pb, mode, calls, 'refined', h)
        rn = {x[4] for x in h if x[0] == 'RULE'}
        an = {x[4] for x in h if x[0] == 'ANA'}
        tn = table_named(pb)
        refused = bool(tn) or r['rule'] > 0 or r['ana'] > 0
        un, via = [], []
        for d in sorted(bound):
            t['defs'] += 1
            if d in tn:
                t['symbol'] += 1
            elif d in rn:
                t['rule'] += 1
            elif d in an:
                t['analysis'] += 1
                via.append(d)
            else:
                t['unseen'] += 1
                un.append(d)
        t['refused' if refused else 'escapes'] += 1
        print('REFUSED' if refused else 'ESCAPES', b, 'bound', len(bound), 'analysis', ' '.join(via) or '-', 'unseen', ' '.join(un) or '-')
    print('TABLE', label, ' '.join('%s=%d' % kv for kv in t.items()))


if __name__ == '__main__':
    main()
