# model_extract.py [SEEDS]: WP-207's archive-extraction rule for an update
# (spec/incr.md IN-71b) written from its text, checked against
# resolve.md §4.1's closure computed from scratch, on R-L's two cases (R1a:
# a weak definition after the archive, a strong member before it; R1b: two
# members that refer to each other, dropped together) and on random
# programs with random edit sequences (several hops each).
#
# What is stored between links (the model's "state"): every file's
# mentions as KEY_REFS rows (key, file, class), lazy members' defined and
# common mentions only; per live member its ROUND (a well-founded support:
# some live file with an edge to it has a smaller round; roots 0). Nothing
# else. An update reads the changed files' new mentions, recomputes
# first(K)/cfirst(K) for the affected keys from KEY_REFS, applies the edge
# changes, repairs removals (the unsupported cone, re-seeded from outside),
# then follows additions forward. A newly extracted member's undefined
# mentions are "parsed" (read from the member) at that point.
#
# What it does not model: --whole-archive, shared objects, --wrap, file 0's
# Assign candidates, COMDAT (extraction ignores it, resolve.md §4.1), a
# file appearing or disappearing from the line (options-changed).
import random, sys

STRONG, WEAK, COMMON, UNDEF, WUNDEF = 'S', 'W', 'C', 'U', 'w'


class Prog:
    def __init__(self, files):
        # files: list of dict(lazy=bool, m={key: (cls, data)}), index = FileId; file 0 the roots holder
        self.files = files

    def first(self, k):
        for i, f in enumerate(self.files):
            c = f['m'].get(k)
            if c and c[0] in (STRONG, WEAK, COMMON):
                return i
        return None

    def cfirst(self, k):
        for i, f in enumerate(self.files):
            c = f['m'].get(k)
            if not c: continue
            if not f['lazy'] and c[0] == STRONG: return i
            if f['lazy'] and c[0] == STRONG and c[1]: return i
        return None

    def edges_from(self, i, first, cfirst):
        out = []
        for k, (cls, data) in self.files[i]['m'].items():
            if cls == UNDEF:
                t = first(k)
                if t is not None and self.files[t]['lazy']: out.append(t)
            if cls == COMMON:
                t = cfirst(k)
                if t is not None and self.files[t]['lazy']: out.append(t)
        return out


def closure(p, roots_keys):
    """resolve.md §4.1 from scratch: reachability from the non-lazy files and file 0's root keys."""
    live = {i for i, f in enumerate(p.files) if not f['lazy']}
    work = list(live)
    for k in roots_keys:
        t = p.first(k)
        if t is not None and p.files[t]['lazy'] and t not in live: live.add(t); work.append(t)
    while work:
        i = work.pop()
        for t in p.edges_from(i, p.first, p.cfirst):
            if t not in live: live.add(t); work.append(t)
    return live


class State:
    """What the first link stores, and what each update rewrites."""
    def __init__(self, p, roots_keys):
        self.roots_keys = roots_keys
        live = closure(p, roots_keys)
        # ROUND: BFS rounds of the first link (any well-founded support would do)
        rnd = {i: 0 for i, f in enumerate(p.files) if not p.files[i]['lazy']}
        frontier = list(rnd)
        for k in roots_keys:
            t = p.first(k)
            if t is not None and p.files[t]['lazy'] and t not in rnd: rnd[t] = 1; frontier.append(t)
        r = 0
        while frontier:
            r += 1; nxt = []
            for i in frontier:
                for t in p.edges_from(i, p.first, p.cfirst):
                    if t not in rnd: rnd[t] = rnd[i] + 1; nxt.append(t)
            frontier = nxt
        assert set(rnd) == live
        self.round = rnd
        # KEY_REFS: (key -> {file: class}); a lazy member's undefined mentions are not stored
        self.refs = {}
        for i, f in enumerate(p.files):
            for k, c in f['m'].items():
                if f['lazy'] and i not in live and c[0] in (UNDEF, WUNDEF): continue
                self.refs.setdefault(k, {})[i] = c
        self.lazy = [f['lazy'] for f in p.files]


