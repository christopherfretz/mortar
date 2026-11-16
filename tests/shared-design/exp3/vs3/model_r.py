#!/usr/bin/env python3
"""resolve.md 9.7 (as restated by WP-110's second pass) and D-114, modelled by
the second reviewer from the TEXT ONLY (vmatch2/model.py not read).
Copied by WP-110's third pass (exp3/vs3/model_r.py) and corrected to the
re-review's fixes M-3 (rule 12: own-node global: wins, --export-dynamic keeps,
foo@@V as foo@V), M-4 (duplicate = same language, both exact with the same
name or both globs with the same text) and M-5 (fnmatch syntax, escapes);
the glob is translated to a Python regex (model_a.py matches recursively).

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
        meta = (not quoted) and re.search(r'(?<!\\)(?:\\\\)*[*?\[]', text) is not None
        self.exact = not meta
        self.name = text if quoted else re.sub(r'\\(.)', r'\1', text)
        self.star = (not quoted) and text == '*'

    def key(self):
        return (self.lang, True, self.name) if self.exact else (self.lang, False, self.text)


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
        return s == e.name
    return re.fullmatch(glob_re(e.text), s, re.S) is not None


@functools.lru_cache(None)
def glob_re(p):
    out, i = [], 0
    while i < len(p):
        c = p[i]
        if c == '*': out.append('.*'); i += 1
        elif c == '?': out.append('.'); i += 1
        elif c == '\\':
            if i + 1 == len(p): return '(?!)'          # final lone backslash: no match
            out.append(re.escape(p[i + 1])); i += 2
        elif c == '[':
            j = i + 1
            neg = j < len(p) and p[j] in '!^'
            j += neg
            mem, first = [], True
            while j < len(p) and (first or p[j] != ']'):
                first = False
                if p[j] == '\\' and j + 1 < len(p): mem.append(p[j + 1]); j += 2
                else: mem.append(p[j]); j += 1
            if j >= len(p):
                out.append(re.escape('[')); i += 1; continue
            # members: ranges a-b where '-' is not first/last
            cls, k = [], 0
            while k < len(mem):
                if k + 2 < len(mem) and mem[k + 1] == '-':
                    a, b = mem[k], mem[k + 2]
                    cls.append(re.escape(a) + '-' + re.escape(b) if a <= b else '')
                    k += 3
                else:
                    cls.append(re.escape(mem[k])); k += 1
                    
            body = ''.join(cls)
            out.append(('[^' + body + ']') if neg else (('[' + body + ']') if body else '(?!)'))
            i = j + 1
        else: out.append(re.escape(c)); i += 1
    return ''.join(out)


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


def decide(text, names, versioned=None, mode='mortar', export_dynamic=False):
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
            _, ents, _ = node[0]
            g = any(e.scope == 'global' and matches(e, nm) for e in ents)
            lo = any(e.scope == 'local' and matches(e, nm) for e in ents)
            out[nm] = 'local' if lo and not g and not export_dynamic else v
        else:
            out[nm] = decide_one(nodes, nm)
    return out
