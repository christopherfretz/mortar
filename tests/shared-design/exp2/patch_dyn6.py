import sys
p = sys.argv[1]; s = open(p).read()
def rep(old, new):
    global s
    if s.count(old) != 1: raise SystemExit('count %d: %r' % (s.count(old), old[:70]))
    s = s.replace(old, new)
rep('''| one entry under `global:` of one node and `local:` of another | refused (`duplicate expression`) | accepted, first | accepted, varies |''',
'''| one entry under `global:` of one node and `local:` of another | refused (`duplicate expression`) | accepted, answers vary | accepted, answers vary |''')
rep('''| mold | vmatch2 S16, X7 (a C++ `*` in one node, C names in another): no name exported; S19, X6: the last node | GNU ld's answer | `exp2/vmatch2/result.txt` |''',
'''| mold | vmatch2 S16 (`V1 { global: *; local: *; }`) and X7 (`extern "C++" { *; }; local: *`, W40): no name exported; S19 (an exact name in two nodes): the last node; X6 (an exact `extern "C++"` name in V1, `_Z*` in V2): V2 | GNU ld's answer | `exp2/vmatch2/result.txt` |''')
open(p, 'w').write(s)
