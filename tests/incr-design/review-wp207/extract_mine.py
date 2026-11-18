# extract_mine.py [SEEDS]: R-2's OWN model of resolve.md §4.1 (closure from
# scratch, written from §4.1 alone) and of spec/incr.md IN-71b (written from
# IN-71b's text alone), in two readings of the word "lazy" that IN-71b uses:
#   kind    - "a lazy first(K)" / "lazy rows" = an ARCHIVE MEMBER (resolve.md
#             §4.1's sense: lazy until reached; extracted or not);
#   literal - "lazy" = the FILES `LAZY` flag of IN-18e ("flag LAZY when not
#             [extracted]"), the sense IN-71b's own first bullet uses
#             ("first(K), the lowest FileId, extracted or lazy").
# Also checks IN-78: the stored ROUND after each update against the
# breadth-first round a full link writes (IN-71b: "the first link writes the
# breadth-first round of §4.1's walk"; IN-78 lists FILES (round) among the
# rows "computed afresh" by C(U)).
import heapq, random, sys

S, W, C, U, w = 'S', 'W', 'C', 'U', 'w'   # strong/weak def, common, undefined, weak undefined
DEF = (S, W, C)


def first(files, k):
    for i, f in enumerate(files):
        if f['m'].get(k, (None,))[0] in DEF:
            return i
    return None


def cfirst(files, k):
    for i, f in enumerate(files):
        c = f['m'].get(k)
        if not c:
            continue
        if not f['member'] and c[0] == S:
            return i
        if f['member'] and c[0] == S and c[1] == 'data':
            return i
    return None


def full(files, roots):
    """§4.1 closure, breadth-first; returns {file: round} of live files."""
    rnd = {i: 0 for i, f in enumerate(files) if not f['member']}
    fr = list(rnd)
    for k in roots:
        t = first(files, k)
        if t is not None and files[t]['member'] and t not in rnd:
            rnd[t] = 1
            fr.append(t)
    while fr:
        nxt = []
        for i in sorted(fr):
            for k, c in files[i]['m'].items():
                t = first(files, k) if c[0] == U else cfirst(files, k) if c[0] == C else None
                if t is not None and files[t]['member'] and t not in rnd:
                    rnd[t] = rnd[i] + 1
                    nxt.append(t)
        fr = nxt
    return rnd


class St:
    def __init__(self, files, roots):
        self.member = [f['member'] for f in files]
        self.roots = roots
        self.round = full(files, roots)
        # KEY_REFS: live files' every mention; lazy members' defined/common only
        self.refs = {}
        for i, f in enumerate(files):
            for k, c in f['m'].items():
                if f['member'] and i not in self.round and c[0] in (U, w):
                    continue
                self.refs.setdefault(k, {})[i] = c


def update(st, newfiles, changed, reading):
    refs, rnd = st.refs, dict(st.round)

    def live(i):
        return i == -1 or i in rnd

    def eligible(t):           # may an edge point at t?
        if t is None or not st.member[t]:
            return False
        return True if reading == 'kind' else t not in rnd

    def kfirst(k):
        for i in sorted(refs.get(k, {})):
            if refs[k][i][0] in DEF:
                return i

    def kcfirst(k):
        for i in sorted(refs.get(k, {})):
            c = refs[k][i]
            # §4.1's (a)/(b) split: (a) files that are not lazy, (b) lazy files
            lazy_i = st.member[i] if reading == 'kind' else (st.member[i] and i not in rnd)
            if (not lazy_i and c[0] == S) or (lazy_i and c[0] == S and c[1] == 'data'):
                return i

    def edges(k):
        e = set()
        t, ct = kfirst(k), kcfirst(k)
        for i, c in refs.get(k, {}).items():
            if not live(i):
                continue
            if c[0] == U and eligible(t):
                e.add((i, t))
            if c[0] == C and eligible(ct):
                e.add((i, ct))
        if k in st.roots and eligible(t):
            e.add((-1, t))
        return e

    affected = set()
    for i in changed:
        affected |= {k for k, m in refs.items() if i in m} | set(newfiles[i]['m'])
    before = set().union(*[edges(k) for k in affected]) if affected else set()
    for k in list(refs):
        for i in changed:
            refs[k].pop(i, None)
    for i in changed:
        for k, c in newfiles[i]['m'].items():
            if st.member[i] and i not in rnd and c[0] in (U, w):
                continue
            refs.setdefault(k, {})[i] = c
    after = set().union(*[edges(k) for k in affected]) if affected else set()
    lost, gained = before - after, after - before

    def in_edges(t):
        out = []
        for k, m in refs.items():
            if t in m and m[t][0] in DEF:
                out += [s for (s, tt) in edges(k) if tt == t]
        return out

    def out_targets(x):
        return [t for k, m in refs.items() if x in m for (s, t) in edges(k) if s == x]

    def rd(s):
        return 0 if s == -1 else rnd[s]

    # removal: members that lost an edge and have no smaller-round support
    cone = set()
    work = [t for (_, t) in lost if t in rnd and st.member[t]
            and not any(live(s) and rd(s) < rnd[t] for s in in_edges(t))]
    cone |= set(work)
    while work:
        x = work.pop()
        for t in out_targets(x):
            if t in rnd and st.member[t] and t not in cone and \
                    not any(live(s) and s not in cone and rd(s) < rnd[t] for s in in_edges(t)):
                cone.add(t)
                work.append(t)
    seeds = {}
    for t in cone:
        rs = [rd(s) for s in in_edges(t) if live(s) and s not in cone]
        if rs:
            seeds[t] = min(rs) + 1
    for t in cone:
        rnd.pop(t)
    h = [(r, t) for t, r in seeds.items()]
    heapq.heapify(h)
    while h:
        r, t = heapq.heappop(h)
        if t in rnd:
            continue
        rnd[t] = r
        for u in out_targets(t):
            if u in cone and u not in rnd:
                heapq.heappush(h, (r + 1, u))
    # addition, breadth-first
    q = sorted((rd(s), s, t) for (s, t) in gained if live(s) and t not in rnd)
    heapq.heapify(q)
    while q:
        r, s, t = heapq.heappop(q)
        if t in rnd:
            continue
        rnd[t] = r + 1
        for k, c in newfiles[t]['m'].items():
            refs.setdefault(k, {})[t] = c
        for u in out_targets(t):
            if u not in rnd:
                heapq.heappush(q, (rnd[t], t, u))
    for t in set(st.round) - set(rnd):
        for k in list(refs):
            if t in refs[k] and refs[k][t][0] in (U, w):
                refs[k].pop(t)
    st.round = rnd
    return rnd


