#!/usr/bin/env python3
"""words.py: read rule.sh's referrer words in a linked output and name
each one.

  words.py OUTPUT TAG SECTION NAMES [SECTION NAMES]...
NAMES: comma-separated case names; NAME:8 is a 64-bit word, else 32-bit.
The words follow the marker WP249REF in the output SECTION. A case Cn's
value is named against the output .debug_* sections' tags: 'kept+N' when
it is the offset of the copy whose tag starts 'Cn' (the only one left, or
the keeper's: tag 'Cnkeep'/'Cntypes'/'Cntag') plus N; 'drop+N' for the
second copy's tag ('Cndrop', 'Cnmacro', kept when the group is not
dropped); 'ones', '0', or the raw hex.
"""
import os
import sys

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
from classes import Elf  # noqa: E402


def main():
    path, tag = sys.argv[1], sys.argv[2]
    elf = Elf(open(path, 'rb').read())
    secs = {}
    for s in elf.secs:
        if s.name.startswith('.debug'):
            secs.setdefault(s.name, []).append(s)
    args = sys.argv[3:]
    out = []
    for k in range(0, len(args), 2):
        sname, names = args[k], args[k + 1].split(',')
        if sname not in secs:
            out.append('%s:absent' % sname)
            continue
        data = elf.contents(secs[sname][0])
        p = data.find(b'WP249REF')
        if p < 0:
            out.append('%s:no-marker' % sname)
            continue
        p += 8
        for n in names:
            w = 4
            if n.endswith(':8'):
                n, w = n[:-2], 8
            v = int.from_bytes(data[p:p + w], 'little')
            p += w
            case = n.split('+')[0]
            if case in ('I1', 'C9'):
                case = 'C1'
            out.append('%s=%s' % (n, name(elf, secs, case, v, w)))
    print(tag + ' ' + ' '.join(out))


def name(elf, secs, case, v, w):
    if v == (1 << (8 * w)) - 1:
        return 'ones'
    if v == 0:
        return '0'
    best = None
    for sname, ss in secs.items():
        data = elf.contents(ss[0])
        start = 0
        while True:
            q = data.find(case.encode(), start)
            if q < 0:
                break
            start = q + 1
            t = data[q:q + 8]
            if not (t[len(case):len(case) + 1].isalpha()):
                continue
            which = 'drop' if (b'drop' in t or b'macro' in t) else 'kept'
            if 0 <= v - q < 32:
                cand = '%s+%d(%s)' % (which, v - q, sname)
                if best is None or v - q == 0:
                    best = cand
    return best or hex(v)


if __name__ == '__main__':
    main()
