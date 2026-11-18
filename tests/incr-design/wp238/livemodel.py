#!/usr/bin/env python3
"""livemodel.py DIR [DIR ...] [--drop KIND[,KIND]] [--frag] [--stats] [--quiet]

Re-review N2: the live set of a --gc-sections link computed from the input
objects alone (the files the map lists, i.e. extraction as given: image.md
I-10 decides it before liveness), by enumerating image.md I-7's edges
from I-6's roots, one edge kind per row:

  reloc      a relocation of a live alloc non-.eh_frame section naming a
             GLOBAL -> the section of its definition (R_*_NONE included)
  startstop  a reference to an undefined __start_NAME/__stop_NAME (NAME a
             C identifier) -> every section named NAME
  local      a record of a live alloc section naming a local or section
             symbol -> its section (merged sections: section level; --frag
             also compares fragments)
  linkorder  a SHF_LINK_ORDER section whose sh_link is s lives with s (I-109)
  fde        a live code section -> the sections the relocations of its
             FDE (the LSDA) and of its CIE (the personality) name
  group      every other member of a live section's group
  nonalloc   relocations of non-alloc non-debug sections (notes, .comment...)
             -> the sections they name (global or local)

Roots (I-6): the entry's (_start) defining section; SHF_GNU_RETAIN;
start-up tables by input name (.init_array .fini_array .preinit_array .init
.fini .ctors .dtors, or one of those followed by '.'); notes and non-alloc
sections not in a group with an allocated member; .eh_frame; never a
SHF_LINK_ORDER section. COMDAT: the first file (load order) holding a
signature keeps the group; a global's definition is the first strong one
in load order, else the first weak one, outside dropped groups.

Compares the model's live set with the map's (place / nplace / piece /
merged = live, `discard F S gc` = dead) over the sections the map lists.
Prints AGREE / DISAGREE per directory; --stats prints, per edge kind, the
distinct (source section, target section) pairs whose source is live.
"""
import os, re, sys, struct, collections, time
sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
from oracle import Link, Obj

KINDS = ['reloc', 'startstop', 'local', 'linkorder', 'fde', 'group', 'nonalloc']
CIDENT = re.compile(r'^[A-Za-z_][A-Za-z0-9_]*$')
INIT = ('.init_array', '.fini_array', '.preinit_array', '.init', '.fini', '.ctors', '.dtors')
SKIP_TYPES = {0, 2, 3, 4, 9, 17, 18}   # NULL SYMTAB STRTAB RELA REL GROUP SYMTAB_SHNDX


def is_debug(n):
    return n.startswith('.debug') or n.startswith('.zdebug') or n in ('.line', '.stab', '.stabstr', '.gdb_index')


def eh_records(o, sec):
    """records of an .eh_frame input section: list of (start, end, is_cie, cie_start)"""
    s = o.S[sec]
    b = o.bytes[s['off']:s['off'] + s['size']]
    out, p = [], 0
    while p + 4 <= len(b):
        (ln,) = struct.unpack_from('<I', b, p)
        if ln == 0:
            break
        hdr = 4
        if ln == 0xffffffff:
            (ln,) = struct.unpack_from('<Q', b, p + 4); hdr = 12
        end = p + hdr + ln
        (cid,) = struct.unpack_from('<I', b, p + hdr)
        out.append((p, end, cid == 0, None if cid == 0 else p + hdr - cid))
        p = end
    return out


