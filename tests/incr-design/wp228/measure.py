#!/usr/bin/env python3
"""measure.py (--map MAP | --list FILE) [--out OUTPUT]: WP-228 step 1,
item 5. Run from the link's working directory.

The linked inputs come from a mortar map (`file N path [member off]`) or
from a list of object paths (one per line: every object of a build, an
upper bound of what a link takes). Prints:

  per debug section: input bytes, contributions (file, section) and the
    allotment rounding the class rule (IN-50, mortar_layout::slack) adds;
    with --out and --map also the output section's size and the map's
    `dead ... neutral` bytes (the rounding as linked)
  .debug_str / .debug_line_str: input bytes, strings, distinct strings and
    their bytes (what the merged section holds), the size of a stored
    hash -> offset index (12 bytes a distinct string: an 8-byte hash and a
    4-byte offset), per-file string-set bytes (mean, p50, p90, max) against
    the tail reserve (size/32)
  relocations of debug sections by target class; the cross-file ones (a
    debug word naming a global another linked file defines): total, and per
    defining file (how many unchanged-file sites move when that file's
    sections move): mean, p50, p90, max and the top files
"""
import os
import struct
import sys

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
from elfdbg import Elf, SHT_RELA, SHT_REL  # noqa: E402

CLASS_MIN = 32
NEUTRAL = {'.debug_info': (15, 1), '.debug_abbrev': (1, 1), '.debug_line': (32, 1),
           '.debug_rnglists': (12, 1), '.debug_str_offsets': (8, 4), '.debug_ranges': (16, 1)}


