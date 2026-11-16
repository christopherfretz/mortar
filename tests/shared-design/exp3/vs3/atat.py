#!/usr/bin/env python3
"""atat.py (from expdyn.py): how .symver @/@@ definitions meet other nodes; which rules of resolve 9.7 --export-dynamic (-E) changes on a -shared link (M-3).
Each script is linked over sv3.o (old@V1 via .symver, cur@@V1, old_impl, cur_impl, bar, foo, fop)
with and without -Wl,--export-dynamic by GNU ld, lld 22 and mold; prints each defined dynsym as name@ver."""
import subprocess
S = {
 'a01 atat-V2global':    'V1 { global: bar; }; V2 { global: cur; };',
 'a02 atat-V2glob':      'V1 { global: bar; }; V2 { global: c*; };',
 'a03 atat-V1glob-loc':  'V1 { global: bar; local: c*; };',
 'a04 atat-g-and-l':     'V1 { global: c*; local: cur; };',
 'a05 sv-V2global':      'V1 { global: bar; }; V2 { global: old; };',
 'a06 sv-V1-global':     'V1 { global: old; local: *; };',
 'a07 atat-V2star':      'V1 { global: bar; }; V2 { global: *; };',
}
LD = {'gnu': ['gcc'], 'lld': ['gcc', '-B/Volumes/AgentWork/mortar-helper/.build/review-110-adv/lld22', '-fuse-ld=lld'], 'mold': ['gcc', '-fuse-ld=mold']}
n = 0
for sid, text in S.items():
    f = 'out/%s.map' % sid.split()[0]
    open(f, 'w').write(text + '\n')
    for E in ('', '-Wl,--export-dynamic'):
        row = []
        for k, cmd in LD.items():
            n += 1
            out = 'out/a-%s-%d.so' % (k, n)
            r = subprocess.run(cmd + ['-shared', '-nostdlib', '-o', out, 'sv3.o', '-Wl,--version-script=' + f] + ([E] if E else []), capture_output=True, text=True)
            if r.returncode:
                row.append('%s=ERR' % k); continue
            d = subprocess.run(['readelf', '-W', '--dyn-syms', out], capture_output=True, text=True).stdout
            ex = sorted({l.split()[7] for l in d.splitlines() if len(l.split()) >= 8 and l.split()[6] != 'UND' and l.split()[3] == 'FUNC'})
            row.append('%s={%s}' % (k, ' '.join(ex)))
        print('%-20s %-3s %s' % (sid, 'E' if E else '-', ' | '.join(row)))
    print('    ', text)
print('ATAT-DONE')
