#!/usr/bin/env python3
"""Summaries of census.py's capture.json / mortar.json.
Run as: python3 -I summary.py WORK   (writes WORK/summary/*)"""
import collections, json, os, re, sys

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
WORK = os.path.abspath(sys.argv[1])
OUT = os.path.join(WORK, 'summary')
os.makedirs(OUT, exist_ok=True)
res = json.load(open(os.path.join(WORK, 'capture.json')))
mres = json.load(open(os.path.join(WORK, 'mortar.json')))
import census  # noqa: E402  (tokens, optkey, filekey; census.py has no side effects on import except argv)

G = {'aarch64': ['/usr/lib/gcc/aarch64-linux-gnu/14'],
     'x86_64': ['/usr/lib/gcc-cross/x86_64-linux-gnu/14'],
     'i686': ['/usr/lib/gcc-cross/i686-linux-gnu/14']}
S = {'aarch64': ['/usr/lib/aarch64-linux-gnu', '/lib/aarch64-linux-gnu'],
     'x86_64': ['/usr/x86_64-linux-gnu/lib64', '/usr/x86_64-linux-gnu/lib'],
     'i686': ['/usr/i686-linux-gnu/lib']}


def clean(t):
    for d in ('/obj/', '/out/', '/mortar/'):
        t = t.replace(WORK + d, '')
    return re.sub(r'(gcc|clang\d\d)_(aarch64|x86_64|i686)_(c\+\+|c|fortran)\.', 'DRV.', t)


def abbrev(a, arch):
    pre = ''
    if a.startswith('-L'):
        pre, a = '-L', a[2:]
    if a.startswith('/'):
        a = os.path.normpath(a)
        for g in G[arch]:
            if a == g or a.startswith(g + '/'):
                return pre + '$G' + a[len(g):]
        for s in S[arch]:
            if a == s or a.startswith(s + '/'):
                return pre + '$S' + a[len(s):] + ('' if s == S[arch][-1] or arch != 'x86_64' else '(lib64)')
        if '/work/' in a and '/obj/' in a:
            return pre + census.filekey(a)
        if '/out/' in a:
            return pre + 'OUT'
    return pre + a


def render(r):
    args = [abbrev(a, r['arch']) for a in r['args']]
    out, i, ls = [], 0, []
    while i < len(args):
        a = args[i]
        if a == '-plugin':
            out.append('-plugin …'); i += 2; continue
        if a.startswith('-plugin-opt='):
            if not out or out[-1] != '-plugin-opt=…':
                out.append('-plugin-opt=…')
            i += 1; continue
        if a.startswith('-L'):
            ls.append(a[2:])
            if not out or out[-1] != '-L…':
                out.append('-L…')
            i += 1; continue
        out.append(a); i += 1
    return ' '.join(out), ls


lines_tsv = ['id\tdriver\tarch\tlang\tvar\tlink_rc\trun\tstartfiles\tltail\toptions\tdiag']
opt_lines = collections.defaultdict(list)
for lid, r in sorted(res.items()):
    if 'args' not in r:
        lines_tsv.append('\t'.join([lid, r['driver'], r['arch'], r['lang'], r['var'], str(r['link_rc']),
                                    '-', '-', '-', '-', 'NOT-WRAPPED: ' + r['link_err'].splitlines()[-1][:200]]))
        continue
    toks = census.tokens(r['args'])
    starts = [census.filekey(t[1][0]) for t in toks if t[0] == 'file' and t[1][0].endswith('.o')
              and census.filekey(t[1][0]) not in ('OBJ', 'UNDEF')]
    ltail = [census.optkey(t[1]) for t in toks if t[0] == 'opt' and (t[1][0].startswith('-l') or
             t[1][0] in ('--as-needed', '--no-as-needed', '--push-state', '--pop-state', '-Bstatic',
                         '-Bdynamic', '--start-group', '--end-group'))]
    keys = []
    for t in toks:
        if t[0] == 'opt':
            k = census.optkey(t[1])
            if k not in keys:
                keys.append(k)
    for k in keys:
        opt_lines[k].append(lid)
    run = r.get('run')
    runs = '-' if not run else (run['out'] or ('rc=%s %s' % (run['rc'], run['err'][:120])))
    diag = ' / '.join(l for l in r['link_err'].splitlines() if l.strip())[:300] if r['link_err'] else ''
    lines_tsv.append('\t'.join([lid, r['driver'], r['arch'], r['lang'], r['var'], str(r['link_rc']),
                                runs.replace('\t', ' '), ' '.join(starts), ' '.join(ltail), ' '.join(keys),
                                diag.replace('\t', ' ')]))
