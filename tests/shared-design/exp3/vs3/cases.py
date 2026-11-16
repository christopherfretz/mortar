"""cases.py: the union of version-script cases (M-5): the second pass's 53
(scripts_author.txt over lib.o), the re-review's 60 (scripts_rev.py over
names.o / sv.o) and this pass's: globs g00-g42 and duplicates d01-d17 over
odd.o, .symver cases e01-e13 / a01-a07 over sv3.o, each with and without -E.
CASES: (id, [file texts], obj, extra ld flags, names, versioned {name: V})."""
import re
from scripts_rev import S as REV
OBJ = {
 'L': ('lib.o', ['foo', '_Z3fooi', '_ZN2ns3fooEv'], {}),
 'n': ('names.o', ['foo', 'foo2', 'fop', 'bar', 'baz', 'fa', '_Z3fooi', '_ZN2ns3fooEv', '_ZN2ns3barIiEEiv'], {}),
 's': ('sv.o', ['old', 'cur', 'bar', 'old_impl'], {'old': 'V1'}),
 'o': ('odd.o', open('odd.names').read().split('\n')[:-1], {}),
 'v': ('sv3.o', ['old', 'cur', 'old_impl', 'cur_impl', 'bar', 'foo', 'fop'], {'old': 'V1', 'cur': 'V1'}),
}
CASES = []
for l in open('scripts_author.txt'):
    if '|' in l:
        k, s = l.rstrip('\n').split('|', 1)
        CASES.append(('A-' + k, [s], 'L', ''))
for sid, files, o, extra in REV:
    CASES.append(('R-' + sid, files, o, extra))
for l in open('globs.out'):
    m = re.match(r'(g\d\d) ', l)
    if m:
        CASES.append(('G-' + m.group(1), [open(m.group(1) + '.map').read().strip()], 'o', ''))
for l in open('dups.out'):
    m = re.match(r'(d\d\d) ', l)
    if m:
        CASES.append(('D-' + m.group(1), [open('out/%s.map' % m.group(1)).read().strip()], 'o', ''))
for f in ('expdyn.out', 'atat.out'):
    for l in open(f):
        m = re.match(r'([ea]\d\d) \S+\s+- ', l)
        if m:
            t = open('out/%s.map' % m.group(1)).read().strip()
            CASES.append(('S-' + m.group(1), [t], 'v', ''))
            CASES.append(('S-' + m.group(1) + 'E', [t], 'v', '-Wl,--export-dynamic'))
