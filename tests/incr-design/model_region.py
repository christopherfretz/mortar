# model_region.py DIR OUT EDIT...: WP-207's region rule (spec/incr.md
# IN-72 item 2 as amended, phases keep / first-fit / overflow region)
# written from the rule text, run over a first link DIR/OUT (+ .map) for
# the changed file strutil.o, whose successive versions are
# DIR/strutil_EDIT.o. Adapted from the R-L reviewer's model_in72.py
# (.build/review-in72/), which modelled WP-202's "laid out again from the
# region's start". Prints per hop the definitions of the changed file whose
# ADDRESS moves (IN-27's global row: their referrers are refilled), beside
# the count the old rule gives on the same hop, and what each phase did.
#
# What it does not model: other files' regions (only strutil.o changes),
# merged pieces (item 6: they take no region bytes, as the rule says),
# packed sections, the tail's other users, growth steps (none is reached by
# these edits: the tail is reported if used).
import sys, subprocess, re, collections
D, OUT, edits = sys.argv[1], sys.argv[2], sys.argv[3:]
CHANGED = 'strutil.o'
def sh(*a): return subprocess.run(a, capture_output=True, text=True).stdout
def secs(path):
    r = []
    for l in sh('readelf', '-SW', path).splitlines():
        m = re.match(r'\s*\[\s*(\d+)\]\s+(\S+)\s+(\S+)\s+([0-9a-f]+)\s+([0-9a-f]+)\s+([0-9a-f]+)\s+([0-9a-f]+)\s+(\S*)\s+(\d+)\s+(\d+)\s+(\d+)', l)
        if m: r.append(dict(idx=int(m[1]), name=m[2], type=m[3], addr=int(m[4], 16), size=int(m[6], 16), flags=m[8], align=max(1, int(m[11]))))
    return r
def groups(path):
    g = {}; cur = None
    for l in sh('readelf', '-gW', path).splitlines():
        m = re.match(r"COMDAT group section \[\s*\d+\] `\.group' \[(\S+)\]", l)
        if m: cur = m[1]; continue
        m = re.match(r'\s+\[\s*(\d+)\]', l)
        if m and cur: g[int(m[1])] = cur
    return g
def syms(path):
    r = []
    for l in sh('readelf', '-sW', path).splitlines():
        p = l.split()
        if len(p) >= 8 and p[0].endswith(':') and p[6].isdigit():
            r.append(dict(value=int(p[1], 16), type=p[3], bind=p[4], shndx=int(p[6]), name=p[7]))
    return r
TOL = ('.text', '.rodata', '.data', '.data.rel.ro', '.bss', '.gcc_except_table')
CATS = ('.text.unlikely', '.text.exit', '.text.startup', '.text.hot')
def cat(name):
    for c in CATS:
        if name == c or name.startswith(c + '.'): return c
    return ''
def out_name(n):
    for o in ('.text', '.rodata', '.data.rel.ro', '.data', '.bss', '.gcc_except_table', '.init_array', '.fini_array', '.tdata', '.tbss', '.eh_frame'):
        if n == o or n.startswith(o + '.'): return o
    return n
