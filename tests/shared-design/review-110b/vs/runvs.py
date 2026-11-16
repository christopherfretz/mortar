#!/usr/bin/env python3
"""Run every script of scripts.py through GNU ld, lld 22 and mold; compare with vmodel (gnu and mortar
modes) and, if given --author, with the author's vmatch2/model.py."""
import subprocess, os, sys, re, shutil
sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
from scripts import S
from vmodel import decide
NAMES = {'n': ['foo', 'foo2', 'fop', 'bar', 'baz', 'fa', '_Z3fooi', '_ZN2ns3fooEv', '_ZN2ns3barIiEEiv'],
         's': ['old', 'cur', 'bar', 'old_impl']}
VERS = {'n': None, 's': {'old': 'V1'}}
LD = {'gnu': [], 'lld': ['-fuse-ld=lld', '-B/Volumes/AgentWork/mortar-helper/.build/review-110-adv/lld22'], 'mold': ['-fuse-ld=mold']}
os.makedirs('work.run', exist_ok=True)
def link(sid, files, obj, extra, ld):
    paths = []
    for i, t in enumerate(files):
        p = f'work.run/{sid}.{i}.map'; open(p, 'w').write(t + '\n'); paths.append(p)
    out = f'work.run/{sid}.{ld}.so'
    if os.path.exists(out): os.unlink(out)
    cmd = ['gcc', '-shared', '-nostdlib', obj] + LD[ld] + [f'-Wl,--version-script={p}' for p in paths] + (extra.split() if extra else []) + ['-o', out]
    r = subprocess.run(cmd, capture_output=True, text=True)
    if r.returncode: return 'REFUSED', r.stderr.strip().splitlines()[-1][:120] if r.stderr.strip() else ''
    ds = subprocess.run(['readelf', '--dyn-syms', '-W', out], capture_output=True, text=True).stdout
    res = {}
    for l in ds.splitlines():
        p = l.split()
        if len(p) >= 8 and p[0].endswith(':') and p[6] != 'UND':
            nm = p[7]
            if '@@' in nm: n, v = nm.split('@@'); res.setdefault(n, v)
            elif '@' in nm: n, v = nm.split('@'); res[n + '@' + v] = v
            else: res[nm] = 'base'
    return res, (r.stderr.strip().splitlines()[0][:100] if r.stderr.strip() else '')
def norm(res, names, vers):
    if isinstance(res, str): return res
    out = {}
    for n in names:
        if vers and n in vers:
            out[n] = vers[n] if (n + '@' + vers[n]) in res else 'local'
        else:
            out[n] = res.get(n, 'local')
    return out
dis_gnu = dis_mortar = 0
rows = []
for sid, files, o, extra in S:
    obj = 'names.o' if o == 'n' else 'sv.o'
    names, vers = NAMES[o], VERS[o]
    ref = {}
    for ld in LD:
        r, msg = link(sid, files, obj, extra, ld)
        ref[ld] = norm(r, names, vers)
    text = '\n'.join(files)
    mg = decide(text, names, vers, 'gnu'); mm = decide(text, names, vers, 'mortar')
    if isinstance(mg, str): mg = 'REFUSED'
    if isinstance(mm, str): mm = 'REFUSED'
    g = ref['gnu']
    eq_g = (mg == g); eq_m = (mm == g)
    if not eq_g: dis_gnu += 1
    if not eq_m: dis_mortar += 1
    print(f'{sid} model-gnu==GNU:{eq_g} model-mortar==GNU:{eq_m}')
    if not (eq_g and eq_m):
        print('   GNU   ', g); print('   lld   ', ref['lld']); print('   mold  ', ref['mold']); print('   model ', mg); print('   mortar', mm)
print(f'SCRIPTS {len(S)} model(gnu)!=GNU {dis_gnu} model(mortar)!=GNU {dis_mortar}')