def rand_files(rng, nobj, nmem, nk):
    fs = []
    for i in range(nobj + nmem):
        m = {}
        for _ in range(rng.randint(1, 4)):
            m[rng.randrange(nk)] = (rng.choice([S, S, W, C, U, U, U, w]), rng.choice(['data', 'func']))
        fs.append(dict(member=i >= nobj, m=m))
    rng.shuffle(fs)
    return fs


def edit(rng, fs, nk, n):
    g = [dict(member=f['member'], m=dict(f['m'])) for f in fs]
    ch = rng.sample(range(len(g)), n)
    for i in ch:
        m = g[i]['m']
        for _ in range(rng.randint(1, 3)):
            if rng.random() < 0.4 and m:
                m.pop(rng.choice(list(m)))
            else:
                m[rng.randrange(nk)] = (rng.choice([S, W, C, U, U, w]), rng.choice(['data', 'func']))
    return g, ch


def named(reading):
    out = []
    # R1a: c.o libk.a(k.o) u.o; u weak K, k strong K; c starts calling K
    v1 = [dict(member=False, m={'main': (S, 'func')}), dict(member=True, m={'K': (S, 'func')}),
          dict(member=False, m={'K': (W, 'func')})]
    v2 = [dict(member=False, m={'main': (S, 'func'), 'K': (U, 'func')}), v1[1], v1[2]]
    st = St(v1, [])
    got = set(update(st, v2, [0], reading))
    out.append(('R1a', got == set(full(v2, []))))
    # R1b: d.o calls m1; m1<->m2; d.o drops the call
    v1 = [dict(member=False, m={'a': (U, 'func')}), dict(member=True, m={'a': (S, 'func'), 'b': (U, 'func')}),
          dict(member=True, m={'b': (S, 'func'), 'a': (U, 'func')})]
    v2 = [dict(member=False, m={}), v1[1], v1[2]]
    st = St(v1, [])
    got = set(update(st, v2, [0], reading))
    out.append(('R1b', got == set(full(v2, []))))
    # R2-1: shortcut: d.o -> m1 -> m2 -> m3; changed e.o starts calling m3 directly (round 3 -> BFS 1)
    v1 = [dict(member=False, m={'a': (U, 'func')}), dict(member=False, m={}),
          dict(member=True, m={'a': (S, 'func'), 'b': (U, 'func')}),
          dict(member=True, m={'b': (S, 'func'), 'c': (U, 'func')}),
          dict(member=True, m={'c': (S, 'func')})]
    v2 = [v1[0], dict(member=False, m={'c': (U, 'func')}), v1[2], v1[3], v1[4]]
    st = St(v1, [])
    got = update(st, v2, [1], reading)
    out.append(('R2-1 set', set(got) == set(full(v2, []))))
    out.append(('R2-1 round==BFS', got == full(v2, [])))
    return out


def main():
    seeds = int(sys.argv[1]) if len(sys.argv) > 1 else 3000
    for reading in ('kind', 'literal'):
        print(f'--- reading: {reading}')
        for name, ok in named(reading):
            print(f'  {name}: {"ok" if ok else "DIFFERS"}')
        hops = bad = badround = flips = 0
        first_bad = None
        for seed in range(seeds):
            rng = random.Random(seed)
            nk = rng.randint(3, 12)
            fs = rand_files(rng, rng.randint(1, 4), rng.randint(1, 8), nk)
            roots = [rng.randrange(nk)] if rng.random() < 0.3 else []
            st = St(fs, roots)
            for hop in range(4):
                g, ch = edit(rng, fs, nk, rng.randint(1, 2))
                before = set(st.round)
                got = update(st, g, ch, reading)
                want = full(g, roots)
                hops += 1
                flips += len(set(got) ^ before)
                if set(got) != set(want):
                    bad += 1
                    first_bad = first_bad or (seed, hop)
                elif got != want:
                    badround += 1
                fs = g
        print(f'  random: {seeds} programs, {hops} updates, {flips} flips; member set differs: {bad}'
              f' (first seed/hop {first_bad}); set right but some ROUND != full link BFS round: {badround}')


if __name__ == '__main__':
    main()
