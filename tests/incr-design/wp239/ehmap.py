#!/usr/bin/env python3
"""WP-239 (S4-3): the `.eh_frame` of an incremental-mode output, read from
its map (image.md I-78: `.eh_frame`'s pieces are its kept records, each at
its output address with its output size).

    ehmap.py MAP [--per-file OUT.tsv]

Checks, and prints as `check <name> <ok|FAIL> <detail>`:
  contiguous   the records tile the chunk from its start, no gap anywhere
               (so no per-file pad: U-4's claim)
  grouped      each file's records are one run (a file range)
  order        the runs are in file-id (load) order
  terminator   chunk size = records + 4
Prints the per-file range sizes summary; --per-file writes `file bytes
records` lines (the file's range)."""
import sys
from collections import OrderedDict


def read(path):
    files, chunk, eh_secs, pieces = {}, None, set(), []
    rows = []
    with open(path) as f:
        for line in f:
            w = line.split()
            if not w:
                continue
            if w[0] == 'file':
                files[int(w[1])] = ' '.join(w[2:])
            elif w[0] == 'chunk' and len(w) >= 5 and w[1] == 'synthetic' and w[2] == '.eh_frame':
                chunk = (int(w[3], 16), int(w[4], 16))
            elif w[0] == 'piece':
                rows.append((int(w[1]), int(w[2]), int(w[3], 16), int(w[4], 16), int(w[5], 16)))
    return files, chunk, rows


def main():
    path = sys.argv[1]
    out = sys.argv[sys.argv.index('--per-file') + 1] if '--per-file' in sys.argv else None
    files, chunk, rows = read(path)
    if chunk is None:
        print('check chunk FAIL no .eh_frame chunk')
        return 1
    lo, size = chunk
    eh = sorted((r for r in rows if lo <= r[3] < lo + size), key=lambda r: (r[3], r[0], r[1], r[2]))
    at, gaps = lo, []
    runs = []  # (file, start, bytes, records)
    for f, s, off, addr, sz in eh:
        if addr != at:
            gaps.append((at - lo, addr - at, f))
        at = addr + sz
        if runs and runs[-1][0] == f:
            runs[-1][2] += sz
            runs[-1][3] += 1
        else:
            runs.append([f, addr - lo, sz, 1])
    term = lo + size - at
    seen = [r[0] for r in runs]
    print(f'check contiguous {"ok" if not gaps else "FAIL"} gaps={len(gaps)} first={gaps[:3]}')
    print(f'check grouped {"ok" if len(seen) == len(set(seen)) else "FAIL"} runs={len(runs)} files={len(set(seen))}')
    print(f'check order {"ok" if seen == sorted(seen) else "FAIL"}')
    print(f'check terminator {"ok" if term == 4 else "FAIL"} tail={term}')
    sizes = sorted(r[2] for r in runs)
    n = len(sizes)
    if n:
        print(f'files_with_records {n} records {len(eh)} bytes {sum(sizes)} chunk {size} '
              f'median_range {sizes[n // 2]} p90 {sizes[int(n * 0.9)]} max {sizes[-1]}')
    if out:
        with open(out, 'w') as o:
            for f, start, b, k in runs:
                o.write(f'{f}\t{start}\t{b}\t{k}\t{files.get(f, "?")}\n')
    return 0


sys.exit(main())
