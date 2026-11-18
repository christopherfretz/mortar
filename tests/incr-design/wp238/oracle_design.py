#!/usr/bin/env python3
"""oracle_design.py ROWS | OLD.rows NEW.rows -- WP-238 step 2, Part 0 (F10).

The design-faithful oracle: a walk that takes ONLY what the state will
store after the review (spec/landed/incr/WP-238.md "Step 2"), over a probe
dump (crates/mortar_live/src/gc/probe.rs, `stored_dump`):

* the stored rows (`W` lines): kinds 0-8 normalised (F8), the group as a
  ring, ROOT, NAMED (target 0), SPLIT, REFUSE;
* RESOLVED as the capture writes it (`Q` lines: def_kind, section, value);
* SECTIONS' kept bits and names (`S` lines: kept and name only);
* the rooted keys of OptionKeys (entry, -u, --require-defined, -init,
  -fini; --defsym naming a symbol is v1's fall-back), parsed from the
  link's own arguments (`A`, `C` lines), NOT the full mark's `R` lines.

It never reads the full link's `K` targets, rooted flags, `E` rows or
`R` lines except to report: the comparison is with the full mark's live
bits (`S`) and live fragments (`L`).

One dump: the stored-form walk against that link's full mark.
Two dumps (OLD NEW): the update as Part 3 does it: the old walk (OLD's
stored rows) and its live-set check, then the new walk from OLD's rows
for unchanged files and NEW's rows for changed and drawn-in files (R-4:
a file whose kept bits differ is drawn in), RESOLVED, SECTIONS and the
options from NEW; compared with NEW's full mark. No skip rule (F5).

Last line: `AGREE ...`, `REFUSED <reasons> ...` or `DISAGREE ...`.
"""
import collections, importlib.util, os, re, shlex, sys

spec = importlib.util.spec_from_file_location(
    'emulate', os.path.join(os.path.dirname(os.path.abspath(__file__)), 'emulate.py'))
E = importlib.util.module_from_spec(spec)
spec.loader.exec_module(E)

KEY_KINDS = {0, 1, 4, 5}
SEC_KINDS = {2, 3, 6, 7, 8}
ROOT, NAMED, SPLIT, REFUSE = 9, 10, 11, 12
REFUSE_NAME = {1: 'fde-global', 2: 'def-dropped', 3: 'unreadable'}
D_UNDEF, D_SECTION, D_ABS, D_COMMON, D_INTERNAL, D_SHARED = 0, 1, 2, 3, 4, 5
WHOLE = -1
# Mutations, to show the oracle can fail (WP238_MUT): `noinit` (F1's
# defect: -init/-fini not rooted), `kindreader` (F2's defect: only kind-1
# rows expand __start_/__stop_), `drop<K>` (rows of kind K ignored).
MUT = set(os.environ.get('WP238_MUT', '').split(','))


def c_ident(n):
    return n != '' and not n[0].isdigit() and all(c == '_' or c.isalnum() and c.isascii() for c in n)


