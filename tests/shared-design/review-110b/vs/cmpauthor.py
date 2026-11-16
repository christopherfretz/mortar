import sys, os
sys.path.insert(0, '.')
from vmodel import decide
from scripts import S
src = open('/Volumes/AgentWork/mortar-helper/.build/target-wp-110/exp2/vmatch2/model.py').read()
src = src[:src.index("for line in open(sys.argv[1])")]
A = {}; exec(src, A)
NAMES = ['foo', 'foo2', 'fop', 'bar', 'baz', 'fa', '_Z3fooi', '_ZN2ns3fooEv', '_ZN2ns3barIiEEiv']
def author(text, sym, mode):
    nodes, lf = A['parse'](text)
    if A['duplicate'](nodes): return 'REFUSED'
    if mode == 'gnu' and lf: return 'REFUSED'
    m = A['decide'](nodes, sym)
    return {'<anon>': 'base', 'BASE': 'base', 'LOCAL': 'local'}.get(m, m)
union = [(sid, ' '.join(files)) for sid, files, o, e in S if o == 'n'] + \
        [tuple(l.rstrip('\n').split('|', 1)) for l in open('/Volumes/AgentWork/mortar-helper/.build/target-wp-110/exp2/vmatch2/scripts.txt') if '|' in l]
dis = 0; n = 0
for sid, text in union:
    for mode in ('gnu', 'mortar'):
        mine = decide(text, NAMES, None, mode)
        for sym in NAMES:
            n += 1
            try: a = author(text, sym, mode)
            except Exception as e: a = 'ERR:' + str(e)[:40]
            m = 'REFUSED' if isinstance(mine, str) else mine[sym]
            if a != m:
                dis += 1; print('DIFF', sid, mode, sym, 'mine', m, 'author', a, '|', text)
print('UNION scripts', len(union), 'comparisons', n, 'disagreements', dis)