def class_at_least(n):
    if n <= CLASS_MIN:
        return CLASS_MIN
    k = max(n.bit_length() - 4, 0)
    step = 1 << k
    return -(-n // step) * step


def allotment(size, form):
    if size == 0:
        return 0
    mn, g = form
    c = class_at_least(size)
    while True:
        pad = c - size
        if pad == 0 or (pad >= mn and pad % g == 0):
            return pad
        n = class_at_least(c + 1)
        if n <= c:
            return 0
        c = n


def read_member(path, off):
    with open(path, 'rb') as f:
        f.seek(off)
        h = f.read(60)
        size = int(h[48:58].decode().strip())
        return f.read(size)


def inputs_from_map(mp):
    out = []
    for ln in open(mp):
        w = ln.split()
        if not w or w[0] != 'file':
            continue
        if len(w) >= 5:
            out.append((int(w[1]), w[2] + '(' + w[3] + ')', (w[2], int(w[4], 16))))
        else:
            out.append((int(w[1]), w[2], (w[2], None)))
    return out


def pct(v, p):
    if not v:
        return 0
    v = sorted(v)
    return v[min(len(v) - 1, int(len(v) * p))]


def main():
    a = sys.argv[1:]
    mp = a[a.index('--map') + 1] if '--map' in a else None
    lst = a[a.index('--list') + 1] if '--list' in a else None
    outp = a[a.index('--out') + 1] if '--out' in a else None
    if mp:
        files = inputs_from_map(mp)
    else:
        files = [(i, p.strip(), (p.strip(), None)) for i, p in enumerate(open(lst)) if p.strip()]
    sec_in = {}       # name -> [bytes, contributions, rounding]
    strs = {}         # name -> [bytes, count, set of hashes, distinct bytes]
    per_file_str = []
    defs = {}         # global name -> file id
    refs = []         # (file id, global name) of debug relocations
    rel_cls = {}
    nfiles = 0
    for fid, label, (path, off) in files:
        try:
            data = read_member(path, off) if off is not None else open(path, 'rb').read()
            elf = Elf(data)
        except (OSError, ValueError):
            continue
        if elf.b[16] != 1:  # ET_REL only
            continue
        nfiles += 1
        syms = elf.symbols()
        for (nm, val, sz, info, other, shndx) in syms:
            if shndx != 0 and info >> 4 in (1, 2) and nm:
                defs.setdefault(nm, fid)
        runs = {}
        fstr = 0
        for s in elf.secs:
            if not s.name.startswith('.debug_') or s.type in (SHT_RELA, SHT_REL) or s.type == 8:
                continue
            runs[s.name] = runs.get(s.name, 0) + s.size
            if s.name in ('.debug_str', '.debug_line_str'):
                d = elf.data(s)
                st = strs.setdefault(s.name, [0, 0, set(), 0])
                st[0] += len(d)
                fstr += len(d)
                for piece in d.split(b'\0')[:-1]:
                    st[1] += 1
                    h = hash(piece)
                    if h not in st[2]:
                        st[2].add(h)
                        st[3] += len(piece) + 1
            for (roff, typ, si, add) in elf.relocs(s):
                if si >= len(syms):
                    continue
                nm, val, sz, info, other, shndx = syms[si]
                typ_ = info & 0xf
                bind = info >> 4
                if typ_ == 3:
                    c = 'section'
                elif bind == 0:
                    c = 'local'
                elif shndx != 0:
                    c = 'global-own'
                else:
                    c = 'global-undef'
                    refs.append((fid, nm))
                rel_cls[(s.name, c)] = rel_cls.get((s.name, c), 0) + 1
        per_file_str.append(fstr)
        for name, size in runs.items():
            e = sec_in.setdefault(name, [0, 0, 0])
            e[0] += size
            e[1] += 1
            if name in NEUTRAL:
                e[2] += allotment(size, NEUTRAL[name])
    print('files %d' % nfiles)
    print('# section input-bytes contributions allotment-rounding(class rule)')
    tot = 0
    rnd = 0
    for k in sorted(sec_in):
        print('in %s %d %d %d' % (k, *sec_in[k]))
        tot += sec_in[k][0]
        rnd += sec_in[k][2]
    print('in total %d rounding %d (%.2f%%)' % (tot, rnd, 100.0 * rnd / max(tot, 1)))
    if outp:
        elf = Elf(open(outp, 'rb').read())
        dt = 0
        for s in elf.secs:
            if s.name.startswith('.debug_'):
                print('out %s %d' % (s.name, s.size))
                dt += s.size
        print('out total %d' % dt)
    if mp:
        dn = {}
        for ln in open(mp):
            w = ln.split()
            if w and w[0] == 'dead' and w[1].startswith('.debug_'):
                dn[(w[1], w[-1])] = dn.get((w[1], w[-1]), 0) + int(w[3], 16)
        for k in sorted(dn):
            print('map-dead %s %s %d' % (k[0], k[1], dn[k]))
    for k, st in sorted(strs.items()):
        print('str %s input-bytes %d strings %d distinct %d distinct-bytes %d index-bytes(12/str) %d'
              % (k, st[0], st[1], len(st[2]), st[3], 12 * len(st[2])))
    print('str per-file bytes mean %d p50 %d p90 %d max %d' % (
        sum(per_file_str) // max(len(per_file_str), 1), pct(per_file_str, .5), pct(per_file_str, .9),
        max(per_file_str or [0])))
    for k in sorted(rel_cls):
        print('rel %s %s %d' % (k[0], k[1], rel_cls[k]))
    cross = {}
    tot_cross = 0
    for fid, nm in refs:
        d = defs.get(nm)
        if d is not None and d != fid:
            cross[d] = cross.get(d, 0) + 1
            tot_cross += 1
    v = list(cross.values()) + [0] * (nfiles - len(cross))
    print('cross-file debug refs %d; defining files with any %d of %d; per file mean %.1f p50 %d p90 %d max %d'
          % (tot_cross, len(cross), nfiles, tot_cross / max(nfiles, 1), pct(v, .5), pct(v, .9), max(v or [0])))
    label = {fid: lb for fid, lb, _ in files}
    for d, n in sorted(cross.items(), key=lambda x: -x[1])[:8]:
        print('cross-top %d %s' % (n, label.get(d, d)))


if __name__ == '__main__':
    main()
