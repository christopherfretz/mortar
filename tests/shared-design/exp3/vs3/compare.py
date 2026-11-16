#!/usr/bin/env python3
"""compare.py: both corrected models (model_a.py, model_r.py), modes gnu and
mortar, against GNU ld (gnuref.json) on every case of cases.py.  A case whose
script has local: before global: in a node is the deliberate family (rule 8):
mortar accepts it, GNU ld refuses.  Prints each difference and a RESULT line."""
import json, re
from cases import CASES, OBJ
import model_a, model_r
ref = json.load(open('gnuref.json'))

def lf(text):
    for body in re.findall(r'\{([^{}]*(?:\{[^{}]*\}[^{}]*)*)\}', text):
        a = body.find('local:'); b = body.find('global:', a + 1) if a >= 0 else -1
        if a >= 0 and b >= 0: return True
    return False

tot = dict(cases=0, cmp=0)
diff = {}
fam = set()
for sid, files, o, extra in CASES:
    obj, names, vers = OBJ[o]
    text = '\n'.join(files); E = '--export-dynamic' in extra
    g = ref[sid]; tot['cases'] += 1
    if lf(text): fam.add(sid)
    for mname, M in (('A', model_a), ('R', model_r)):
        for mode in ('gnu', 'mortar'):
            r = M.decide(text, names, vers or None, mode, E)
            if isinstance(r, str): r = 'REFUSED'
            tot['cmp'] += 1
            if r != g:
                diff.setdefault((mname, mode), []).append(sid)
                print('DIFF', mname, mode, sid, '|', text, '| GNU', g if isinstance(g, str) else {k: v for k, v in g.items() if isinstance(r, str) or r.get(k) != v}, '| model', r if isinstance(r, str) else {k: v for k, v in r.items() if isinstance(g, str) or g.get(k) != v})
for k in sorted(diff):
    print('DIFFSET', k, len(diff[k]), ' '.join(diff[k]), 'all-rule8' if set(diff[k]) <= fam else 'NOT-ALL-RULE8')
print('FAMILY', len(fam), ' '.join(sorted(fam)))
print('RESULT cases', tot['cases'], 'model-runs', tot['cmp'], ' '.join('%s/%s=%d' % (a, b, len(v)) for (a, b), v in sorted(diff.items())) or 'no differences')