def update(st, newp, changed):
    """IN-71b, from the stored rows and the changed files' new mentions only."""
    refs = st.refs
    old_round = dict(st.round)
    live = set(old_round)
    # the affected keys and the edges they carried, from the stored rows
    affected = set()
    for i in changed:
        affected |= {k for k, m in refs.items() if i in m}
        affected |= set(newp.files[i]['m'])
    def first_of(rf, k):
        for i in sorted(rf.get(k, {})):
            if rf[k][i][0] in (STRONG, WEAK, COMMON): return i
        return None
    def cfirst_of(rf, k):
        for i in sorted(rf.get(k, {})):
            c = rf[k][i]
            if not st.lazy[i] and c[0] == STRONG: return i
            if st.lazy[i] and c[0] == STRONG and c[1]: return i
        return None
    def edges_of_key(rf, k, srcs_live):
        """(src, target) edges carried by key k among live sources."""
        e = set(); t = first_of(rf, k); ct = cfirst_of(rf, k)
        for i, c in rf.get(k, {}).items():
            if i not in srcs_live: continue
            if c[0] == UNDEF and t is not None and st.lazy[t]: e.add((i, t))
            if c[0] == COMMON and ct is not None and st.lazy[ct]: e.add((i, ct))
        if k in st.roots_keys and t is not None and st.lazy[t]: e.add((-1, t))
        return e
    old_edges = set()
    for k in affected: old_edges |= edges_of_key(refs, k, live)
    # write the changed files' new rows (lazy members keep defined/common only while lazy)
    for k in list(refs):
        for i in changed: refs[k].pop(i, None)
    for i in changed:
        for k, c in newp.files[i]['m'].items():
            refs.setdefault(k, {})[i] = c
    new_edges = set()
    for k in affected: new_edges |= edges_of_key(refs, k, live)
    lost = old_edges - new_edges
    gained = new_edges - old_edges
    rnd = dict(old_round)
    def in_edges(t, live_now):
        """every edge into member t from a live file: the keys t is first/cfirst of, their referrers (KEY_REFS)."""
        e = []
        for k, m in refs.items():
            if t in m and m[t][0] in (STRONG, WEAK, COMMON):
                for (s, tt) in edges_of_key(refs, k, live_now):
                    if tt == t: e.append(s)
        return e
    def round_of(s):
        return 0 if s == -1 else rnd.get(s)
    # removals: the unsupported cone X
    X = set()
    cand = sorted({t for (_, t) in lost if t in rnd})
    def supported(t, excl):
        return any(s == -1 or (s in rnd and s not in excl and round_of(s) < rnd[t]) for s in in_edges(t, set(rnd)))
    work = [t for t in cand if not supported(t, set())]
    X |= set(work)
    while work:
        x = work.pop()
        for (s, t) in edges_out(refs, st, x):
            if t in rnd and t not in X and not st_is_root(st, t) and not supported(t, X):
                X.add(t); work.append(t)
    # re-seed the cone from outside, in round order
    outside = set(rnd) - X
    seeds = {}
    for t in X:
        rs = [round_of(s) for s in in_edges(t, set(rnd)) if s == -1 or s in outside]
        if rs: seeds[t] = min(rs) + 1
    for t in X: rnd.pop(t)
    import heapq
    h = [(r, t) for t, r in seeds.items()]; heapq.heapify(h)
    while h:
        r, t = heapq.heappop(h)
        if t in rnd: continue
        rnd[t] = r
        for (_, u) in edges_out(refs, st, t):
            if u in X and u not in rnd: heapq.heappush(h, (r + 1, u))
    # additions: forward from the live sources of gained edges; a newly
    # extracted member is parsed: its rows (undefined mentions included) are written
    work = [(s, t) for (s, t) in gained if (s == -1 or s in rnd) and t not in rnd]
    while work:
        s, t = work.pop()
        if t in rnd: continue
        rnd[t] = round_of(s) + 1
        for k, c in newp.files[t]['m'].items():
            refs.setdefault(k, {})[t] = c
        for e in edges_out(refs, st, t): work.append(e)
    # members no longer extracted: their undefined rows are dropped (lazy rows)
    for t in set(old_round) - set(rnd):
        for k in list(refs):
            if t in refs[k] and refs[k][t][0] in (UNDEF, WUNDEF): refs[k].pop(t)
    st.round = rnd
    return set(rnd)


def st_is_root(st, t):
    return not st.lazy[t]


