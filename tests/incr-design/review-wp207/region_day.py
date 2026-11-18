# region_day.py [SEED] [HOPS]: R-2's own model of spec/incr.md IN-72 item 2
# (keep / grow in place / first fit / one overflow region per (file, O, c)
# per update) and item 3 (the tail, the growth step), written from the
# text, run over a "day" of random edits on abstract -ffunction-sections
# files (.text only): grow, shrink, add, remove one function per hop.
# IN-33's region capacity: size + max(64, size/8); the output section's tail
# reserve max(4 KiB, size/16) (item 3); a growth step adds
# need + max(4 KiB, size/16) when the tail cannot hold a new region.
# Reports per run: hops, overflow regions opened, regions per file at the
# end, growth steps, moved definitions (sections of the changed file whose
# address moved) per hop: mean / max, the fraction of the output section
# that is holes at the end (fragmentation), and the first hop needing a
# growth step.
import random, sys, statistics

def rup(x, a): return -(-x // a) * a

def run(seed, hops, nfiles, edit_files):
    rng = random.Random(seed)
    files = []
    addr = 0x400000
    for f in range(nfiles):
        secs = []
        for i in range(rng.randint(20, 120)):
            secs.append(dict(id=(f, i), size=max(8, int(rng.lognormvariate(5.3, 1.1))) // 4 * 4, align=rng.choice([4, 4, 8, 16])))
        files.append(dict(secs=secs, regions=[], place={}))
        a = rup(addr, 16)
        st = a
        for s in secs:
            a = rup(a, s['align']); files[f]['place'][s['id']] = (a, s['size']); a += s['size']
        used = a - st
        cap = rup(used + max(64, used // 8), 16)
        files[f]['regions'].append([st, cap])
        addr = st + cap
    osize = addr - 0x400000
    tail = max(4096, osize // 16)
    tail_start, tail_used, steps, first_step = addr, 0, 0, None
    moved, overflow_regions = [], 0
    nextid = 10 ** 6
    for hop in range(1, hops + 1):
        f = rng.choice(edit_files)
        F = files[f]
        secs = [dict(s) for s in F['secs']]
        op = rng.random()
        if op < 0.55:
            s = rng.choice(secs); s['size'] += rng.choice([4, 8, 16, 32, 64, 128, 256])
        elif op < 0.75:
            s = rng.choice(secs); s['size'] = max(8, s['size'] - rng.choice([4, 8, 16, 32, 64]))
        elif op < 0.9:
            secs.insert(rng.randrange(len(secs) + 1), dict(id=(f, nextid), size=max(8, int(rng.lognormvariate(5.3, 1.1))) // 4 * 4, align=rng.choice([4, 8, 16]))); nextid += 1
        elif len(secs) > 2:
            secs.pop(rng.randrange(len(secs)))
        old = F['place']
        new, occ, rest = {}, [], []
        for s in secs:                                  # keep
            o = old.get(s['id'])
            if o and s['size'] <= o[1] and o[0] % s['align'] == 0:
                new[s['id']] = (o[0], s['size']); occ.append((o[0], o[0] + s['size']))
            else:
                rest.append(s)
        over = []
        for s in rest:                                  # place: grow in place, first fit
            o = old.get(s['id']); got = None
            if o and o[0] % s['align'] == 0:
                reg = [r for r in F['regions'] if r[0] <= o[0] < r[0] + r[1]]
                if reg and o[0] + s['size'] <= reg[0][0] + reg[0][1] and not any(e0 < o[0] + s['size'] and o[0] < e1 for e0, e1 in occ):
                    got = o[0]
            for st, cap in ([] if got is not None else F['regions']):
                a = rup(st, s['align'])
                while a + s['size'] <= st + cap:
                    clash = [e for e in occ if e[0] < a + s['size'] and a < e[1]]
                    if not clash: got = a; break
                    a = rup(max(e[1] for e in clash), s['align'])
                if got is not None: break
            if got is None: over.append(s); continue
            new[s['id']] = (got, s['size']); occ.append((got, got + s['size']))
        if over:                                        # overflow: one new region
            need = 0
            for s in over: need = rup(need, s['align']) + s['size']
            cap = rup(need + max(64, need // 8), 16)
            st = rup(tail_start + tail_used, 16)
            if st + cap > tail_start + tail:
                steps += 1; first_step = first_step or hop
                grow = (st + cap) - (tail_start + tail)
                tail += grow + max(4096, osize // 16)
            a = st
            for s in over:
                a = rup(a, s['align']); new[s['id']] = (a, s['size']); a += s['size']
            tail_used = st + cap - tail_start
            F['regions'].append([st, cap]); overflow_regions += 1
        moved.append(sum(1 for s in secs if s['id'] in old and new[s['id']][0] != old[s['id']][0]))
        F['secs'], F['place'] = secs, new
    live = sum(s['size'] for F in files for s in F['secs'])
    span = (tail_start + tail_used) - 0x400000
    return dict(hops=hops, overflow_regions=overflow_regions, regions_max=max(len(F['regions']) for F in files),
                steps=steps, first_step=first_step, moved_mean=statistics.mean(moved), moved_max=max(moved),
                holes=1 - live / span, tail_used=tail_used, tail=tail, osize=osize)

if __name__ == '__main__':
    seed = int(sys.argv[1]) if len(sys.argv) > 1 else 1
    hops = int(sys.argv[2]) if len(sys.argv) > 2 else 200
    for nf, ef, label in ((40, [7], 'one file edited, 40 files'), (40, list(range(0, 40, 4)), 'ten files edited, 40 files'),
                          (400, [7], 'one file edited, 400 files'), (400, list(range(0, 400, 40)), 'ten files edited, 400 files')):
        rs = [run(seed * 100 + k, hops, nf, ef) for k in range(10)]
        def agg(key, fn=statistics.mean): return fn([r[key] for r in rs])
        fs = [r['first_step'] for r in rs if r['first_step']]
        print(f'{label}: {hops} hops x 10 seeds; overflow regions {agg("overflow_regions"):.1f} (max regions of one file {agg("regions_max", max)}); '
              f'growth steps {agg("steps"):.2f} (first at hop {min(fs) if fs else "-"}, in {len(fs)}/10 runs); moved definitions per hop mean {agg("moved_mean"):.2f}, '
              f'max {agg("moved_max", max)}; holes at the end {100 * agg("holes"):.1f}% of the span; tail used {agg("tail_used"):.0f} of reserve {agg("tail"):.0f} B '
              f'(output section {agg("osize"):.0f} B)')
