#!/usr/bin/env python3
"""WP-239: the stored rows proposed for IN-72 items 5, 6, 7, MAINTAINED over
a series of real links (hop i-1 -> hop i, as an update would: only the
files that changed, appeared, went or were drawn in are re-read) and
compared with the rows computed AFRESH from hop i's full link.

    model.py SERIES_DIR NHOPS [--pad FRACTION]

SERIES_DIR holds hN.map, hN.out, hN.state (m1series.sh). Per hop:

  files    changed (same identity, other bytes), appeared, gone, drawn in
           (a COMDAT group whose keeper, the first live file holding it,
           moved to or from the file: IN-72 item 2a).
  eh       S4-3: every file's `.eh_frame` range size, from the map. FAIL if
           a file outside the four sets has a range of another size (then
           a range row maintained from those sets alone would be stale).
           The changed file's size old -> new, and whether it fits the
           old range's capacity with pad 0 and with pad
           max(16, FRACTION * size) rounded up to 4.
  frags    S4-2: one row per (file, fragment it names) with refs = files
           naming the fragment; maintained = previous refs minus the old
           rows of the four sets plus their new rows; FAIL if it differs
           from the refs of hop i computed afresh, or if a fragment's
           identity set differs from the hop's state FRAGS row count.
           Also: how many of the changed file's old fragments the stored
           FRAG_AT (one row per fragment at its first occurrence) names.
  slots    S4-4: per file the global keys it uses through a slot, from the
           state's INDEX rows (kind SLOT), plus the GLOBAL references to a
           key holding an IPLT slot (an ifunc: its demand is any
           reference); refs per key maintained as above; FAIL if the keys
           with refs > 0 differ from the hop's SLOTS keys.
"""
import hashlib
import struct
import sys
from bisect import bisect_right
from collections import Counter, defaultdict

sys.path.insert(0, __import__('os').path.dirname(__file__))
import elfobj  # noqa: E402
import st  # noqa: E402

KINDS = ['got', 'plt', 'iplt', 'tlsdesc', 'tlsgd', 'tlsie']


_AR = {}


def member_bytes(path, member):
    if path not in _AR:
        _AR.clear()
        _AR[path] = open(path, 'rb').read()
    d = _AR[path]
    if member is None:
        return d
    size = int(d[member + 48:member + 58].decode().strip())
    return d[member + 60:member + 60 + size]