class Model:
    def __init__(self, d):
        self.L = Link(d)
        L = self.L
        self.order = [L.files[i] for i in sorted(L.files)]
        self.objs = {k: Obj(L.file_bytes(k)) for k in self.order}
        # COMDAT: first holder keeps
        seen, self.dropped = set(), set()
        for k in self.order:
            for sig, mem in self.objs[k].groups.items():
                if sig in seen:
                    self.dropped |= {(k, m) for m in mem}
                seen.add(sig)
        # resolution
        res = {}
        for k in self.order:
            o = self.objs[k]
            for y in o.syms:
                if y['bind'] == 0 or not y['name'] or y['shn'] == 0:
                    continue
                if 0 < y['shn'] < 0xff00 and (k, y['shn']) in self.dropped:
                    continue
                strong = y['bind'] == 1 and y['shn'] != 0xfff2
                cur = res.get(y['name'])
                if cur is None or (strong and not cur[2]):
                    res[y['name']] = (k, y['shn'], strong, y['value'])
        self.res = res
        # sections, names, group membership
        self.secs = []
        self.byname = collections.defaultdict(list)
        self.group_of = {}
        for k in self.order:
            o = self.objs[k]
            for g in o.groups.values():
                for m in g:
                    self.group_of[(k, m)] = g
            for i, s in enumerate(o.S):
                if s['type'] in SKIP_TYPES or (k, i) in self.dropped:
                    continue
                self.secs.append((k, i))
                self.byname[s['name']].append((k, i))
        self.edges = collections.defaultdict(set)    # kind -> {(src, dst)}
        self.adj = collections.defaultdict(lambda: collections.defaultdict(set))  # src -> kind -> {dst}
        self.frag_refs = collections.defaultdict(lambda: collections.defaultdict(set))  # src -> (k, sec) -> {offsets}
        self.build()

    def target(self, k, sym, add):
        """(kind-of-name, section or list of sections, offset) for a record."""
        o = self.objs[k]
        if sym >= len(o.syms) or sym == 0:
            return None
        y = o.syms[sym]
        if y['bind'] != 0 and y['name']:
            r = self.res.get(y['name'])
            if r is None or r[1] == 0:
                m = re.match(r'^__(start|stop)_(.*)$', y['name'])
                if m and CIDENT.match(m.group(2)):
                    return ('startstop', self.byname.get(m.group(2), []), None)
                return None
            if not (0 < r[1] < 0xff00):
                return None         # absolute / common: nothing
            return ('global', [(r[0], r[1])], r[3])
        if 0 < y['shn'] < 0xff00:
            # the fragment: the symbol's value, plus the addend for a section symbol (I-7, model 3.5)
            return ('local', [(k, y['shn'])], y['value'] + ((add or 0) if y['type'] == 3 else 0))
        return None

    def add(self, kind, src, dst):
        if src == dst or dst[1] >= len(self.objs[dst[0]].S) or (dst[0], dst[1]) in self.dropped:
            return
        self.edges[kind].add((src, dst))
        self.adj[src][kind].add(dst)

    def build(self):
        for k in self.order:
            o = self.objs[k]
            # relocation edges of non-debug, non-.eh_frame sections
            for sec, rs in o.rela.items():
                if sec >= len(o.S) or (k, sec) in self.dropped:
                    continue
                s = o.S[sec]
                if is_debug(s['name']) or s['name'] == '.eh_frame' or s['type'] in SKIP_TYPES:
                    continue
                alloc = bool(s['flags'] & 2)
                for (off, sym, ty, a) in rs:
                    t = self.target(k, sym, a)
                    if t is None:
                        continue
                    nk, dsts, toff = t
                    kind = 'nonalloc' if not alloc else ('startstop' if nk == 'startstop' else 'reloc' if nk == 'global' else 'local')
                    for d in dsts:
                        self.add(kind, (k, sec), d)
                        if nk in ('local', 'global'):
                            self.frag_refs[(k, sec)][d].add(toff)
            # link order
            for i, s in enumerate(o.S):
                if s['flags'] & 0x80 and 0 < s['link'] < len(o.S) and (k, i) not in self.dropped:
                    self.add('linkorder', (k, s['link']), (k, i))
            # groups
            for g in o.groups.values():
                for m in g:
                    for m2 in g:
                        if m != m2 and o.S[m2]['type'] not in SKIP_TYPES:
                            self.add('group', (k, m), (k, m2))
            # FDE rule
            for sec, s in enumerate(o.S):
                if s['name'] != '.eh_frame' or s['type'] in SKIP_TYPES:
                    continue
                recs = eh_records(o, sec)
                rel = sorted(o.rela.get(sec, ()))
                by_rec = collections.defaultdict(list)
                starts = [r[0] for r in recs]
                import bisect
                for (off, sym, ty, a) in rel:
                    j = bisect.bisect_right(starts, off) - 1
                    if j >= 0:
                        by_rec[recs[j][0]].append((off, sym, ty, a))
                cie_tg = {}
                for (p, e, is_cie, cp) in recs:
                    if is_cie:
                        cie_tg[p] = [t for r in by_rec[p] for t in (self.target(k, r[1], r[3]) or (None, [], None))[1]]
                for (p, e, is_cie, cp) in recs:
                    if is_cie:
                        continue
                    rs = by_rec[p]
                    pcb = [r for r in rs if r[0] == p + 8]
                    if not pcb:
                        continue
                    code = self.target(k, pcb[0][1], pcb[0][3])
                    if not code:
                        continue
                    others = [t for r in rs if r[0] != p + 8 for t in (self.target(k, r[1], r[3]) or (None, [], None))[1]]
                    for c in code[1]:
                        for d in others + cie_tg.get(cp, []):
                            self.add('fde', c, d)

    def roots(self):
        r = set()
        st = self.res.get('_start')
        if st and 0 < st[1] < 0xff00:
            r.add((st[0], st[1]))
        for (k, i) in self.secs:
            o = self.objs[k]
            s = o.S[i]
            n = s['name']
            if s['flags'] & 0x80:
                continue                                  # SHF_LINK_ORDER: never a root
            if s['flags'] & 0x200000:
                r.add((k, i)); continue                   # SHF_GNU_RETAIN
            if n in INIT or any(n.startswith(x + '.') for x in INIT):
                r.add((k, i)); continue
            if n == '.eh_frame':
                r.add((k, i)); continue
            if s['type'] == 7 or not s['flags'] & 2:
                g = self.group_of.get((k, i))
                if g and any(o.S[m]['flags'] & 2 for m in g):
                    continue
                r.add((k, i))
        return r

    def live(self, drop=()):
        live = set(self.roots())
        work = list(live)
        while work:
            s = work.pop()
            for kind, ds in self.adj.get(s, {}).items():
                if kind in drop:
                    continue
                for d in ds:
                    if d not in live:
                        live.add(d); work.append(d)
        return live

    def map_live(self):
        L = self.L
        lv, dead = set(), set()
        for line in open(os.path.join(L.d, 'prog.map')):
            t = line.split()
            if not t:
                continue
            if t[0] in ('place', 'nplace', 'piece', 'merged'):
                lv.add((L.files[int(t[1])], int(t[2])))
            elif t[0] == 'discard' and t[3] == 'gc':
                dead.add((L.files[int(t[1])], int(t[2])))
        return lv, dead

    def compare(self, live):
        lv, dead = self.map_live()
        miss = sorted(x for x in lv if x not in live and x not in self.dropped)
        extra = sorted(x for x in dead if x in live)
        return miss, extra, len(lv), len(dead)

    def frag_compare(self, live):
        """merged alloc sections: fragments referenced from live sections vs the map's pieces"""
        L = self.L
        mp = collections.defaultdict(set)
        for line in open(os.path.join(L.d, 'prog.map')):
            t = line.split()
            if t and t[0] == 'piece':
                mp[(L.files[int(t[1])], int(t[2]))].add(int(t[3], 16))
        out = []
        for (k, i) in self.secs:
            s = self.objs[k].S[i]
            if not (s['flags'] & 0x10 and s['flags'] & 2) or (k, i) not in live:
                continue
            o = self.objs[k]
            data = o.bytes[s['off']:s['off'] + s['size']]
            frags, p = [], 0
            if s['flags'] & 0x20:
                while p < len(data):
                    e = data.find(b'\0', p)
                    e = len(data) if e < 0 else e + 1
                    frags.append(p); p = e
            else:
                es = max(o.S[i].get('entsize', 0) or 1, 1)
                frags = list(range(0, len(data), es))
            import bisect
            want = set()
            for src, d in self.frag_refs.items():
                if src in live and (k, i) in d:
                    for off in d[(k, i)]:
                        j = bisect.bisect_right(frags, off) - 1
                        want.add(frags[max(j, 0)])
            got = mp.get((k, i), set())
            if s['flags'] & 0x20:   # empty strings (alignment padding) are not compared
                empty = {f for f in frags if data[f:f + 1] == b'\0'}
                want, got = want - empty, got - empty
            out.append(((k, i), len(frags), want, got))
        return out


