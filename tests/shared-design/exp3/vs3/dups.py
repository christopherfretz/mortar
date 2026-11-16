#!/usr/bin/env python3
"""dups.py: GNU ld / lld / mold on the duplicate-pattern rule (M-4): which pairs of
entries in global: of one node and local: of another are "the same entry"."""
import subprocess
S = {
 'd01': 'V1 { global: foo; }; V2 { local: "foo"; };',
 'd02': 'V1 { global: fo\\o; }; V2 { local: foo; };',
 'd03': 'V1 { global: "fo*"; }; V2 { local: fo\\*; };',
 'd04': 'V1 { global: fo*; }; V2 { local: "fo*"; };',
 'd05': 'V1 { global: fo*; }; V2 { local: fo*; };',
 'd06': 'V1 { global: f[o]o; }; V2 { local: foo; };',
 'd07': 'V1 { global: fo?; }; V2 { local: fo[?]; };',
 'd08': 'V1 { global: fo\\*; }; V2 { local: fo\\*; };',
 'd09': 'V1 { global: foo; }; V2 { local: foo; };',
 'd10': 'V1 { global: foo; }; V2 { local: foo; }; V3 { global: foo; };',
 'd11': 'V1 { global: foo; local: foo; };',
 'd12': 'V1 { global: foo; }; V2 { global: foo; };',
 'd13': 'V1 { local: foo; }; V2 { local: foo; };',
 'd14': 'V1 { global: *; }; V2 { local: *; };',
 'd15': 'V1 { global: fo*; }; V2 { local: fo**; };',
 'd16': 'V1 { global: extern "C++" { foo; }; }; V2 { local: foo; };',
 'd17': 'V1 { global: fo\\*; }; V2 { local: "fo*"; };',
}
LD = {'gnu': ['gcc'], 'lld': ['gcc', '-B/Volumes/AgentWork/mortar-helper/.build/review-110-adv/lld22', '-fuse-ld=lld'], 'mold': ['gcc', '-fuse-ld=mold']}
n = 0
for sid, text in S.items():
    f = 'out/%s.map' % sid
    open(f, 'w').write(text + '\n')
    row = []
    for k, cmd in LD.items():
        n += 1
        out = 'out/d-%s-%d.so' % (k, n)
        r = subprocess.run(cmd + ['-shared', '-nostdlib', '-o', out, 'odd.o', '-Wl,--version-script=' + f], capture_output=True, text=True)
        if r.returncode:
            msg = [l for l in r.stderr.splitlines() if 'collect2' not in l]
            row.append('%s=ERR(%s)' % (k, (msg[0].split(': ', 1)[-1] if msg else '?')[:60])); continue
        d = subprocess.run(['readelf', '-W', '--dyn-syms', out], capture_output=True, text=True).stdout
        v = {l.split()[7].split('@')[0]: l.split()[7] for l in d.splitlines() if len(l.split()) >= 8 and l.split()[6] != 'UND' and l.split()[3] == 'FUNC'}
        row.append('%s=foo:%s fo*:%s' % (k, v.get('foo', 'local'), v.get('fo*', 'local')))
    print('%s %-60s %s' % (sid, text, ' | '.join(row)))
print('DUPS-DONE')