class Hop:
    def __init__(self, base):
        # the series copies each hop's changed object to hN.strutil.o
        base = __import__('os').path.abspath(base)
        here = base.rsplit('/', 1)[0]
        import glob
        import os
        tag = os.path.basename(base) + '.'
        self.subst = {here + '/' + os.path.basename(g)[len(tag):]: g
                      for g in glob.glob(base + '.*.o')}
        self.files = {}  # map id -> (path, member)
        pieces, merged, eh = [], [], None
        for line in open(base + '.map'):
            w = line.split()
            if not w:
                continue
            if w[0] == 'file':
                self.files[int(w[1])] = (w[2], int(w[4], 16) if len(w) > 4 else None)
            elif w[0] == 'piece':
                pieces.append((int(w[1]), int(w[2]), int(w[3], 16), int(w[4], 16), int(w[5], 16)))
            elif w[0] == 'merged':
                merged.append((int(w[1]), int(w[2]), w[3]))
            elif w[0] == 'chunk' and len(w) >= 5 and w[2] == '.eh_frame':
                eh = (int(w[3], 16), int(w[4], 16))
        self.key = {m: k for m, k in self.files.items()}
        self.ident = {k: m for m, k in self.files.items()}
        self.hash = {}
        self.elf = {}
        for m, (p, mem) in self.files.items():
            b = member_bytes(self.subst.get(p, p), mem)
            self.hash[m] = hashlib.blake2b(b, digest_size=16).digest()
        outsecs = elfobj.Elf.open(base + '.out').alloc_ranges()
        starts = [r[0] for r in outsecs]
        self.eh = Counter()
        self.ids = defaultdict(set)
        for f, s, inoff, addr, size in pieces:
            if eh[0] <= addr < eh[0] + eh[1]:
                self.eh[self.files[f]] += size
                continue
            e = self.obj(f)
            h = e.shdr(s)
            i = bisect_right(starts, addr) - 1
            cls = (bool(h['flags'] & 0x20), h['entsize'], max(h['addralign'], 1))
            data = e.section_bytes(s)[inoff:inoff + size]
            self.ids[self.files[f]].add((outsecs[i][2], cls, data))
        for f, s, name in merged:
            e = self.obj(f)
            h = e.shdr(s)
            cls = (bool(h['flags'] & 0x20), h['entsize'], max(h['addralign'], 1))
            data = e.section_bytes(s)
            for a, b in elfobj.cut(data, cls[0], cls[1]):
                self.ids[self.files[f]].add((name, cls, data[a:b]))
        self.state = st.State(base + '.state')
        # groups: signature -> first live file (keeper)
        self.keeper = {}
        for m in sorted(self.files):
            for sig, _ in self.obj(m).groups():
                self.keeper.setdefault(sig, self.files[m])
        # slots and per-file slot demand (by global key name)
        syms = self.state.symbols()
        sec_file = self.state.section_files()
        frows = self.state.files()
        self.slot_keys = set()
        self.slots = set()
        for k, sym in self.state.slots():
            kind, file, ordinal, name = syms[sym]
            if kind == 2:  # a local: IN-28b's, per file by definition
                continue
            self.slot_keys.add(name)
            self.slots.add((KINDS[k], name))
        iplt = {n for k, n in self.slots if k == 'iplt'}
        self.demand = defaultdict(set)
        self.demand_index_only = defaultdict(set)
        for unit, kind, target in self.state.index():
            if kind not in (0, 2):
                continue
            name = syms[target][3]
            fk = frows[sec_file[unit]][0]
            fkey = (fk[0], fk[1], fk[2])
            if kind == 2:
                self.demand[fkey].add(name)
                self.demand_index_only[fkey].add(name)
            elif name in iplt:
                self.demand[fkey].add(name)
        self.state_frags = self.state.t[11][0]

    def obj(self, m):
        if m not in self.elf:
            p, mem = self.files[m]
            self.elf = {m: elfobj.Elf.open(self.subst.get(p, p), mem)}
        return self.elf[m]


def skey(hop, k):
    """A map file key as the state's FILES identity (path, member name, ordinal) is not
    in the map; use the map's (path, member offset), and match the demand rows by path."""
    return k


def refs(sets):
    c = Counter()
    for s in sets.values():
        c.update(s)
    return c


