#!/usr/bin/env python3
"""onlyrule.py OBJ...: per object, whether D-117's address-level rule refuses it
(rulehit) and whether a non-GOT use NAMES an exported data definition by
symbol (symnamed: what GNU ld, lld and mold refuse).  Objects refused only by
the address rule are the ones the references link and mortar refuses."""
import sys, os, collections
sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
import relscan
class A: hits = False
only = both = 0
for path in sys.argv[1:]:
    summ = collections.Counter(); f = {}
    for n, d in relscan.members(path):
        relscan.scan(n, d, [], summ, f, A)
    for k, fe in f.items():
        r = bool(fe['rulehit']); s = bool(fe['symnamed'])
        if r and not s: only += 1; print('only-rule\t%s' % os.path.basename(str(k)))
        elif r and s: both += 1
print('refused by the rule: %d; of them with no symbol-named use (the references link them): %d' % (only + both, only))
