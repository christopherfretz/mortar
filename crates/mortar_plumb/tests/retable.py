#!/usr/bin/env python3
# retable.py ELF MAP: WP-253's one-time conversion of the checked-in
# incremental-mode fixtures (tests/symfill, tests/symnames,
# tests/gnuindex), made by a mortar before WP-228 part A, to IN-51's one
# 38-byte neutral abbreviation table (spec/incr.md IN-51).
#
# Those outputs end .debug_abbrev with the old 9-byte table
# `01 11 00 80 40 09 00 00 00` in a `dead .debug_abbrev <off> 9 neutral`
# line; their neutral units name <off>. The table hole grows to 38 bytes
# at the same offset (so every unit's abbreviation offset stays right),
# holding the four abbreviations; the sections after .debug_abbrev in file
# order (all non-allocated, which is checked: no segment maps a byte past
# .debug_abbrev's start) move by 32 bytes (29 rounded up to their largest
# alignment), as does the section header table; the map line says 26.
# Nothing else changes: the fixtures' other bytes and map lines are the
# old link's. In place. Run once per fixture; a second run refuses.
import struct
import sys

OLD = bytes([0x01, 0x11, 0x00, 0x80, 0x40, 0x09, 0x00, 0x00, 0x00])
TABLE = bytes([
    0x01, 0x11, 0x00, 0x80, 0x40, 0x09, 0x00, 0x00,
    0x02, 0x41, 0x00, 0x80, 0x40, 0x09, 0x00, 0x00,
    0x03, 0x11, 0x00, 0x73, 0x17, 0x80, 0x40, 0x09, 0x00, 0x00,
    0x04, 0x11, 0x00, 0xb3, 0x42, 0x17, 0x80, 0x40, 0x09, 0x00, 0x00,
    0x00,
])
SH = '<IIQQQQIIQQ'


def main():
    elf, mapf = sys.argv[1:3]
    b = bytearray(open(elf, 'rb').read())
    assert b[4] == 2 and b[5] == 1, 'ELF64 little endian only'
    phoff, shoff = struct.unpack_from('<QQ', b, 0x20)
    phentsize, phnum, shentsize, shnum, shstrndx = struct.unpack_from('<HHHHH', b, 0x36)
    secs = [list(struct.unpack_from(SH, b, shoff + i * shentsize)) for i in range(shnum)]
    so = secs[shstrndx][4]

    def name(s):
        return b[so + s[0]:b.index(b'\0', so + s[0])].decode()

    ai = [i for i, s in enumerate(secs) if name(s) == '.debug_abbrev'][0]
    ab = secs[ai]
    off, size = ab[4], ab[5]
    t = size - len(OLD)
    assert bytes(b[off + t:off + size]) == OLD, 'no old 9-byte table at the end'
    lines = open(mapf).read().split('\n')
    want = 'dead .debug_abbrev %x 9 neutral' % t
    assert want in lines, want
    for i in range(phnum):
        p_type, _, p_offset, _, _, p_filesz = struct.unpack_from('<IIQQQQ', b, phoff + i * phentsize)
        assert p_type == 0 or p_offset + p_filesz <= off, 'a segment maps a byte past .debug_abbrev'
    later = [s for s in secs if s[4] > off]
    align = max([s[8] for s in later] + [8])
    delta = -(-(len(TABLE) - len(OLD)) // align) * align
    tail = bytes(b[off + size:])
    new = b[:off + t] + TABLE + bytes(delta - (len(TABLE) - len(OLD))) + tail
    for s in later:
        assert s[2] & 2 == 0, name(s) + ' is allocated'
        s[4] += delta
    ab[5] = size + len(TABLE) - len(OLD)
    shoff += delta
    struct.pack_into('<Q', new, 0x28, shoff)
    for i, s in enumerate(secs):
        struct.pack_into(SH, new, shoff + i * shentsize, *s)
    open(elf, 'wb').write(new)
    lines[lines.index(want)] = 'dead .debug_abbrev %x 26 neutral' % t
    open(mapf, 'w').write('\n'.join(lines))


if __name__ == '__main__':
    main()
