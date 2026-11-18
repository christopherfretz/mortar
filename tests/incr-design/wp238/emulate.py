#!/usr/bin/env python3
"""emulate.py OLD.rows NEW.rows -- WP-238 step 1, experiments (d) and item 4.

Two probe dumps (crates/mortar_live/src/gc/probe.rs, MORTAR_WP238_ROWS) of
full --gc-sections links: OLD (the stored link) and NEW (the full link of
the edited inputs). Everything is named by identity, never by id: a file
by (path, member), a section by (file, index), a key by name.

The emulated update (IN-29, IN-71a): the rows and root bits of every
UNCHANGED file (same bytes in OLD and NEW, same kept sections) are OLD's;
those of changed, appeared and drawn-in-by-election files are NEW's (the
enumerator run on the new bytes); keys resolve by NEW's resolution
(IN-26's); the rooted keys are NEW's. Walked from the roots, at section
and at fragment level, and compared with NEW's live set.

Also checked: an unchanged file's OLD rows and root bits equal its NEW
ones (the stored form stays true when its neighbours change), and item 4's
skip rule: when it holds, OLD's live set must equal NEW's.

Last line: `AGREE ...` or `DISAGREE ...`.
"""
import collections, sys


class Dump:
    def __init__(self, path):
        self.files = {}      # fid -> (path, member)
        self.hash = {}       # (path, member) -> hash
        self.sec = {}        # sid -> (fileident, index)
        self.info = {}       # secident -> (name, kept, live, bits, split)
        self.keys = {}       # kid -> name
        self.target = {}     # keyname -> ('S', secident, at) | ('N', (secident...)) | None
        self.rooted = set()  # key names rooted
        self.rows = collections.defaultdict(set)   # fileident -> {(srcident, kind, to)}
        self.pieces = collections.defaultdict(list)  # secident -> [(off, frag)]
        self.live_frags = set()
        raw_k, raw_e = [], []
        for line in open(path, encoding='utf-8', errors='replace'):
            t = line.rstrip('\n').split('\t')
            if t[0] == 'F':
                fi = (t[2], t[3])
                self.files[int(t[1])] = fi
                self.hash[fi] = t[4]
            elif t[0] == 'S':
                sid, fid, idx = int(t[1]), int(t[2]), int(t[3])
                si = (self.files[fid], idx)
                self.sec[sid] = si
                self.info[si] = (t[4], t[5] == '1', t[6] == '1', int(t[7]), t[8] == '1')
            elif t[0] == 'K':
                raw_k.append(t)
            elif t[0] == 'R':
                raw_k.append(['R', t[1]])
            elif t[0] == 'E':
                raw_e.append(t)
            elif t[0] == 'P':
                self.pieces[int(t[1])].append((int(t[2]), int(t[3])))
            elif t[0] == 'L':
                self.live_frags.add(int(t[1]))
        roots_by_id = []
        for t in raw_k:
            if t[0] == 'R':
                roots_by_id.append(int(t[1]))
                continue
            kid, name, tgt, rooted = int(t[1]), t[2], t[3], t[4]
            self.keys[kid] = name
            if tgt == '-':
                self.target[name] = None
            elif tgt.startswith('S '):
                _s, sid, at = tgt.split(' ')
                self.target[name] = ('S', self.sec.get(int(sid)), int(at))
            else:
                ids = tgt[2:].split(',') if len(tgt) > 2 else []
                self.target[name] = ('N', tuple(self.sec[int(x)] for x in ids if x))
            if rooted == '1':
                self.rooted.add(name)
        for kid in roots_by_id:
            self.rooted.add(self.keys[kid])
        for t in raw_e:
            src = self.sec[int(t[1])]
            kind = int(t[2])
            if t[3] == 'K':
                to = ('K', self.keys[int(t[4])])
            else:
                to = ('S', self.sec[int(t[4])], int(t[5]))
            self.rows[src[0]].add((src, kind, to))
        # pieces by section identity, sorted by offset
        self.pieces = {self.sec[s]: sorted(v) for s, v in self.pieces.items()}

    def file_secs(self, fi):
        return {si for si in self.info if si[0] == fi}