class Stored:
    """What the state stores, read from a dump."""

    def __init__(self, path):
        self.D = E.Dump(path)
        self.args, self.cwd = [], None
        self.resolved = {}            # key name -> (kind, secident|None, value)
        self.rows = collections.defaultdict(set)   # fileident -> {(src, kind, tk, target, off)}
        self.kind_count = collections.Counter()
        for line in open(path, encoding='utf-8', errors='replace'):
            t = line.rstrip('\n').split('\t')
            if t[0] == 'A':
                self.args.append(t[1] if len(t) > 1 else '')
            elif t[0] == 'C':
                self.cwd = t[1]
            elif t[0] == 'Q':
                name = self.D.keys.get(int(t[1]))
                sec = None if t[3] == '-' else self.D.sec.get(int(t[3]))
                self.resolved[name] = (int(t[2]), sec, int(t[4]))
            elif t[0] == 'W':
                src = self.D.sec[int(t[1])]
                kind, tk, tgt, off = int(t[2]), t[3], int(t[4]), int(t[5])
                if tk == 'K':
                    tgt = self.D.keys[tgt]
                elif tk == 'S':
                    tgt = self.D.sec[tgt]
                self.rows[src[0]].add((src, kind, tgt, off))
                self.kind_count[kind] += 1

    def kept(self, si):
        inf = self.D.info.get(si)
        return inf is not None and inf[1]

    def name(self, si):
        return self.D.info[si][0]

    def options(self):
        """OptionKeys' rooted keys, from the link's arguments."""
        args = expand(self.args, self.cwd)
        entry, keys, defsym, fallback = None, [], {}, []
        i = 0

        def val(opt_long, opt_short=None):
            nonlocal i
            a = args[i]
            for o in [x for x in (opt_long, opt_short) if x]:
                if a == o:
                    i += 1
                    return args[i] if i < len(args) else ''
                if a.startswith(o + '=') and o.startswith('-'):
                    return a[len(o) + 1:]
                if len(o) == 2 and a.startswith(o) and len(a) > 2:
                    return a[2:]
            return None

        while i < len(args):
            a = args[i]
            for lng, sht, what in (('--entry', '-e', 'entry'), ('--undefined', '-u', 'u'),
                                   ('--require-defined', None, 'u'), ('-init', None, 'init'),
                                   ('--init', None, 'init'), ('-fini', None, 'init'),
                                   ('--fini', None, 'init'),
                                   ('--defsym', None, 'defsym')):
                if a.startswith(('-o', '-L', '-l')) and sht is None:
                    continue
                v = val(lng, sht)
                if v is None:
                    continue
                if what == 'entry':
                    entry = v
                elif what == 'u' or what == 'init' and 'noinit' not in MUT:
                    keys.append(v)
                elif what == 'init':
                    pass
                else:
                    name, _, expr = v.partition('=')
                    defsym[name] = expr
                break
            if a in ('-shared', '-pie', '-dynamic-linker', '--dynamic-linker', '-Bshareable',
                     '--export-dynamic', '-E'):
                fallback.append('incremental-unsupported (%s)' % a)
            if a.startswith('@'):
                fallback.append('ORACLE-ARGS-UNKNOWN (%s gone)' % a)
            if a == '--gc-keep-exported':
                fallback.append('gc-keep-exported (R-5)')
            i += 1
        # Implicit input scripts (an input that is neither ELF nor an
        # archive): ENTRY and EXTERN are OptionKeys' too.
        for a in args:
            if a.startswith('-'):
                continue
            p = os.path.join(self.cwd or '.', a)
            try:
                with open(p, 'rb') as f:
                    head = f.read(1 << 16)
            except OSError:
                continue
            if head[:4] == b'\x7fELF' or head[:8] == b'!<arch>\n' or b'\0' in head:
                continue
            text = head.decode('utf-8', 'replace')
            for m in re.finditer(r'\bENTRY\s*\(\s*([^\s)]+)\s*\)', text):
                entry = entry or m.group(1)
            for m in re.finditer(r'\bEXTERN\s*\(([^)]*)\)', text):
                keys.extend(re.split(r'[\s,]+', m.group(1).strip()))
        rooted = [entry or '_start'] + [k for k in keys if k]
        for name, expr in defsym.items():
            base = expr.strip().split('+')[0].split('-')[0].strip()
            if base and not base[0].isdigit():
                # OptionKeys::roots' defsym base (rooted, as live_roots
                # does), and v1's fall-back (F9) all the same.
                rooted.append(base)
                fallback.append('defsym %s=%s names a symbol (F9)' % (name, expr))
        return rooted, set(defsym), fallback


def expand(args, cwd):
    out = []
    for a in args:
        if a.startswith('@'):
            p = a[1:]
            if cwd and not os.path.isabs(p):
                p = os.path.join(cwd, p)
            try:
                out.extend(expand(shlex.split(open(p).read()), cwd))
                continue
            except OSError:
                pass
        out.append(a)
    return out


def walk(S, rows, rooted, defsyms, problems):
    """The stored-form walk. `S` gives RESOLVED, SECTIONS (kept, names)
    and the pieces; `rows` is fileident -> stored rows."""
    D = S.D
    by_src = collections.defaultdict(list)
    roots, named, split = set(), collections.defaultdict(list), set()
    refused = collections.Counter()
    for rs in rows.values():
        for (src, kind, tgt, off) in rs:
            if 'drop%d' % kind in MUT:
                continue
            if kind == ROOT:
                roots.add(src)
            elif kind == NAMED:
                named[S.name(src)].append(src)
            elif kind == SPLIT:
                split.add(src)
            elif kind == REFUSE:
                refused[REFUSE_NAME.get(tgt, str(tgt))] += 1
            else:
                by_src[src].append((kind, tgt, off))
    live, frags, whole, stack = set(), set(), set(), []

    def at(s, off):
        if s is None or not S.kept(s):
            problems.append('a row or key names a section not kept: %s' % (s,))
            return
        if s not in live:
            live.add(s)
            stack.append(s)
        if s not in split:
            return
        ps = D.pieces.get(s, [])
        if off == WHOLE:
            if s not in whole:
                whole.add(s)
                frags.update(f for _o, f in ps)
            return
        lo, hi = 0, len(ps)
        while lo < hi:
            mid = (lo + hi) // 2
            if ps[mid][0] <= off:
                lo = mid + 1
            else:
                hi = mid
        if lo > 0:
            frags.add(ps[lo - 1][1])

    def key(name, via=None):
        # The ONE key reader (F2): kind-blind, by RESOLVED's def_kind.
        r = S.resolved.get(name)
        if r is None:
            return
        kind, sec, value = r
        if kind == D_SECTION:
            if sec is None:
                problems.append('RESOLVED SECTION without a section: %s' % name)
                return
            at(sec, value if value < 2 ** 32 - 1 else WHOLE)
        elif kind == D_INTERNAL:
            for p in ('__start_', '__stop_'):
                if 'kindreader' in MUT and via not in (None, 1):
                    continue
                if name.startswith(p) and c_ident(name[len(p):]) and name not in defsyms:
                    for s in named.get(name[len(p):], ()):
                        at(s, WHOLE)
        # UNDEFINED, ABSOLUTE, COMMON, SHARED: nothing.

    for s in sorted(roots):
        at(s, WHOLE)
    for name in rooted:
        key(name)
    while stack:
        s = stack.pop()
        for kind, tgt, off in by_src.get(s, ()):
            if kind in KEY_KINDS:
                key(tgt, kind)
            else:
                at(tgt, off)
    return live, frags, refused


