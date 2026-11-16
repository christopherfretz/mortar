#!/usr/bin/env python3
"""expdyn.py: which rules of resolve 9.7 --export-dynamic (-E) changes on a -shared link (M-3).
Each script is linked over sv3.o (old@V1 via .symver, cur@@V1, old_impl, cur_impl, bar, foo, fop)
with and without -Wl,--export-dynamic by GNU ld, lld 22 and mold; prints each defined dynsym as name@ver."""
import subprocess
S = {
 'e01 exact-local':      'V1 { global: bar; local: foo; };',
 'e02 glob-local':       'V1 { global: bar; local: f*; };',
 'e03 star-local':       'V1 { global: bar; local: *; };',
 'e04 anon-star':        '{ global: bar; local: *; };',
 'e05 V2-star':          'V1 { global: bar; }; V2 { local: *; };',
 'e06 sv-exact-local':   'V1 { global: bar; local: old; };',
 'e07 sv-glob-local':    'V1 { global: bar; local: o*; };',
 'e08 sv-star-local':    'V1 { local: *; };',
 'e09 sv-g-and-l':       'V1 { global: o*; local: old; };',
 'e10 sv-V2local':       'V1 { global: bar; }; V2 { local: old; };',
 'e11 atat-local':       'V1 { global: bar; local: cur; };',
 'e12 atat-star':        'V1 { global: bar; local: *; }; V2 { global: foo; } V1;',
 'e13 impl-local':       'V1 { global: bar; local: old_impl; };',
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
            out = 'out/e-%s-%d.so' % (k, n)
            r = subprocess.run(cmd + ['-shared', '-nostdlib', '-o', out, 'sv3.o', '-Wl,--version-script=' + f] + ([E] if E else []), capture_output=True, text=True)
            if r.returncode:
                row.append('%s=ERR' % k); continue
            d = subprocess.run(['readelf', '-W', '--dyn-syms', out], capture_output=True, text=True).stdout
            ex = sorted({l.split()[7] for l in d.splitlines() if len(l.split()) >= 8 and l.split()[6] != 'UND' and l.split()[3] == 'FUNC'})
            row.append('%s={%s}' % (k, ' '.join(ex)))
        print('%-20s %-3s %s' % (sid, 'E' if E else '-', ' | '.join(row)))
    print('    ', text)
print('EXPDYN-DONE')
