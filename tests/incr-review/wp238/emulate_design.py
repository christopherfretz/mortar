#!/usr/bin/env python3
"""emulate_design.py OLD.rows NEW.rows [--roots a,b,...] -- WP-238 review.

A second, design-faithful emulation of WP-238 step 1's update walk over
two probe dumps (crates/mortar_live/src/gc/probe.rs), written for the
adversarial review. It differs from the author's emulate.py where the
record's design differs from the full link's own mark:

* Rooted keys are the design's (record §2: `OptionKeys` -- entry, `-u`,
  `--require-defined`; `OptionKeys::roots` adds `--defsym` bases), given
  by name with --roots (default `_start`), NOT the dump's `R` lines and
  rooted flags (which are the full link's: they include `-init`/`-fini`).
* A key is resolved by its row kind as IN-29's table says: `reloc`,
  `fde_key`, `nonalloc_key` ("as reloc") take the key's definition, and a
  linker-defined (`Internal`) key gives nothing; only `startstop` rows
  expand to the sections named N (record §2, kind 1's reader).
* The output's live set is modelled as an update produces it (R-3, R-4):
  for changed and election-drawn files the new walk's marks; for every
  other file the STORED state (OLD's live bit) unless the two walks (old:
  OLD's rows and OLD's roots by the design; new: as above) differ on the
  section, which then takes the new walk's mark. Compared with NEW.
* Also printed: the old walk against OLD's stored live set (review item
  4 (c): would comparing them catch a wrong design?).

Last line: `AGREE ...` or `DISAGREE ...`.
"""
import collections, importlib.util, os, sys

spec = importlib.util.spec_from_file_location(
    'emulate', os.path.join(os.path.dirname(os.path.abspath(__file__)),
                            '..', '..', 'incr-design', 'wp238', 'emulate.py'))
E = importlib.util.module_from_spec(spec)
spec.loader.exec_module(E)

KEY_AS_RELOC = {0, 4, 5}


def walk(D, rows, rootbits, rooted):
    by_src = collections.defaultdict(list)
    for rs in rows.values():
        for (src, kind, to) in rs:
            by_src[src].append((kind, to))
    live, frags, whole = set(), set(), set()
    stack = []

    def at(s, off):
        if s is None:
            return
        inf = D.info.get(s)
        if inf is None or not inf[1]:
            return
        if s not in live:
            live.add(s)
            stack.append(s)
        if not inf[4]:
            return
        ps = D.pieces.get(s, [])
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

    def key(name, kind):
        t = D.target.get(name)
        if t is None:
            return
        if t[0] == 'S':
            at(t[1], t[2])
        elif kind not in KEY_AS_RELOC:      # startstop (1) or a root
            for s in t[1]:
                at(s, -1)

    for s in sorted(rootbits):
        at(s, -1)
    for name in sorted(rooted):
        key(name, 1 if name.startswith(('__start_', '__stop_')) else 0)
    while stack:
        s = stack.pop()
        for kind, to in by_src.get(s, ()):
            if to[0] == 'K':
                key(to[1], kind)
            else:
                at(to[1], to[2])
    return live, frags


def rootbits_of(D, files):
    out = set()
    for fi in files:
        for si in D.file_secs(fi):
            inf = D.info[si]
            if inf[1] and inf[3] & 2:
                out.add(si)
    return out


def main():
    args = sys.argv[1:]
    roots = ['_start']
    if '--roots' in args:
        i = args.index('--roots')
        roots = [r for r in args[i + 1].split(',') if r]
        del args[i:i + 2]
    O, N = E.Dump(args[0]), E.Dump(args[1])
    changed, drawn, same = [], [], []
    for fi in N.hash:
        if O.hash.get(fi) != N.hash[fi] or N.hash[fi] == '0' * 16:
            changed.append(fi)
            continue
        okept = {si[1] for si in O.file_secs(fi) if O.info[si][1]}
        nkept = {si[1] for si in N.file_secs(fi) if N.info[si][1]}
        (drawn if okept != nkept else same).append(fi)
    rooted = set(roots)
    # Old walk: the stored rows and roots of every file of OLD.
    o_live, _ = walk(O, {fi: O.rows.get(fi, set()) for fi in O.hash}, rootbits_of(O, O.hash), rooted)
    o_stored = {si for si, inf in O.info.items() if inf[2]}
    # New walk: unchanged files' stored rows, changed/drawn files' fresh.
    rows, rb = {}, set()
    for fi in N.hash:
        src = N if fi in changed or fi in drawn else O
        rows[fi] = src.rows.get(fi, set())
        rb |= rootbits_of(src, [fi])
    n_live, n_frags = walk(N, rows, rb, rooted)
    want = {si for si, inf in N.info.items() if inf[2]}
    # The update's output (R-3/R-4): stored state unless the walks differ.
    final = set()
    for si, inf in N.info.items():
        if not inf[1]:
            continue
        if si[0] in changed or si[0] in drawn:
            if si in n_live:
                final.add(si)
        elif (si in o_live) != (si in n_live):
            if si in n_live:
                final.add(si)
        elif si in o_stored:
            final.add(si)

    def names(ss):
        return sorted('%s:%d:%s' % (s[0][0].split('/')[-1], s[1], N.info.get(s, O.info.get(s, ('?',)))[0])
                      for s in ss)[:6]
    w_miss, w_extra = want - n_live, n_live - want
    f_miss, f_extra = want - final, final - want
    ow_miss, ow_extra = o_stored - o_live, o_live - o_stored
    print('changed %d drawn %d; old walk vs stored live: miss %d extra %d %s;' % (
        len(changed), len(drawn), len(ow_miss), len(ow_extra), names(ow_miss | ow_extra)))
    print('new walk vs NEW live: miss %d %s extra %d %s;' % (
        len(w_miss), names(w_miss), len(w_extra), names(w_extra)))
    ok = not (f_miss or f_extra)
    print('%s update output vs NEW live: miss %d %s extra %d %s' % (
        'AGREE' if ok else 'DISAGREE', len(f_miss), names(f_miss), len(f_extra), names(f_extra)))


if __name__ == '__main__':
    main()
