#!/usr/bin/env python3
"""globs.py: GNU ld / lld / mold on version-script glob syntax (M-5).
For each pattern P: script `V1 { global: P; local: *; };` linked -shared over
odd.o; prints which of odd.names are exported (dynsym, defined)."""
import subprocess, sys, os
names = open('odd.names').read().split('\n')[:-1]
pats = ['f[^o]o', 'f[!o]o', 'f[^o]*', 'f[!o]*', 'f[a-o]?', 'f[o-a]?', '[]]oo', '[]o]oo', '[!]]oo', '[^]]oo',
        'f[o', '[oo', 'fo\\*', 'fo*', '"fo*"', 'fo\\o', 'foo\\', '"foo\\\\"', 'foo\\\\', 'foo*', 'fo[o]*', 'f*o', 'f*', '*o', '[[]oo',
        '[-]oo', '[a-]oo', '[\\]]oo', 'f[.]o', 'f?o', '???', '\\fo?', 'fo[*]', '[!a-z]oo', '[^a-z]oo', '[o*', 'f[o*', 'f*\\', 'foo*\\', 'fo[o-o]\\', '*', 'fo[]', 'fo[!]']
LD = {'gnu': ['gcc'], 'lld': ['gcc', '-B/Volumes/AgentWork/mortar-helper/.build/review-110-adv/lld22', '-fuse-ld=lld'], 'mold': ['gcc', '-fuse-ld=mold']}
n = 0
for i, p in enumerate(pats):
    open('g%02d.map' % i, 'w').write('V1 {\n  global: %s;\n  local: *;\n};\n' % p)
    row = []
    for k, cmd in LD.items():
        n += 1
        out = 'out/%s-g%02d-%d.so' % (k, i, n)
        r = subprocess.run(cmd + ['-shared', '-nostdlib', '-o', out, 'odd.o', '-Wl,--version-script=g%02d.map' % i], capture_output=True, text=True)
        if r.returncode:
            row.append('%s=ERR(%s)' % (k, r.stderr.strip().splitlines()[-1][:70] if r.stderr.strip() else '?'))
            continue
        d = subprocess.run(['readelf', '-W', '--dyn-syms', out], capture_output=True, text=True).stdout
        ex = sorted({l.split()[7].split('@')[0] for l in d.splitlines() if len(l.split()) >= 8 and l.split()[6] != 'UND' and l.split()[3] in ('FUNC',)})
        row.append('%s={%s}' % (k, ' '.join(ex)))
    print('g%02d %-12s %s' % (i, p, ' | '.join(row)))
print('GLOBS-DONE')