def edges_out(refs, st, i):
    out = []
    for k, m in refs.items():
        c = m.get(i)
        if not c: continue
        if c[0] == UNDEF:
            t = None
            for j in sorted(m):
                if m[j][0] in (STRONG, WEAK, COMMON): t = j; break
            if t is not None and st.lazy[t]: out.append((i, t))
        if c[0] == COMMON:
            t = None
            for j in sorted(m):
                cc = m[j]
                if (not st.lazy[j] and cc[0] == STRONG) or (st.lazy[j] and cc[0] == STRONG and cc[1]): t = j; break
            if t is not None and st.lazy[t]: out.append((i, t))
    return out


def rand_prog(rng, nobj, nmem, nkeys):
    files = []
    for i in range(nobj + nmem):
        lazy = i >= nobj
        m = {}
        for _ in range(rng.randint(1, 4)):
            k = rng.randrange(nkeys)
            m[k] = (rng.choice([STRONG, STRONG, WEAK, COMMON, UNDEF, UNDEF, UNDEF, WUNDEF]), rng.random() < 0.5)
        files.append(dict(lazy=lazy, m=m))
    # interleave: members in the middle of the line too
    order = list(range(len(files)))
    rng.shuffle(order)
    return Prog([files[i] for i in order])


def edit(rng, p, nkeys, nfiles):
    q = Prog([dict(lazy=f['lazy'], m=dict(f['m'])) for f in p.files])
    ch = rng.sample(range(len(q.files)), nfiles)
    for i in ch:
        m = q.files[i]['m']
        for _ in range(rng.randint(1, 3)):
            op = rng.random()
            if op < 0.4 and m: m.pop(rng.choice(list(m)))
            else: m[rng.randrange(nkeys)] = (rng.choice([STRONG, WEAK, COMMON, UNDEF, UNDEF, WUNDEF]), rng.random() < 0.5)
    return q, ch


def named():
    # R1a: c.o libk.a(k.o) u.o; u.o defines K weak, k.o strong; v2's c.o calls K
    v1 = Prog([dict(lazy=False, m={'main': (STRONG, False)}), dict(lazy=True, m={'K': (STRONG, False)}),
               dict(lazy=False, m={'K': (WEAK, False)})])
    v2 = Prog([dict(lazy=False, m={'main': (STRONG, False), 'K': (UNDEF, False)}), v1.files[1], v1.files[2]])
    st = State(v1, []); got = update(st, v2, [0]); want = closure(v2, [])
    print(f'R1a weak definition after the archive: update live {sorted(got)}, full {sorted(want)}: {"ok" if got == want else "WRONG"}')
    # R1b: d.o calls m1; m1 <-> m2 refer to each other; v2's d.o drops the call
    v1 = Prog([dict(lazy=False, m={'m1f': (UNDEF, False)}), dict(lazy=True, m={'m1f': (STRONG, False), 'm2f': (UNDEF, False)}),
               dict(lazy=True, m={'m2f': (STRONG, False), 'm1f': (UNDEF, False)})])
    v2 = Prog([dict(lazy=False, m={}), v1.files[1], v1.files[2]])
    st = State(v1, []); got = update(st, v2, [0]); want = closure(v2, [])
    print(f'R1b member cycle dropped together: update live {sorted(got)}, full {sorted(want)}: {"ok" if got == want else "WRONG"}')
    return 0


def main():
    seeds = int(sys.argv[1]) if len(sys.argv) > 1 else 300
    named()
    bad = hops = flips = 0
    for seed in range(seeds):
        rng = random.Random(seed)
        nk = rng.randint(3, 12)
        p = rand_prog(rng, rng.randint(1, 4), rng.randint(1, 8), nk)
        roots = [rng.randrange(nk)] if rng.random() < 0.3 else []
        st = State(p, roots)
        for hop in range(4):
            q, ch = edit(rng, p, nk, rng.randint(1, 2))
            before = set(st.round)
            got = update(st, q, ch)
            want = closure(q, roots)
            hops += 1; flips += len(got ^ before)
            if got != want:
                bad += 1
                if bad <= 5: print(f'seed {seed} hop {hop}: update {sorted(got)} full {sorted(want)} changed {ch}')
            # the state invariant: every live member has a support with a smaller round
            p = q
    print(f'random: {seeds} programs, {hops} updates, {flips} extraction flips, {bad} differ from the full closure')


if __name__ == '__main__':
    main()
