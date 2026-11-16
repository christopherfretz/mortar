#!/usr/bin/env python3
"""model.py SCRIPTS: an independent model of the restated resolve.md 9.7
(WP-110 second pass), written from the rule text, not from a linker:

  Matching: a C entry is compared with the symbol's name; an extern "C++"
  entry with its demangled name (the name itself when it does not demangle).
  A quoted entry, or one with none of * ? [, is EXACT; otherwise it is a
  glob; the glob consisting of '*' alone is STAR.
  Errors (refused before matching): the same entry text (same language,
  same quoting) under global: of one node and local: of ANOTHER node
  ('duplicate-version-pattern').
  Decision for a name, scanning nodes in script order, in each node its
  global: entries before its local: entries:
    an EXACT global match in node N -> version N, stop;
    an EXACT local match           -> local, stop;
    a glob (non-STAR) global match -> G := N   (a later node replaces it)
    a STAR global match            -> G* := N
    a glob local match             -> L := N
    a STAR local match             -> L* := N
  after the scan: G if set; else local if L set; else G* if set; else local
  if L* set; else BASE (unmatched: exported unversioned).
  Two EXACT global matches in two nodes: the first (warning).

Prints, per script and symbol, the model's answer; 'mortar' accepts local:
before global: (the WP-110 ruling), the 'gnu' column predicts GNU ld (which
refuses it as a syntax error).
"""
import re, sys, fnmatch, subprocess

SYMS = ['foo', '_Z3fooi', '_ZN2ns3fooEv']

def demangle(names):
    out = subprocess.run(['c++filt'], input='\n'.join(names) + '\n', capture_output=True, text=True).stdout.split('\n')
    return dict(zip(names, out))

DM = demangle(SYMS)

def tokens(s):
    out = []
    for t in re.findall(r'"[^"]*"|[{};]|[^\s{};"]+', s):
        if t in ('global:', 'local:'): out += [t[:-1], ':']   # '::' inside a C++ pattern is not a separator
        else: out.append(t)
    return out

def parse(s):
    t = tokens(s); i = 0; nodes = []; local_first = False
    while i < len(t):
        name = '<anon>'
        if t[i] != '{': name = t[i]; i += 1
        assert t[i] == '{'; i += 1
        scope = 'global'; ents = []; seen_local = False
        while t[i] != '}':
            if t[i] in ('global', 'local') and t[i + 1] == ':':
                if t[i] == 'global' and seen_local: local_first = True
                if t[i] == 'local': seen_local = True
                scope = t[i]; i += 2; continue
            if t[i] == 'extern':
                lang = t[i + 1].strip('"'); i += 3
                while t[i] != '}':
                    if t[i] != ';': ents.append((scope, lang, t[i]))
                    i += 1
                i += 1
                if t[i] == ';': i += 1
                continue
            if t[i] != ';': ents.append((scope, 'C', t[i]))
            i += 1
        i += 1
        while t[i] != ';': i += 1   # parents
        i += 1
        nodes.append((name, ents))
    return nodes, local_first

def kind(text):
    if text.startswith('"'): return 'EXACT', text[1:-1]
    if any(c in text for c in '*?['): return ('STAR' if text == '*' else 'GLOB'), text
    return 'EXACT', text

def matches(lang, text, sym):
    subject = sym if lang == 'C' else DM.get(sym, sym)
    k, pat = kind(text)
    return subject == pat if k == 'EXACT' else fnmatch.fnmatchcase(subject, pat)

def duplicate(nodes):
    for a, (na, ea) in enumerate(nodes):
        for b, (nb, eb) in enumerate(nodes):
            if a == b: continue
            for sc, lang, text in ea:
                if sc == 'global' and ('local', lang, text) in eb: return True
    return False

def decide(nodes, sym):
    G = Gs = L = Ls = None
    for name, ents in nodes:
        for want in ('global', 'local'):
            for sc, lang, text in ents:
                if sc != want or not matches(lang, text, sym): continue
                k, _ = kind(text)
                if k == 'EXACT': return name if want == 'global' else 'LOCAL'
                if want == 'global':
                    if k == 'STAR': Gs = name
                    else: G = name
                else:
                    if k == 'STAR': Ls = name
                    else: L = name
    if G: return G
    if L: return 'LOCAL'
    if Gs: return Gs
    if Ls: return 'LOCAL'
    return 'BASE'

for line in open(sys.argv[1]):
    k, s = line.rstrip('\n').split('|', 1)
    nodes, lf = parse(s)
    for sym in SYMS:
        if duplicate(nodes): m = g = 'ERR'
        else:
            m = decide(nodes, sym); m = 'BASE' if m == '<anon>' else m
            g = 'ERR' if lf else m
        print(k, sym, m, g)
