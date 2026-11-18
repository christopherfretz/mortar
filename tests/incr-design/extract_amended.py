# Runs R-2's extract_mine.py (read-only, imported by path) in the reading
# WP-211's amended IN-71b states ("archive member (extracted or not)" = the
# model's 'kind'), and judges ROUND by IN-78's amended canon: the SUPPORT
# property (every live member has a live in-edge from a smaller round),
# not equality with the breadth-first round.
import importlib.util, random, sys
spec = importlib.util.spec_from_file_location('em', '/Volumes/AgentWork/mortar-helper/.build/review-wp207/extract_mine.py')
em = importlib.util.module_from_spec(spec); spec.loader.exec_module(em)

def supported(files, roots, rnd):
    for t, r in rnd.items():
        if not files[t]['member']:
            if r != 0: return False
            continue
        ok = any(em.first(files, k) == t for k in roots) and r > 0
        for s, rs in rnd.items():
            if ok: break
            if rs >= r: continue
            for k, c in files[s]['m'].items():
                tt = em.first(files, k) if c[0] == em.U else em.cfirst(files, k) if c[0] == em.C else None
                if tt == t: ok = True; break
        if not ok: return False
    return True

def named():
    out = []
    for name, ok in em.named('kind'):
        out.append((name, ok))
    return out

seeds = int(sys.argv[1]) if len(sys.argv) > 1 else 3000
for name, ok in named():
    print(f'  {name}: {"ok" if ok else "DIFFERS"}')
hops = bad = notbfs = unsup = flips = 0
for seed in range(seeds):
    rng = random.Random(seed)
    nk = rng.randint(3, 12)
    fs = em.rand_files(rng, rng.randint(1, 4), rng.randint(1, 8), nk)
    roots = [rng.randrange(nk)] if rng.random() < 0.3 else []
    st = em.St(fs, roots)
    for hop in range(4):
        g, ch = em.edit(rng, fs, nk, rng.randint(1, 2))
        before = set(st.round)
        got = em.update(st, g, ch, 'kind')
        want = em.full(g, roots)
        hops += 1; flips += len(set(got) ^ before)
        if set(got) != set(want): bad += 1
        elif got != want: notbfs += 1
        if not supported(g, roots, got): unsup += 1
        fs = g
print(f'  amended reading: {seeds} programs, {hops} updates, {flips} flips; member set differs: {bad}; '
      f'round != BFS (no longer a failure): {notbfs}; support property violated: {unsup}')