open(os.path.join(OUT, 'lines.tsv'), 'w').write('\n'.join(lines_tsv) + '\n')

with open(os.path.join(OUT, 'options.tsv'), 'w') as f:
    f.write('option\tlines\tdefault_lines\tvariations\tline_ids\n')
    for k in sorted(opt_lines, key=lambda k: (-len(opt_lines[k]), k)):
        ids = opt_lines[k]
        vs = sorted(set(res[i]['var'] for i in ids))
        nd = sum(1 for i in ids if res[i]['var'] == 'default')
        f.write(f'{k}\t{len(ids)}\t{nd}\t{",".join(vs)}\t{",".join(ids)}\n')
nwr = sum(1 for r in res.values() if 'args' in r)
with open(os.path.join(OUT, 'union.txt'), 'w') as f:
    f.write(f'# union of linker options on the {nwr} captured -shared lines (key: count)\n')
    for k in sorted(opt_lines):
        f.write(f'{k}\t{len(opt_lines[k])}\n')

# default lines, rendered
with open(os.path.join(OUT, 'default-lines.txt'), 'w') as f:
    for lid, r in res.items():
        if r['var'] != 'default' or 'args' not in r:
            continue
        txt, ls = render(r)
        f.write(f'== {r["driver"]} ({r["ld_name"]})\n{txt}\n   -L: {" ".join(ls)}\n')

# variation diffs against the default of the same driver
def keyseq(r):
    out = []
    for t in census.tokens(r['args']):
        if t[0] == 'opt':
            k = census.optkey(t[1])
            if k in ('-o FILE', '-plugin PLUGIN', '-plugin-opt=...'):
                continue
            if k == '-L DIR':
                k = '-L' + abbrev(t[1][0][2:] if len(t[1]) == 1 else t[1][1], r['arch'])
            out.append(k)
        else:
            out.append(abbrev(t[1][0], r['arch']))
    return out


def elfdiff(d, e, r):
    out = []
    for k in ('e_type', 'interp', 'soname', 'flags', 'flags_1'):
        if d[k] != e[k]:
            out.append(f'{k}:{d[k] or "-"}->{e[k] or "-"}')
    if d['runpath'] != e['runpath']:
        out.append('runpath:' + ','.join(f'{a}={b}' for a, b in e['runpath']))
    ta, tb = collections.Counter(d['tags']), collections.Counter(e['tags'])
    if ta != tb:
        out.append('tags:' + ' '.join(['+' + t for t in (tb - ta).elements()] + ['-' + t for t in (ta - tb).elements()]))
    if d['needed'] != e['needed']:
        out.append(f'needed:{e["needed"]}')
    vd = [n for fl, n in e['verdefs'] if 'BASE' not in fl]
    if vd:
        out.append('verdefs:' + ','.join(vd))
    sa, sb = set(d['dyn_defs']), set(e['dyn_defs'])
    if sa != sb:
        add, rem = sorted(sb - sa), sorted(sa - sb)
        out.append(f'defs:+{len(add)}{add[:6]} -{len(rem)}{rem[:8]}')
    ua, ub = set(d['dyn_undefs']), set(e['dyn_undefs'])
    if ua != ub:
        out.append(f'undefs:+{len(ub - ua)}{sorted(ub - ua)[:5]} -{len(ua - ub)}{sorted(ua - ub)[:5]}')
    if d['phdrs'] != e['phdrs']:
        out.append('phdrs:' + ' '.join(['+' + x for x in sorted(set(e['phdrs']) - set(d['phdrs']))] + ['-' + x for x in sorted(set(d['phdrs']) - set(e['phdrs']))]))
    return ' '.join(out) or '(same as default)'


