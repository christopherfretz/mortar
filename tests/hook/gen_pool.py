#!/usr/bin/env python3
"""WP-077: one module of the G / cost / J sets (after prototype 2's gentab2.py).

usage: gen_pool.py OUT N SEED [bad]
  OUT.c    defines exported data mk0..mkN-1 (the shared pool: every module of a
           set defines them with the same bodies) and mpSEED_0..mpSEED_{N/4-1}
           (private), and refers to every one of them (an array of their
           addresses: one dynamic relocation each, as a vague definition a
           unit emits is one it uses), the first 64 also through the GOT
  OUT.meta the test producer's kind 2 rows (bodies: mk i -> i, mp -> 10^6 + i;
           'bad' changes mk7's body)
mortar writes the table (--odr-table=exported) from the rows."""
import sys

out, n, seed = sys.argv[1], int(sys.argv[2]), sys.argv[3]
bad = len(sys.argv) > 4 and sys.argv[4] == 'bad'
names, bodies = [], []
for i in range(n):
    names.append('mk%d' % i)
    bodies.append(i + (777777 if bad and i == 7 else 0))
for i in range(n // 4):
    names.append('mp%s_%d' % (seed, i))
    bodies.append(1000000 + i)
with open(out + '.c', 'w') as f:
    for nm in names:
        f.write('__attribute__((visibility("default"))) const char %s[1] = {0};\n' % nm)
    f.write('__attribute__((visibility("default"))) const void* const mason_refs_%s[] = {\n' % seed)
    f.write(',\n'.join(names))
    f.write('};\n')
    f.write('__attribute__((visibility("default"))) int mason_touch_%s(void) { return 0' % seed)
    for nm in names[:64]:
        f.write(' + %s[0]' % nm)
    f.write('; }\n')


def h(b):
    return '%016x%016x' % ((b * 2654435761 + 40503) & 0x7fffffffffffffff, b + 4096)


with open(out + '.meta', 'w') as f:
    f.write('summary name=lang value=c++\n')
    for nm, b in zip(names, bodies):
        f.write('definition anchor=%s hash=%s class=1 flags=1\n' % (nm, h(b)))
