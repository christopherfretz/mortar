#!/usr/bin/env python3
"""model_a.py: the WP-110 author's model of resolve.md 9.7 (exp2/vmatch2/model.py),
corrected by the third pass from the restated text (M-3, M-4, M-5):

  Entries.  A quoted entry is EXACT and its text is taken verbatim.  An
  unquoted entry with an unescaped * ? or [ is a GLOB (STAR when it is '*'
  alone); any other unquoted entry is EXACT and its name is the text with
  each backslash-pair \\c replaced by c (a final lone backslash stays).
  GLOB matching: '*' any string (also empty), '?' one character, '\\c' the
  character c, a final lone '\\' makes the pattern match nothing; '[' opens a
  class: a first '!' or '^' negates, then a first ']' is a member, then
  members up to the closing ']': '\\c' is c, 'a-b' a range (empty when a>b),
  '-' first or last is itself; a '[' with no closing ']' is the character '['.
  Duplicate (refused): an entry under global: of one node and one under
  local: of another, same language, both EXACT with the same name, or both
  GLOB with the same text.
  Decision: unchanged (exact global, exact local, then G, L, G*, L*, base).
  Rule 12: a .symver definition foo@V or foo@@V is matched only by node V:
  local exactly when one of V's local: entries matches, none of V's global:
  entries matches, and --export-dynamic is not given; else version V.  A
  node V missing from the script: refused.
decide(text, names, versioned, mode, export_dynamic) -> {name: V|'base'|'local'} or 'REFUSED'
"""
import re, subprocess, functools


@functools.lru_cache(None)
def dm(sym):
    return subprocess.run(['c++filt', sym], capture_output=True, text=True).stdout.strip() or sym


def tokens(s):
    out = []
    for t in re.findall(r'"[^"]*"|[{};]|[^\s{};"]+', s):
        if t in ('global:', 'local:'): out += [t[:-1], ':']
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
        while t[i] != ';': i += 1
        i += 1
        nodes.append((name, ents))
    return nodes, local_first


def unescaped_meta(text):
    i = 0
    while i < len(text):
        if text[i] == '\\': i += 2; continue
        if text[i] in '*?[': return True
        i += 1
    return False


def kind(text):
    if text.startswith('"'): return 'EXACT', text[1:-1]
    if unescaped_meta(text): return ('STAR' if text == '*' else 'GLOB'), text
    return 'EXACT', re.sub(r'\\(.)', r'\1', text)


def gmatch(p, s):
    """the measured GNU ld glob (fnmatch, no flags)"""
    if not p: return not s
    c = p[0]
    if c == '*':
        return any(gmatch(p[1:], s[k:]) for k in range(len(s) + 1))
    if c == '?':
        return bool(s) and gmatch(p[1:], s[1:])
    if c == '\\':
        if len(p) == 1: return False
        return bool(s) and s[0] == p[1] and gmatch(p[2:], s[1:])
    if c == '[':
        j = 1; neg = False
        if j < len(p) and p[j] in '!^': neg = True; j += 1
        items = []; first = True
        while j < len(p) and (p[j] != ']' or first):
            first = False
            a = p[j]
            if a == '\\' and j + 1 < len(p): a = p[j + 1]; j += 1
            j += 1
            if j + 1 < len(p) and p[j] == '-' and p[j + 1] != ']':
                b = p[j + 1]
                if b == '\\' and j + 2 < len(p): b = p[j + 2]; j += 1
                j += 2
                items.append((a, b))
            else:
                items.append((a, a))
        if j >= len(p):                      # no closing ']': '[' is itself
            return bool(s) and s[0] == '[' and gmatch(p[1:], s[1:])
        if not s: return False
        hit = any(lo <= s[0] <= hi for lo, hi in items)
        return hit != neg and gmatch(p[j + 1:], s[1:])
    return bool(s) and s[0] == c and gmatch(p[1:], s[1:])


def matches(lang, text, sym):
    subject = sym if lang == 'C' else dm(sym)
    k, pat = kind(text)
    return subject == pat if k == 'EXACT' else gmatch(pat, subject)


def same(lang, text, lang2, text2):
    if lang != lang2: return False
    k, a = kind(text); k2, b = kind(text2)
    if (k == 'EXACT') != (k2 == 'EXACT'): return False
    return a == b


def duplicate(nodes):
    for a, (na, ea) in enumerate(nodes):
        for b, (nb, eb) in enumerate(nodes):
            if a == b: continue
            for sc, lang, text in ea:
                if sc != 'global': continue
                for sc2, lang2, text2 in eb:
                    if sc2 == 'local' and same(lang, text, lang2, text2): return True
    return False


def decide1(nodes, sym):
    G = Gs = L = Ls = None
    for name, ents in nodes:
        for want in ('global', 'local'):
            for sc, lang, text in ents:
                if sc != want or not matches(lang, text, sym): continue
                k, _ = kind(text)
                if k == 'EXACT': return name if want == 'global' else 'local'
                if want == 'global':
                    if k == 'STAR': Gs = name
                    else: G = name
                else:
                    if k == 'STAR': Ls = name
                    else: L = name
    if G: return G
    if L: return 'local'
    if Gs: return Gs
    if Ls: return 'local'
    return 'base'


def decide(text, names, versioned=None, mode='mortar', export_dynamic=False):
    try:
        nodes, lf = parse(text)
    except (AssertionError, IndexError):
        return 'REFUSED'
    if duplicate(nodes) or (mode == 'gnu' and lf): return 'REFUSED'
    out = {}
    for n in names:
        if versioned and n in versioned:
            v = versioned[n]
            node = [x for x in nodes if x[0] == v]
            if not node: return 'REFUSED'
            ents = node[0][1]
            g = any(sc == 'global' and matches(l, t, n) for sc, l, t in ents)
            lo = any(sc == 'local' and matches(l, t, n) for sc, l, t in ents)
            out[n] = 'local' if (lo and not g and not export_dynamic) else v
        else:
            r = decide1(nodes, n)
            out[n] = 'base' if r == '<anon>' else r
    return out
