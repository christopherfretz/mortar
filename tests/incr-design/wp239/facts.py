"""WP-239 R-4a: over a series' hN.facts (the wp239_facts probe), check
that the scan's facts per key maintained by the rule (recomputed for the
affected keys, kept for every other key) equal the facts computed afresh
at every hop, and count the hops where `update-not-implemented (slots)`
would fire (an affected key whose facts change and that a file left
unchanged mentions in both versions).

    facts.py SERIES_DIR CHANGED_BASENAME

Affected keys (IN-71, as this model has them): every key a changed,
appeared, gone or drawn-in file mentions in either version (a drawn-in
file: live in both, unchanged, its section states differ: IN-72 item
2a), and every key no live object mentions (linker-defined; IN-71c).
Also: per (file, key) mention of an unchanged file, whether its value
changed while the key's value did not (the per-key datum would miss it).
"""
import os
import re
import sys

FIELD = re.compile(r'(\w+): ([^,}]+(?:\{[^}]*\})?)')


def parse(s):
    return dict((k, v.strip()) for k, v in FIELD.findall(s.split('{', 1)[1]))


def load(path):
    files, keys, ments = {}, {}, {}
    for line in open(path, encoding='latin1'):
        p = line.rstrip('\n').split('\t')
        if p[0] == 'F':
            files[int(p[1])] = (p[2], p[3] == '1', p[4])
        elif p[0] == 'K':
            keys[(p[1], p[2])] = p[3]
        elif p[0] == 'M':
            ments.setdefault(files[int(p[1])][0], {})[(p[2], p[3])] = p[4]
    live = {lab: h for (lab, lv, h) in files.values() if lv}
    return live, keys, ments


def skc(a, b):
    """IN-26's symbol-kind-changed condition on two facts values."""
    fa, fb = parse(a), parse(b)
    ta, tb = int(fa['sym_type']), int(fb['sym_type'])
    if (ta == 10) != (tb == 10) or (ta == 6) != (tb == 6):
        return True
    if (fa['kind'] == 'Abs') != (fb['kind'] == 'Abs'):
        return True
    return (int(fa['st_other']) & ~3) != (int(fb['st_other']) & ~3)


def main():
    d, changed = sys.argv[1], sys.argv[2]
    hops = []
    n = 0
    while os.path.exists(f'{d}/h{n}.facts'):
        hops.append(load(f'{d}/h{n}.facts'))
        n += 1
    maint = dict(hops[0][1])
    fired_hops, stale_total, mention_only_total = 0, 0, 0
    for i in range(1, len(hops)):
        (lp, kp, mp), (ln, kn, mn) = hops[i - 1], hops[i]
        chg = {lab for lab in set(lp) | set(ln) if lab == changed or lab.endswith('/' + changed)}
        appeared = set(ln) - set(lp)
        gone = set(lp) - set(ln)
        drawn = {lab for lab in set(lp) & set(ln) if lab not in chg and lp[lab] != ln[lab]}
        moved = chg | appeared | gone | drawn
        affected = set()
        for lab in moved:
            affected |= set(mp.get(lab, {})) | set(mn.get(lab, {}))
        mentioned = set()
        for m in list(mp.values()) + list(mn.values()):
            mentioned |= set(m)
        if '--linker-defined-unmentioned' in sys.argv:
            affected |= (set(kp) | set(kn)) - mentioned
        for k in affected:
            if k in kn:
                maint[k] = kn[k]
            else:
                maint.pop(k, None)
        stale = sorted(k for k in kn if maint.get(k) != kn[k])
        unchanged = (set(lp) & set(ln)) - moved
        fired = []
        for k in sorted(affected):
            if k in kp and k in kn and kp[k] != kn[k]:
                refs = [u for u in unchanged if k in mp.get(u, {}) and k in mn.get(u, {})]
                if refs:
                    fired.append((k, kp[k], kn[k], skc(kp[k], kn[k]), refs[0]))
        mention_only = []
        for u in unchanged:
            for k, v in mn.get(u, {}).items():
                old = mp.get(u, {}).get(k)
                if old is not None and old != v and kp.get(k) == kn.get(k):
                    mention_only.append((u, k))
        stale_total += len(stale)
        mention_only_total += len(mention_only)
        fired_hops += bool(fired)
        print(f'hop {i}: changed {len(chg)} appeared {len(appeared)} gone {len(gone)} '
              f'drawn-in {len(drawn)} affected {len(affected)} keys {len(kn)}; '
              f'stale {len(stale)}; slots-fires {len(fired)}; mention-only {len(mention_only)}')
        for k in stale[:5]:
            print(f'  stale {k}: maintained {maint.get(k)} afresh {kn[k]}')
        for k, a, b, s, u in fired[:8]:
            print(f'  fires {k[0]}: {a} -> {b}; symbol-kind-changed {s}; referrer {u}')
        for u, k in mention_only[:5]:
            print(f'  mention-only {u} {k}')
    print(f'SUMMARY hops {len(hops) - 1} stale {stale_total} hops-where-slots-fires '
          f'{fired_hops} mention-only {mention_only_total}')


main()
