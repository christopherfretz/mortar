#!/usr/bin/env python3
"""resolve.md 9.7 (as restated by WP-110's second pass) and D-114, modelled by
the second reviewer from the TEXT ONLY (vmatch2/model.py not read).

decide(script_text, names, versioned) -> {name: 'V'|'local'|'base'} or 'REFUSED:<why>'
versioned: {name: node} for non-default .symver definitions foo@V (rule 12).
mode 'mortar' accepts local: before global: (rule 8); mode 'gnu' refuses it.
"""
import re, subprocess, fnmatch, functools

TOK = re.compile(r'\s+|#[^\n]*|/\*.*?\*/|"(?:[^"\\]|\\.)*"|[{}();:]|(?:[^\s{}();:"]|::)+', re.S)


def lex(t):
    out = []
    for m in TOK.finditer(t):
        s = m.group(0)
        if s.isspace() or s.startswith('#') or s.startswith('/*'):
            continue
        if s.startswith('//'):
            raise SyntaxError('//')
        out.append(s)
    return out


class Entry:
    def __init__(self, text, quoted, lang, scope):
        self.text, self.quoted, self.lang, self.scope = text, quoted, lang, scope
        self.exact = quoted or not any(c in text for c in '*?[')
        self.star = (not quoted) and text == '*'

    def key(self):
        return (self.lang, self.text, self.quoted)


def parse(text, mode):
    toks = lex(text)
    i = 0
    nodes = []  # (name or None, [Entry], parents)

    def peek():
        return toks[i] if i < len(toks) else None
    while i < len(toks):
        name = None
        if toks[i] != '{':
            name = toks[i]; i += 1
        if peek() != '{':
            raise SyntaxError('expected {')
        i += 1
        entries = []
        scope = 'global'
        seen_global = seen_local = False

        def entry_list(lang):
            nonlocal i
            while peek() != '}':
                s = toks[i]; i += 1
                q = s.startswith('"')
                if q:
                    s = s[1:-1]
                if peek() == ';':
                    i += 1
                elif peek() != '}':
                    raise SyntaxError('expected ;')
                entries.append(Entry(s, q, lang, scope))
            i += 1
        while peek() != '}':
            s = toks[i]
            if s in ('global', 'local') and i + 1 < len(toks) and toks[i + 1] == ':':
                if s == 'global' and seen_local and mode == 'gnu':
                    raise SyntaxError('local before global')
                scope = s
                seen_global |= s == 'global'; seen_local |= s == 'local'
                i += 2
                continue
            if s == 'extern':
                lang = toks[i + 1].strip('"')
                if toks[i + 2] != '{':
                    raise SyntaxError('extern {')
                i += 3
                entry_list('C++' if lang == 'C++' else 'C')
                if peek() == ';':
                    i += 1
                continue
            q = s.startswith('"')
            t = s[1:-1] if q else s
            i += 1
            if peek() == ';':
                i += 1
            elif peek() != '}':
                raise SyntaxError('expected ; after ' + s)
            entries.append(Entry(t, q, 'C', scope))
        i += 1
        parents = []
        while peek() not in (';', None):
            parents.append(toks[i]); i += 1
        if peek() != ';':
            raise SyntaxError('expected ; after node')
        i += 1
        nodes.append((name, entries, parents))
    # grammar refusals (script.md 9.1)
    names = [n for n, _, _ in nodes]
    if None in names and len(nodes) > 1:
        raise SyntaxError('anonymous with others')
    seen = set()
    for n, _, ps in nodes:
        for p in ps:
            if p not in seen:
                raise SyntaxError('forward or missing parent ' + p)
        if n in seen:
            raise SyntaxError('node twice ' + n)
        seen.add(n)
    return nodes


@functools.lru_cache(None)
def demangle(name):
    r = subprocess.run(['c++filt', name], capture_output=True, text=True).stdout.strip()
    return r


def subject(e, name):
    if e.lang == 'C++':
        d = demangle(name)
        return d  # c++filt echoes the name when it does not demangle
    return name


def matches(e, name):
    s = subject(e, name)
    if e.exact:
        return s == e.text
    return fnmatch.fnmatchcase(s, e.text.replace('[^', '[!'))


def check_dups(nodes):
    for a, (na, ea, _) in enumerate(nodes):
        for b, (nb, eb, _) in enumerate(nodes):
            if a == b:
                continue
            for x in ea:
                if x.scope != 'global':
                    continue
                for y in eb:
                    if y.scope == 'local' and x.key() == y.key():
                        return f'duplicate-version-pattern {x.text}'
    return None


def decide_one(nodes, name):
    G = L = Gs = Ls = None
    warn = None
    for n, ents, _ in nodes:
        glob_ = [e for e in ents if e.scope == 'global']
        loc = [e for e in ents if e.scope == 'local']
        if any(e.exact and matches(e, name) for e in glob_):
            return n or 'base'   # anonymous node: exported unversioned
        if any(e.exact and matches(e, name) for e in loc):
            return 'local'
        for e in glob_:
            if not e.exact and matches(e, name):
                if e.star:
                    Gs = n or 'base'
                else:
                    G = n or 'base'
        for e in loc:
            if not e.exact and matches(e, name):
                if e.star:
                    Ls = True
                else:
                    L = True
    if G:
        return G
    if L:
        return 'local'
    if Gs:
        return Gs
    if Ls:
        return 'local'
    return 'base'


def decide(text, names, versioned=None, mode='mortar'):
    try:
        nodes = parse(text, mode)
    except SyntaxError as e:
        return 'REFUSED:syntax ' + str(e)
    d = check_dups(nodes)
    if d:
        return 'REFUSED:' + d
    out = {}
    for nm in names:
        if versioned and nm in versioned:
            v = versioned[nm]
            node = [x for x in nodes if x[0] == v]
            if not node:
                return 'REFUSED:undefined-version'
            r = decide_one(node, nm)
            out[nm] = 'local' if r == 'local' else v
        else:
            out[nm] = decide_one(nodes, nm)
    return out
