#!/usr/bin/env python3
"""WP-239 (S4-1, S4-2): merged pieces, (file, fragment) pairs and fragments
per output section of a link, from its map and its inputs.

    frags.py MAP OUTPUT [--nonalloc | --scan-nonalloc] [--cwd DIR] [--state STATE] [--per-file OUT.tsv]

Alloc split sections: the map's `piece` records outside `.eh_frame`
(image.md I-78: pieces of one fragment share an address), the output
section by the OUTPUT's section headers. Non-alloc `merged` sections
(`.debug_str`, `.comment`, ...; their pieces are not in the map): read
from the input, cut by synth.md S-9/S-10 (mortar_merge::cut), a fragment
being (output section, strings, entsize, align, bytes) (S-13). With
--state, the format-4 state's FRAGS rows per output section, compared.
--scan-nonalloc: for a link made with --strip-debug, every non-alloc
SHF_MERGE section (entsize > 0, not SHF_COMPRESSED, no relocation section:
S-6) of every live file is cut instead of the map's `merged` records.
--cwd: the directory relative input paths are read from (the link's).
Prints `out <name> pieces P pairs F frags N` lines and totals."""
import hashlib
import struct
import sys
from collections import defaultdict

sys.path.insert(0, __import__('os').path.dirname(__file__))
import elfobj  # noqa: E402


def main():
    mp, output = sys.argv[1], sys.argv[2]
    nonalloc = '--nonalloc' in sys.argv
    scan = '--scan-nonalloc' in sys.argv
    if '--cwd' in sys.argv:
        __import__('os').chdir(sys.argv[sys.argv.index('--cwd') + 1])
    state = sys.argv[sys.argv.index('--state') + 1] if '--state' in sys.argv else None
    per_file = sys.argv[sys.argv.index('--per-file') + 1] if '--per-file' in sys.argv else None
    files, pieces, merged = {}, [], []
    eh = None
    for line in open(mp):
        w = line.split()
        if not w:
            continue
        if w[0] == 'file':
            files[int(w[1])] = (w[2], int(w[4], 16) if len(w) > 4 else None)
        elif w[0] == 'piece':
            pieces.append((int(w[1]), int(w[2]), int(w[4], 16)))
        elif w[0] == 'merged':
            merged.append((int(w[1]), int(w[2]), w[3]))
        elif w[0] == 'chunk' and len(w) >= 5 and w[2] == '.eh_frame':
            eh = (int(w[3], 16), int(w[4], 16))
    outsecs = elfobj.Elf.open(output).alloc_ranges()
    stat = defaultdict(lambda: [0, set(), set()])  # out -> pieces, pairs, frags
    filepairs = defaultdict(set)
    import bisect
    starts = [r[0] for r in outsecs]
    for f, s, addr in pieces:
        if eh and eh[0] <= addr < eh[0] + eh[1]:
            continue
        i = bisect.bisect_right(starts, addr) - 1
        name = outsecs[i][2] if i >= 0 and addr < outsecs[i][0] + max(outsecs[i][1], 1) else '?'
        st = stat[name]
        st[0] += 1
        st[1].add((f, addr))
        st[2].add(addr)
        filepairs[f].add((name, addr))
    if scan:
        merged = []
        for f in sorted(files):
            path, member = files[f]
            try:
                e = elfobj.Elf.open(path, member)
            except (ValueError, OSError):
                continue
            relocated = {x['info'] for x in e.h if x['type'] in (4, 9)}
            for i, x in enumerate(e.h):
                if (x['flags'] & 0x10 and not x['flags'] & 2 and x['entsize'] and not x['flags'] & 0x800
                        and i not in relocated and x['type'] != 8):
                    merged.append((f, i, x['sname']))
    pairs_n = defaultdict(int)  # non-alloc pairs, counted per file (memory)
    if nonalloc or scan:
        cache = {}
        cur, cur_sets = None, defaultdict(set)
        merged.append((None, None, None))
        for f, s, name in merged:
            if f != cur:
                for nm, ids in cur_sets.items():
                    pairs_n[nm] += len(ids)
                cur, cur_sets = f, defaultdict(set)
            if f is None:
                break
            path, member = files[f]
            key = (path, member)
            if key not in cache:
                cache.clear()
                cache[key] = elfobj.Elf.open(path, member)
            e = cache[key]
            h = e.shdr(s)
            flags, entsize, align = h['flags'], h['entsize'], max(h['addralign'], 1)
            strings = bool(flags & 0x20)
            data = e.section_bytes(s)
            cls = (strings, entsize, align)
            st = stat[name]
            for a, b in elfobj.cut(data, strings, entsize):
                d = int.from_bytes(hashlib.blake2b(data[a:b], digest_size=8).digest(), 'little')
                st[0] += 1
                cur_sets[name].add((cls, d))
                st[2].add((cls, d))
                if per_file:
                    filepairs[f].add((name, cls, d))
    tot = [0, 0, 0]
    for name in sorted(stat):
        p, pr, fr = stat[name][0], len(stat[name][1]) + pairs_n[name], len(stat[name][2])
        tot = [tot[0] + p, tot[1] + pr, tot[2] + fr]
        print(f'out {name} pieces {p} pairs {pr} frags {fr}')
    print(f'total pieces {tot[0]} pairs {tot[1]} frags {tot[2]} files_with_pieces {len(filepairs)}')
    if state:
        t = elfobj.state_tables(state)
        print(f'state FRAGS rows {t[11][0]} FRAG_AT rows {t[40][0]}')
    if per_file:
        with open(per_file, 'w') as o:
            for f in sorted(filepairs):
                o.write(f'{f}\t{len(filepairs[f])}\n')
    return 0


sys.exit(main())