with open(os.path.join(OUT, 'variations.txt'), 'w') as f:
    byvar = collections.defaultdict(list)
    for lid, r in res.items():
        byvar[r['var']].append(r)
    for vid, *_ in census.VARIATIONS:
        rs = byvar.get(vid, [])
        groups = collections.defaultdict(list)
        for r in rs:
            d = res.get(r['driver'].replace('/', '_') + '.default')
            if 'args' not in r or not d or 'args' not in d:
                groups[('NOT-WRAPPED', r['link_err'].splitlines()[-1][:160] if r['link_err'] else '')].append(r['driver'])
                continue
            a, b = collections.Counter(keyseq(d)), collections.Counter(keyseq(r))
            add = tuple(sorted((b - a).elements()))
            rem = tuple(sorted((a - b).elements()))
            groups[('+' + ' '.join(add), '-' + ' '.join(rem))].append(r['driver'])
        nl = sum(1 for r in rs if r['link_rc'] == 0)
        nr = sum(1 for r in rs if r.get('run', {}).get('rc') == 0)
        f.write(f'### {vid}: {len(rs)} drivers, linked {nl}, ran OK {nr}\n')
        for g, ds in groups.items():
            f.write(f'  {g[0]}   {g[1]}\n      <- {" ".join(ds)}\n')
        fails = collections.defaultdict(list)
        for r in rs:
            if r['link_rc'] != 0:
                diag = [l for l in r['link_err'].splitlines() if re.search(r'error|undefined|can not|cannot|recompile|failed|not supported|unrecognized|unknown', l)]
                d = ' // '.join(diag[:3]) if diag else (r['link_err'].splitlines() or [''])[-1]
                fails[clean(d)[:500]].append(r['driver'])
            elif r.get('run', {}).get('rc') != 0:
                fails['RUN: ' + clean(r['run']['out'] + ' ' + r['run']['err'])[:260]].append(r['driver'])
        for d, ds in fails.items():
            f.write(f'  FAIL {d}\n      <- {" ".join(ds)}\n')
        warns = collections.defaultdict(list)
        for r in rs:
            if r['link_rc'] == 0 and r['link_err']:
                for l in r['link_err'].splitlines():
                    if 'warning' in l:
                        warns[clean(l)[:260]].append(r['driver'])
        for d, ds in warns.items():
            f.write(f'  WARN(linked) {d}\n      <- {" ".join(ds)}\n')
        facts = collections.defaultdict(list)
        for r in rs:
            e = r.get('elf')
            d = res.get(r['driver'].replace('/', '_') + '.default', {}).get('elf')
            if not e or not d:
                continue
            facts[elfdiff(d, e, r)].append(r['driver'])
        for k, ds in facts.items():
            f.write(f'  ELF-vs-default {k}\n      <- {" ".join(ds)}\n')
        runs = collections.Counter((r.get('run') or {}).get('out', '-') for r in rs)
        f.write('  RUN ' + '; '.join(f'{k} x{n}' for k, n in runs.items()) + '\n\n')

# mortar
with open(os.path.join(OUT, 'mortar.txt'), 'w') as f:
    first = collections.defaultdict(list)
    for lid, m in sorted(mres.items()):
        r = res[lid]
        ls = [l for l in m['stderr'].splitlines() if l.strip()]
        key = (m['rc'], ' | '.join(clean(l) for l in ls[:6]))
        first[key].append(lid)
    for (rc, msg), ids in sorted(first.items(), key=lambda kv: -len(kv[1])):
        f.write(f'rc={rc} x{len(ids)}: {msg}\n   <- {" ".join(ids[:40])}{" …" if len(ids) > 40 else ""}\n')
    refused = collections.Counter()
    for lid, m in mres.items():
        for o in set(re.findall(r'unsupported option: (\S+)', m['stderr'])):
            refused[o] += 1
    f.write('\nrefused options (lines carrying each):\n')
    for o, n in refused.most_common():
        f.write(f'  {o}\t{n}\n')
print('written', OUT)
