#!/usr/bin/python3 -I
# An independent model of resolve.md 9.7 rules 1-5 (WP-110 text), read literally:
# 1 exact before wildcard (any node, global or local); 2 among wildcards the LAST node wins;
# 3 inside one node a global match beats a local match of the same kind;
# 4 wildcard local never overrides an exact global, exact local overrides wildcard global (=1);
# 5 exact name in two nodes: the first. Prints the version of `foo` or LOCAL.
import re, sys, fnmatch
def parse(s):
    nodes = []
    for m in re.finditer(r'(\w*)\s*\{([^}]*)\}\s*[\w ]*;', s):
        name, body = m.group(1) or '<anon>', m.group(2); scope = 'global'; pats = []
        for tok in [t.strip() for t in body.split(';') if t.strip()]:
            mm = re.match(r'(global|local)\s*:\s*(.*)', tok)
            if mm: scope = mm.group(1); tok = mm.group(2).strip()
            if tok: pats.append((scope, tok))
        nodes.append((name, pats))
    return nodes
def decide(nodes, sym):
    wild = lambda p: any(c in p for c in '*?[')
    exact = [(i, sc) for i, (n, ps) in enumerate(nodes) for sc, p in ps if not wild(p) and p == sym]
    if exact:
        i, sc = exact[0]
        return 'LOCAL' if sc == 'local' else nodes[i][0]
    hits = [(i, sc) for i, (n, ps) in enumerate(nodes) for sc, p in ps if wild(p) and fnmatch.fnmatchcase(sym, p)]
    if not hits: return 'BASE'
    last = max(i for i, _ in hits)
    scs = {sc for i, sc in hits if i == last}
    return nodes[last][0] if 'global' in scs else 'LOCAL'
for line in open(sys.argv[1]):
    k, s = line.rstrip('\n').split('|', 1)
    print(k, decide(parse(s), 'foo'))
