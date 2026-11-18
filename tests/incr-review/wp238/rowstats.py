#!/usr/bin/env python3
"""rowstats.py DUMP.rows -- WP-238 review (item 4, the stored form's size).

Over a probe dump (crates/mortar_live/src/gc/probe.rs): the distinct
rows as the record counts them (kind, source, target, offset), and what
remains when (a) a section target that is NOT a split section drops its
offset (the walk reads the offset only for a split section: gc.rs
`Marks::at`), and (b) self-edges (target == source, no piece to mark) are
dropped. Ring rows and ROOT/NAMED rows are not in the dump (the record
adds them); the counts here are of kinds 0-8 as dumped (group as pairs).
"""
import collections, sys

split, rows = set(), []
for line in open(sys.argv[1], encoding='utf-8', errors='replace'):
    t = line.rstrip('\n').split('\t')
    if t[0] == 'S' and t[8] == '1':
        split.add(int(t[1]))
    elif t[0] == 'E':
        rows.append((int(t[1]), int(t[2]), t[3], int(t[4]), int(t[5]) if t[3] == 'S' else 0))
names = ['reloc', 'startstop', 'local', 'fde_section', 'fde_key', 'nonalloc_key', 'nonalloc_section', 'linkorder', 'group']
by = collections.Counter(r[1] for r in rows)
norm = set()
selfe = collections.Counter()
for (src, kind, tk, tgt, off) in rows:
    if tk == 'S' and tgt not in split:
        off = 0
        if tgt == src and kind in (2, 3, 6):
            selfe[kind] += 1
            continue
    norm.add((src, kind, tk, tgt, off))
nby = collections.Counter(r[1] for r in norm)
print('rows %d -> normalized %d (%.1f%%)' % (len(rows), len(norm), 100.0 * len(norm) / max(1, len(rows))))
for k in range(9):
    if by[k]:
        print('  %-17s %8d -> %8d  (self-edges dropped %d)' % (names[k], by[k], nby[k], selfe[k]))
