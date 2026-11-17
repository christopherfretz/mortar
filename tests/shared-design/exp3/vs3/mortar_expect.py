#!/usr/bin/env python3
"""mortar_expect.py (WP-125 part 2): the 213 cases of cases.py with the
author's model's MORTAR reading (model_a.decide, mode 'mortar'), written
as the fixture of crates/mortar_resolve/tests/version_table.rs. No linker
runs; the dups/expdyn/atat maps are written to out/ first, as
remeasure.py does. Needs c++filt (the model demangles for extern "C++").

Fixture format, one block per case:
  case <id> <E|->            (E: --export-dynamic)
  names <name> ...
  versioned <name>=<V> ...   (the .symver definitions; may be empty)
  file                       (one per script file, then its lines:)
  | <line>
  expect REFUSED | <name>=<V|base|local> ...
Usage: python3 mortar_expect.py > ../../../../crates/mortar_resolve/tests/data/version_cases.txt
"""
import os
os.makedirs('out', exist_ok=True)
for f in ('dups.py', 'expdyn.py', 'atat.py'):
    src = open(f).read()
    ns = {}
    exec(src[:src.index('\nLD =')], ns)
    for sid, text in ns['S'].items():
        open('out/%s.map' % sid.split()[0], 'w').write(text + '\n')
from cases import CASES, OBJ
import model_a

for sid, files, o, extra in CASES:
    obj, names, vers = OBJ[o]
    E = '--export-dynamic' in extra
    print('case', sid, 'E' if E else '-')
    print('names', *names)
    print('versioned', *('%s=%s' % kv for kv in sorted((vers or {}).items())))
    for t in files:
        print('file')
        for line in t.split('\n'):
            print('|', line)
    r = model_a.decide('\n'.join(files), names, vers or None, 'mortar', E)
    print('expect', r if isinstance(r, str) else ' '.join('%s=%s' % (n, r[n]) for n in names))
