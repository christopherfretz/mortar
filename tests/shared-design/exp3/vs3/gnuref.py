#!/usr/bin/env python3
"""gnuref.py: links every case of cases.py with GNU ld 2.44 (-shared) and
records each name's outcome (V / base / local, or REFUSED) in gnuref.json."""
import subprocess, json, os
from cases import CASES, OBJ
os.makedirs('ref', exist_ok=True)
res = {}
for sid, files, o, extra in CASES:
    obj, names, vers = OBJ[o]
    paths = []
    for i, t in enumerate(files):
        p = 'ref/%s.%d.map' % (sid, i); open(p, 'w').write(t + '\n'); paths.append(p)
    out = 'ref/%s.so' % sid
    r = subprocess.run(['gcc', '-shared', '-nostdlib', obj] + ['-Wl,--version-script=' + p for p in paths] + (extra.split() if extra else []) + ['-o', out], capture_output=True, text=True)
    if r.returncode:
        res[sid] = 'REFUSED'; continue
    ds = subprocess.run(['readelf', '--dyn-syms', '-W', out], capture_output=True, text=True).stdout
    have = {}
    for l in ds.splitlines():
        p = l.split()
        if len(p) >= 8 and p[0].endswith(':') and p[6] != 'UND':
            nm = p[7]
            if '@@' in nm: n, v = nm.split('@@')
            elif '@' in nm: n, v = nm.split('@')
            else: n, v = nm, 'base'
            have[n] = v
    res[sid] = {n: have.get(n, 'local') for n in names}
json.dump(res, open('gnuref.json', 'w'), indent=0, sort_keys=True)
print('GNUREF-DONE', len(res))
