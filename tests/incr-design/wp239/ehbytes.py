#!/usr/bin/env python3
"""WP-239 (S4-3): walk an output's `.eh_frame` BYTES (no input read) and
say what U-6 needs to move a file's range without reading the file.

    ehbytes.py OUTPUT RANGES.tsv      (RANGES.tsv from ehmap.py --per-file)

For every record: its range (file); for an FDE whether its CIE is in the
same range (IN-34a: no FDE points into another file's range); every
pointer field's encoding (CIE 'R' FDE encoding, 'P' personality, 'L'
LSDA) and whether a walker of the output alone finds it. Prints
`check ...` lines and an encoding census."""
import struct
import sys
from collections import Counter
from bisect import bisect_right


def sections(data):
    if data[:4] != b'\x7fELF' or data[4] != 2:
        raise SystemExit('ELF64 only')
    shoff, = struct.unpack_from('<Q', data, 0x28)
    shentsize, shnum, shstrndx = struct.unpack_from('<HHH', data, 0x3a)
    hdrs = [struct.unpack_from('<IIQQQQIIQQ', data, shoff + i * shentsize) for i in range(shnum)]
    stro = hdrs[shstrndx][4]
    out = {}
    for h in hdrs:
        name = data[stro + h[0]:data.index(b'\0', stro + h[0])].decode()
        out[name] = h
    return out


def uleb(b, i):
    r = s = 0
    while True:
        x = b[i]
        i += 1
        r |= (x & 0x7f) << s
        s += 7
        if x < 0x80:
            return r, i


def sleb(b, i):
    r = s = 0
    while True:
        x = b[i]
        i += 1
        r |= (x & 0x7f) << s
        s += 7
        if x < 0x80:
            if x & 0x40:
                r -= 1 << s
            return r, i


SIZE = {0x00: 8, 0x02: 2, 0x03: 4, 0x04: 8, 0x0a: 2, 0x0b: 4, 0x0c: 8}


def enc_size(e):
    return SIZE.get(e & 0x0f)


def main():
    data = open(sys.argv[1], 'rb').read()
    secs = sections(data)
    h = secs['.eh_frame']
    addr, off, size = h[3], h[4], h[5]
    eh = data[off:off + size]
    ranges = []
    for line in open(sys.argv[2]):
        f, start, b, k = line.split('\t')[:4]
        ranges.append((int(start), int(b), int(f)))
    starts = [r[0] for r in ranges]

    def file_at(o):
        i = bisect_right(starts, o) - 1
        if i >= 0 and o < ranges[i][0] + ranges[i][1]:
            return ranges[i][2]
        return None

    cies = {}
    enc = Counter()
    aug = Counter()
    cross = fdes = nrec = 0
    unknown = []
    i = 0
    while i + 4 <= len(eh):
        length, = struct.unpack_from('<I', eh, i)
        if length == 0:
            break
        if length == 0xffffffff:
            unknown.append((i, '64-bit length'))
            break
        body = i + 4
        cid, = struct.unpack_from('<I', eh, body)
        nrec += 1
        if cid == 0:
            ver = eh[body + 4]
            j = body + 5
            a = eh[j:eh.index(b'\0', j)].decode('latin1')
            j += len(a) + 1
            aug[a] += 1
            _, j = uleb(eh, j)
            _, j = sleb(eh, j)
            if ver == 1:
                j += 1
            else:
                _, j = uleb(eh, j)
            fenc, lenc = 0, None
            if a.startswith('z'):
                _, j = uleb(eh, j)
                for ch in a[1:]:
                    if ch == 'R':
                        fenc = eh[j]
                        j += 1
                    elif ch == 'P':
                        penc = eh[j]
                        enc[f'personality 0x{penc:02x}'] += 1
                        n = enc_size(penc)
                        if n is None:
                            unknown.append((i, f'personality enc {penc:#x}'))
                            break
                        j += 1 + n
                    elif ch == 'L':
                        lenc = eh[j]
                        j += 1
                    elif ch in 'SB':
                        pass
                    else:
                        unknown.append((i, f'aug {a}'))
                        break
            cies[i] = (fenc, lenc, a)
            enc[f'fde 0x{fenc:02x}'] += 1
            if lenc is not None:
                enc[f'lsda 0x{lenc:02x}'] += 1
        else:
            fdes += 1
            cie = body - cid
            if cie not in cies:
                unknown.append((i, 'cie not seen'))
            elif file_at(cie) != file_at(i):
                cross += 1
        i = body + length
    print(f'records {nrec} fdes {fdes} cies {len(cies)} bytes_walked {i + 4} section {size}')
    print(f'check cie_same_range {"ok" if cross == 0 else "FAIL"} fdes_with_cie_in_other_range={cross}')
    print(f'check walker_knows_every_field {"ok" if not unknown else "FAIL"} {unknown[:5]}')
    for k, v in sorted(enc.items()):
        print(f'encoding {k} cies={v}')
    for k, v in sorted(aug.items()):
        print(f'augmentation "{k}" cies={v}')
    return 0


sys.exit(main())