def rup(x, a): return -(-x // a) * a

def identity_sections(obj):
    """model.md 7.1: (anchor, name, ordinal among same anchor and name)."""
    ss = {s['idx']: s for s in secs(obj)}
    gr = groups(obj)
    sy = syms(obj)
    out = {}
    count = collections.Counter()
    for i in sorted(ss):
        s = ss[i]
        if i in gr: anchor = ('comdat', gr[i])
        else:
            at0 = [y for y in sy if y['shndx'] == i and y['value'] == 0 and y['type'] in ('FUNC', 'OBJECT', 'NOTYPE') and y['name'] and not y['name'].startswith('$')]
            g = [y['name'] for y in at0 if y['bind'] in ('GLOBAL', 'WEAK')]
            l = [y['name'] for y in at0 if y['bind'] == 'LOCAL']
            anchor = ('global', g[0]) if g else ('local', l[0]) if l else ('none', '')
        k = (anchor, s['name']); out[i] = (anchor, s['name'], count[k]); count[k] += 1
    return ss, gr, sy, out

# the first link: the changed file's places, its regions per (output
# section, category), each region's capacity = up to the next other file's
# place or the section's end (IN-33's capacity, as R-L measured it).
osecs = [s for s in secs(f'{D}/{OUT}') if 'A' in s['flags']]
def osec_of(addr):
    for s in osecs:
        if s['addr'] <= addr < s['addr'] + max(s['size'], 1): return s['name']
    return None
osec_end = {s['name']: s['addr'] + s['size'] for s in osecs}
osize = {s['name']: s['size'] for s in osecs}
mp = open(f'{D}/{OUT}.map').read().splitlines()
fname = {int(l.split()[1]): l.split()[2] for l in mp if l.startswith('file ')}
fid = [k for k, v in fname.items() if v == CHANGED][0]
places = [(int(p[1]), int(p[2]), int(p[3], 16), int(p[4], 16)) for p in (l.split() for l in mp if l.startswith('place '))]
earlier_groups = set()
for f, n in fname.items():
    if f < fid and n.endswith('.o') and '/' not in n:
        earlier_groups |= set(groups(f'{D}/{n}').values())

ss0, gr0, sy0, id0 = identity_sections(f'{D}/strutil_base.o')
regions = collections.OrderedDict()   # (osec, cat) -> list of [start, cap]
placed = {}                            # identity -> (addr, size)
for f, si, a, sz in places:
    if f != fid or si not in ss0: continue
    o = osec_of(a)
    if o not in TOL: continue
    k = (o, cat(ss0[si]['name']))
    r = regions.setdefault(k, [[a, a + sz]])[0]
    r[0] = min(r[0], a); r[1] = max(r[1], a + sz)
    placed[id0[si]] = (a, sz, k)
for k, rl in regions.items():
    st, en = rl[0]
    nxt = min([a for f, si, a, sz in places if f != fid and osec_of(a) == k[0] and a >= en] + [osec_end[k[0]]])
    rl[0] = [st, nxt - st]
tail_used = collections.Counter()

def gaddr(obj, ss, sy, idmap, where):
    r = {}
    for y in sy:
        if y['bind'] in ('GLOBAL', 'WEAK') and y['type'] in ('FUNC', 'OBJECT') and y['shndx'] in ss:
            i = idmap.get(y['shndx'])
            if i in where: r[y['name']] = where[i][0] + y['value']
    return r

def old_rule(ss, gr, idmap):
    """WP-202's text: laid out again from the region's start (the reviewer's reading)."""
    where = {}
    by = collections.defaultdict(list)
    for i in sorted(ss):
        s = ss[i]
        if 'A' not in s['flags'] or s['type'] not in ('PROGBITS', 'NOBITS') or s['size'] == 0 or 'M' in s['flags']: continue
        if i in gr and gr[i] in earlier_groups: continue
        o = out_name(s['name'])
        if o in TOL: by[(o, cat(s['name']))].append(i)
    for k, lst in by.items():
        if k not in regions: continue
        a = regions[k][0][0]
        for i in lst:
            a = rup(a, ss[i]['align']); where[idmap[i]] = (a, ss[i]['size'], k); a += ss[i]['size']
    return where

prev_new = gaddr(f'{D}/strutil_base.o', ss0, sy0, id0, placed)
prev_old = dict(prev_new)
print(f'{D}: regions of {CHANGED}: ' + ', '.join(f"{k[0]}{'/' + k[1] if k[1] else ''} {rl[0][0]:#x}+{rl[0][1]:#x}" for k, rl in regions.items()))
for hop, e in enumerate(edits, 1):
    obj = f'{D}/strutil_{e}.o'
    ss, gr, sy, idm = identity_sections(obj)
    new = {}
    by = collections.defaultdict(list)
    for i in sorted(ss):
        s = ss[i]
        if 'A' not in s['flags'] or s['type'] not in ('PROGBITS', 'NOBITS') or s['size'] == 0: continue
        if 'M' in s['flags']: continue                       # item 6 only
        if i in gr and gr[i] in earlier_groups: continue      # IN-71's election first: not kept here
        o = out_name(s['name'])
        if o not in TOL: continue                             # packed / other: items 4, 5
        by[(o, cat(s['name']))].append(i)
    stats = collections.Counter()
    for k, lst in by.items():
        rl = regions.get(k, [])
        occupied = []  # (start, end) of sections already placed this hop in k's regions
        # phase 1: keep
        rest = []
        for i in lst:
            s = ss[i]; ident = idm[i]; old = placed.get(ident)
            if old and old[2] == k and s['size'] <= old[1] and old[0] % s['align'] == 0:
                new[ident] = (old[0], s['size'], k); occupied.append((old[0], old[0] + s['size'])); stats['kept'] += 1
            else:
                rest.append(i)
        # phase 2: first fit in the file's regions of (O, c), in their order
        over = []
        for i in rest:
            s = ss[i]; got = None
            old = placed.get(idm[i])
            if old and old[2] == k and old[0] % s['align'] == 0:
                # (i) grow in place: the bytes after it are free, inside its region
                reg = [r for r in rl if r[0] <= old[0] < r[0] + r[1]]
                if reg and old[0] + s['size'] <= reg[0][0] + reg[0][1] and not [e for e in occupied if e[0] < old[0] + s['size'] and old[0] < e[1]]:
                    got = old[0]; stats['grown-in-place'] += 1
            for st, cap in ([] if got is not None else rl):
                a = rup(st, s['align'])
                while a + s['size'] <= st + cap:
                    clash = [e for e in occupied if e[0] < a + s['size'] and a < e[1]]
                    if not clash: got = a; break
                    a = rup(max(e[1] for e in clash), s['align'])
                if got is not None: break
            if got is None: over.append(i); continue
            new[idm[i]] = (got, s['size'], k); occupied.append((got, got + s['size'])); stats['first-fit'] += 1 if got != (old or [None])[0] else 0
        # phase 3: one overflow region at the output section's tail
        if over:
            need = 0
            for i in over: need = rup(need, ss[i]['align']) + ss[i]['size']
            st = rup(osec_end[k[0]] + tail_used[k[0]], max(ss[i]['align'] for i in over))
            cap = rup(need + max(64, need // 8), max(ss[i]['align'] for i in over))
            a = st
            for i in over:
                a = rup(a, ss[i]['align']); new[idm[i]] = (a, ss[i]['size'], k); a += ss[i]['size']; stats['overflow'] += 1
            tail_used[k[0]] += st - (osec_end[k[0]] + tail_used[k[0]]) + cap
            regions.setdefault(k, []).append([st, cap])
            stats['overflow-regions'] += 1
    cur_new = gaddr(obj, ss, sy, idm, new)
    cur_old = gaddr(obj, ss, sy, idm, old_rule(ss, gr, idm))
    mv_new = sum(1 for n in cur_new if n in prev_new and cur_new[n] != prev_new[n])
    mv_old = sum(1 for n in cur_old if n in prev_old and cur_old[n] != prev_old[n])
    print(f'hop {hop} ({e}): definitions whose address moves: WP-207 rule {mv_new} of {len(cur_new)}; '
          f'WP-202 rule {mv_old} of {len(cur_old)}; sections kept {stats["kept"]}, first-fit {stats["first-fit"]}, '
          f'grown in place {stats["grown-in-place"]}, to an overflow region {stats["overflow"]} ({stats["overflow-regions"]} regions); tail used '
          + ', '.join(f'{o} {v:#x} of {max(4096, osize[o] // 16):#x}' + (' (GROWTH STEP)' if v > max(4096, osize[o] // 16) else '') for o, v in tail_used.items()))
    placed = new; prev_new = cur_new; prev_old = cur_old