def main():
    d = sys.argv[1]
    n = int(sys.argv[2])
    frac = float(sys.argv[sys.argv.index('--pad') + 1]) if '--pad' in sys.argv else 0.125
    hops = [Hop(f'{d}/h{i}') for i in range(n)]
    fails = 0
    caps = {}  # the range capacity an update keeps (IN-33): first link's, or a whole rewrite's
    overflows = [0, 0]

    def padf(size):
        return 0 if size == 0 else max(16, -(-int(size * frac) // 4) * 4)
    # demand keyed by path only for files (member names differ in form): map path+offset ->
    # state (path, member name): match on path and the member's order is enough for the
    # changed file (an object), so demand sets are compared per path+member name below.
    for i in range(1, n):
        a, b = hops[i - 1], hops[i]
        ka, kb = set(a.ident), set(b.ident)
        appeared, gone = kb - ka, ka - kb
        changed = {k for k in ka & kb if a.hash[a.ident[k]] != b.hash[b.ident[k]]}
        drawn = set()
        for sig in set(a.keeper) | set(b.keeper):
            x, y = a.keeper.get(sig), b.keeper.get(sig)
            if x != y:
                drawn |= {z for z in (x, y) if z is not None}
        drawn -= changed | appeared | gone
        touched = changed | appeared | gone | drawn
        print(f'hop {i}: changed {len(changed)} appeared {len(appeared)} gone {len(gone)} drawn_in {len(drawn)}'
              f' {[k[0].rsplit("/", 1)[-1] for k in sorted(drawn)][:6]}')
        # eh
        stale = [k for k in ka & kb if k not in touched and a.eh[k] != b.eh[k]]
        print(f'  eh check untouched_ranges_same {"ok" if not stale else "FAIL"} {len(stale)}')
        fails += bool(stale)
        for k in sorted(changed | drawn):
            old, new = a.eh[k], b.eh[k]
            cap = caps.get(k, old + padf(old))
            fits = new <= cap
            caps[k] = cap if fits else new + padf(new)
            overflows[0] += new > old
            overflows[1] += not fits
            print(f'  eh {k[0].rsplit("/", 1)[-1]} {old} -> {new} delta {new - old:+d} '
                  f'fits_pad0 {new <= old} fits_pad({frac}) {fits} (cap {cap})')
        # frags
        maintained = refs(a.ids)
        for k in touched:
            maintained.subtract(a.ids.get(k, set()))
            maintained.update(b.ids.get(k, set()))
        maintained = +maintained
        fresh = refs(b.ids)
        ok = maintained == fresh and len(fresh) == b.state_frags
        print(f'  frags check maintained_equals_fresh {"ok" if maintained == fresh else "FAIL"}'
              f' fragments {len(fresh)} state_FRAGS {b.state_frags} pairs {sum(fresh.values())}')
        fails += not ok
        for k in sorted(changed):
            old = a.ids.get(k, set())
            # the stored FRAG_AT: one row per fragment at its first occurrence
            first = {}
            for m in sorted(a.files):
                for x in a.ids.get(a.files[m], ()):
                    first.setdefault(x, a.files[m])
            named = sum(1 for x in old if first[x] == k)
            print(f'  frags {k[0].rsplit("/", 1)[-1]}: old fragments {len(old)}, of which FRAG_AT names {named};'
                  f' new {len(b.ids.get(k, set()))}; refs falling to 0 {sum(1 for x in old if fresh[x] == 0)},'
                  f' rising from 0 {sum(1 for x in b.ids.get(k, set()) if refs(a.ids)[x] == 0)}')
        # slots: per file demand by state FILES identity; match touched files by path
        def by_path(h):
            out = defaultdict(set)
            for fk, s in h.demand.items():
                out[(fk[0], fk[1])] |= s
            return out
        da, db = by_path(a), by_path(b)
        tp = set()
        for k in touched:
            for fk in list(da) + list(db):
                if fk[0] == k[0] or fk[0].endswith('/' + k[0].rsplit('/', 1)[-1]) and k[1] is None:
                    tp.add(fk)
        m_refs = refs(da)
        for fk in tp:
            m_refs.subtract(da.get(fk, set()))
            m_refs.update(db.get(fk, set()))
        live = {x for x, c in m_refs.items() if c > 0}
        ok = live == b.slot_keys
        print(f'  slots check maintained_keys_equal_SLOTS {"ok" if ok else "FAIL"} slots {len(b.slots)} '
              f'global keys {len(b.slot_keys)} extra {sorted(live - b.slot_keys)[:3]} missing {sorted(b.slot_keys - live)[:3]}'
              f' (touched files matched {len(tp)})')
        fails += not ok
        added, removed = b.slots - a.slots, a.slots - b.slots
        print(f'  slots delta +{len(added)} -{len(removed)} {sorted(added)[:3]} {sorted(removed)[:3]}')
    io = refs(hops[0].demand_index_only)
    print(f'index-only demand (INDEX kind SLOT): SLOTS keys with no user {len(hops[0].slot_keys - set(k for k, c in io.items() if c))}')
    print(f'eh overflows (whole-section rewrites) over {n - 1} hops: pad0 {overflows[0]} pad({frac}) {overflows[1]}')
    print(f'FINAL model fails={fails}')


sys.exit(main())