def walk(N, rows, rootbits):
    """The walk of IN-71a over `rows` (fileident -> set of rows), from
    `rootbits` (sections) and N's rooted keys, resolving by N. Returns
    (live sections, live fragments of N)."""
    by_src = collections.defaultdict(list)
    for rs in rows.values():
        for (src, kind, to) in rs:
            by_src[src].append(to)
    live, frags, whole = set(), set(), set()
    stack = []

    def at(s, off):
        if s is None:
            return
        inf = N.info.get(s)
        if inf is None or not inf[1]:      # only a kept section is marked
            return
        if s not in live:
            live.add(s)
            stack.append(s)
        if not inf[4]:
            return
        ps = N.pieces.get(s, [])
        if off == -1:
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

    def key(name):
        t = N.target.get(name)
        if t is None:
            return
        if t[0] == 'S':
            at(t[1], t[2])
        else:
            for s in t[1]:
                at(s, -1)

    for s in sorted(rootbits):
        at(s, -1)
    for name in sorted(N.rooted):
        key(name)
    while stack:
        s = stack.pop()
        for to in by_src.get(s, ()):
            if to[0] == 'K':
                key(to[1])
            else:
                at(to[1], to[2])
    return live, frags


def main():
    O, N = Dump(sys.argv[1]), Dump(sys.argv[2])
    changed, drawn, same = [], [], []
    for fi in N.hash:
        if O.hash.get(fi) != N.hash[fi]:
            changed.append(fi)
            continue
        okept = {si[1] for si in O.file_secs(fi) if O.info[si][1]}
        nkept = {si[1] for si in N.file_secs(fi) if N.info[si][1]}
        (drawn if okept != nkept else same).append(fi)
    gone = [fi for fi in O.hash if fi not in N.hash]
    rows, rootbits = {}, set()
    stale_rows = stale_roots = 0
    for fi in N.hash:
        src = N if fi in changed or fi in drawn else O
        rows[fi] = src.rows.get(fi, set())
        for si in src.file_secs(fi):
            inf = src.info[si]
            if inf[1] and inf[3] & 2:
                rootbits.add(si)
        if fi in same:
            if O.rows.get(fi, set()) != N.rows.get(fi, set()):
                stale_rows += 1
            if {si for si in O.file_secs(fi) if O.info[si][3] & 2} != \
                    {si for si in N.file_secs(fi) if N.info[si][3] & 2}:
                stale_roots += 1
    live, frags = walk(N, rows, rootbits)
    want = {si for si, inf in N.info.items() if inf[2]}
    sec_miss, sec_extra = want - live, live - want
    fr_miss, fr_extra = N.live_frags - frags, frags - N.live_frags
    # Liveness flips of sections of unchanged files (item 5's subjects).
    flips = [si for si in N.info if si[0] in same and O.info.get(si, (0, 0, False))[2] != N.info[si][2]]
    # Item 4's skip rule.
    reasons = []
    if gone or any(fi not in O.hash for fi in N.hash):
        reasons.append('appeared/gone')
    if drawn:
        reasons.append('election')
    if any(O.rows.get(fi, set()) != N.rows.get(fi, set()) for fi in changed):
        reasons.append('rows')
    if any({si for si in O.file_secs(fi) if O.info[si][3] & 2} != {si for si in N.file_secs(fi) if N.info[si][3] & 2}
           for fi in changed):
        reasons.append('roots')
    named = set()
    for fi in changed:
        for (_s, _k, to) in N.rows.get(fi, set()) | O.rows.get(fi, set()):
            if to[0] == 'K':
                named.add(to[1])
    moved = [k for k in set(O.target) | set(N.target) if O.target.get(k) != N.target.get(k)]
    if moved:
        reasons.append('defs(%d)' % len(moved))
    if O.rooted != N.rooted:
        reasons.append('rooted-keys')
    skip = not reasons
    skip_ok = None
    if skip:
        olive = {si for si, inf in O.info.items() if inf[2]}
        skip_ok = olive == want
    ok = not (sec_miss or sec_extra or fr_miss or fr_extra or stale_rows or stale_roots) and skip_ok is not False
    print('changed %d drawn-by-election %d unchanged %d gone %d; unchanged files with stale rows %d, stale roots %d; '
          'flips in unchanged files %d %s' % (len(changed), len(drawn), len(same), len(gone), stale_rows, stale_roots,
                                               len(flips), sorted(f'{s[0][0].split("/")[-1]}{s[0][1]}:{s[1]}:{N.info[s][0]}:{int(N.info[s][2])}' for s in flips)[:4]))
    print('%s sections live %d (miss %d extra %d) fragments live %d (miss %d extra %d); skip rule %s%s' % (
        'AGREE' if ok else 'DISAGREE', len(want), len(sec_miss), len(sec_extra), len(N.live_frags), len(fr_miss),
        len(fr_extra), 'HOLDS' if skip else 'fails: ' + ','.join(reasons),
        '' if skip_ok is None else (' and OLD live == NEW live' if skip_ok else ' BUT OLD live != NEW live')))


if __name__ == '__main__':
    main()