def full_mark(S):
    live = {si for si, inf in S.D.info.items() if inf[2]}
    return live, set(S.D.live_frags)


def short(S, ss):
    return sorted('%s:%d:%s' % (s[0][0].split('/')[-1] + ('(' + s[0][1] + ')' if s[0][1] != '-' else ''),
                                 s[1], S.D.info.get(s, ('?',))[0]) for s in ss)[:6]


def verdict(S, live, frags, refused, problems, fallback, prefix=''):
    want, wfrags = full_mark(S)
    miss, extra = want - live, live - want
    fmiss, fextra = wfrags - frags, frags - wfrags
    detail = 'sections live %d miss %d %s extra %d %s; frags live %d miss %d extra %d; problems %d %s' % (
        len(want), len(miss), short(S, miss), len(extra), short(S, extra),
        len(wfrags), len(fmiss), len(fextra), len(problems), problems[:3])
    if refused or fallback:
        why = ['%s x%d' % kv for kv in sorted(refused.items())] + fallback
        return 'REFUSED %s %s; walk %s' % (prefix, why, detail)
    ok = not (miss or extra or fmiss or fextra or problems)
    return '%s %s%s' % ('AGREE' if ok else 'DISAGREE', prefix, detail)


def single(path):
    S = Stored(path)
    rooted, defsyms, fallback = S.options()
    problems = []
    live, frags, refused = walk(S, S.rows, rooted, defsyms, problems)
    # The full link's R lines (resolution's roots), reported only.
    full_roots = set()
    for line in open(path, encoding='utf-8', errors='replace'):
        t = line.rstrip('\n').split('\t')
        if t[0] == 'R':
            full_roots.add(S.D.keys[int(t[1])])
    have = {n for n in rooted if n in S.resolved}
    print('rows %s; rooted %s; full-link roots not rooted here %s' % (
        dict(sorted(S.kind_count.items())), sorted(have), sorted(full_roots - have)))
    print(verdict(S, live, frags, refused, problems, fallback))


def update(opath, npath):
    O, N = Stored(opath), Stored(npath)
    changed, drawn = [], []
    for fi in N.D.hash:
        if O.D.hash.get(fi) != N.D.hash[fi] or N.D.hash[fi] == '0' * 16:
            changed.append(fi)
            continue
        okept = {si[1] for si in O.D.file_secs(fi) if O.D.info[si][1]}
        nkept = {si[1] for si in N.D.file_secs(fi) if N.D.info[si][1]}
        if okept != nkept:
            drawn.append(fi)
    orooted, odefs, ofall = O.options()
    nrooted, ndefs, nfall = N.options()
    # The old walk over OLD's stored rows: its live set is what META's
    # hash covers (F7); a difference is the named fall-back.
    op = []
    o_live, o_frags, o_ref = walk(O, O.rows, orooted, odefs, op)
    owant, ofwant = full_mark(O)
    hash_ok = o_live == owant and o_frags == ofwant
    # The new walk: unchanged files' stored rows (OLD), changed and drawn
    # files' fresh rows (NEW); RESOLVED / SECTIONS / options of NEW.
    rows = {}
    for fi in N.D.hash:
        rows[fi] = (N if fi in changed or fi in drawn else O).rows.get(fi, set())
    np_ = []
    n_live, n_frags, n_ref = walk(N, rows, nrooted, ndefs, np_)
    refused = o_ref + n_ref
    prefix = 'changed %d drawn %d old-walk-vs-META %s; ' % (
        len(changed), len(drawn), 'same' if hash_ok else 'DIFFERS')
    print(verdict(N, n_live, n_frags, refused, op + np_, ofall + nfall, prefix))


def main():
    a = sys.argv[1:]
    if len(a) == 1:
        single(a[0])
    elif len(a) == 2:
        update(a[0], a[1])
    else:
        sys.exit(__doc__)


if __name__ == '__main__':
    main()