def short(x):
    (p, m, o), i = x
    return '%s%s:%d' % (os.path.basename(p), '(' + m + ')' if m else '', i)


def main():
    args = [a for a in sys.argv[1:] if not a.startswith('--')]
    drop = set()
    if '--drop' in sys.argv:
        v = sys.argv[sys.argv.index('--drop') + 1]
        drop = set(v.split(','))
        args = [a for a in args if a != v]
    rc = 0
    for d in args:
        t0 = time.time()
        M = Model(d)
        t1 = time.time()
        live = M.live(drop)
        miss, extra, nl, nd = M.compare(live)
        ok = not miss and not extra
        rc |= 0 if ok else 1
        print('== %s (dropped: %s): map live %d, map gc-dead %d; model live %d of %d sections; %s  (load %.2fs, model %.2fs)'
              % (d, ','.join(sorted(drop)) or 'none', nl, nd, len(live), len(M.secs), 'AGREE' if ok else 'DISAGREE', t1 - t0, time.time() - t1))
        if '--quiet' not in sys.argv:
            for x in miss[:8]:
                print('   live in the map, dead in the model: %s %s' % (short(x), M.objs[x[0]].S[x[1]]['name']))
            for x in extra[:8]:
                print('   gc-dead in the map, live in the model: %s %s' % (short(x), M.objs[x[0]].S[x[1]]['name']))
        if '--frag' in sys.argv:
            fr = M.frag_compare(live)
            bad = [(x, n, w, g) for (x, n, w, g) in fr if w != g]
            print('   merged alloc sections live: %d; fragment sets equal %d, differ %d' % (len(fr), len(fr) - len(bad), len(bad)))
            for (x, n, w, g) in bad[:6]:
                print('     %s %s: %d fragments, model %d live, map %d pieces; model-only %s map-only %s'
                      % (short(x), M.objs[x[0]].S[x[1]]['name'], n, len(w), len(g), sorted(w - g)[:4], sorted(g - w)[:4]))
        if '--stats' in sys.argv:
            print('   edges whose source is live (distinct (source section, target section) pairs; bytes at 8 a row):')
            tot = 0
            for kind in KINDS:
                n = sum(1 for (s, t) in M.edges[kind] if s in live)
                tot += n
                print('     %-10s %9d  %11d B' % (kind, n, 8 * n))
            print('     %-10s %9d  %11d B' % ('total', tot, 8 * tot))
            print('   live sections %d; roots %d' % (len(live), len(M.roots())))
    sys.exit(rc)


if __name__ == '__main__':
    main()
