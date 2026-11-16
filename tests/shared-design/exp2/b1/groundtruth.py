#!/usr/bin/env python3
"""groundtruth.py OBJDIR COMP: how much of GCC/Clang -fPIE's local binding of
exported data the address-level rule (relscan.py) sees.

Ground truth: the same source compiled -fPIC names, through a GOT-class use,
every exported data definition its code reads or writes; in the -fPIE object
of the same source those definitions are bound locally (no GOT use names
them).  For each source: the accessed exported data definitions (from the
-fPIC object), and which of them the rule reports in the -fPIE object (a
CODE/DATA-place non-GOT use of a local symbol whose address lies inside the
definition).  Object-level: is the -fPIE object refused at all (any hit)?
"""
import sys, os, glob, collections
sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
import relscan

class A: hits = False

def facts(path):
    summ = collections.Counter(); f = {}
    for n, d in relscan.members(path):
        relscan.scan(n, d, [], summ, f, A)
    return next(iter(f.values()), None)

objdir, comp = sys.argv[1], sys.argv[2]
tot_src = tot_acc = tot_defs = tot_hitdefs = obj_acc = obj_caught = 0
missed = []
for pic in sorted(glob.glob(os.path.join(objdir, comp + '.pic.*.o'))):
    pie = pic.replace('.pic.', '.pie.', 1)
    if not os.path.exists(pie): continue
    fp, fe = facts(pic), facts(pie)
    if fp is None or fe is None: continue
    tot_src += 1
    accessed = {nm for (t, nm) in fp['gotnamed'] if any(nm == d for (_, d) in fp['datadefs'])}
    # GOT uses in the -fPIE object name extern data; keep only own exported data defs
    pie_defs = {nm for (_, nm) in fe['datadefs']}
    accessed &= pie_defs
    if not accessed: continue
    obj_acc += 1
    hit = {nm for (_, nm) in fe['rulehit']}
    pie_got = {nm for (_, nm) in fe['gotnamed']}
    bound = accessed - pie_got          # accessed and not through the GOT in -fPIE
    sym = {nm for (_, nm) in fe['symnamed']} & bound
    tot_defs += len(bound); tot_hitdefs += len(bound & hit); tot_sym = globals().get('tot_sym', 0) + len(sym - hit); globals()['tot_sym'] = tot_sym
    if hit or sym: obj_caught += 1
    elif bound: missed.append((os.path.basename(pie), sorted(bound)))
    for nm in sorted(bound - hit - sym):
        print('unseen-def\t%s\t%s' % (os.path.basename(pie), nm))
print('%s sources %d; objects whose code accesses its own exported data %d; -fPIE objects refused (the new rule, or D-117 against the symbol) %d; '
      'locally bound defs %d, of which named by a use the rule sees %d' % (comp, tot_src, obj_acc, obj_caught, tot_defs, tot_hitdefs))
print('defs named by D-117 (symbol) only: %d' % globals().get('tot_sym', 0))
for o, b in missed: print('object-missed\t%s\t%s' % (o, ','.join(b)))
