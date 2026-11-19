#!/usr/bin/env python3
"""WP-258: rewrite one DIE tag in every abbreviation of an ELF64 LE file's
.debug_abbrev (the probe of what makes llvm-dwarfdump --verify loop).

  retag.py IN OUT FROM TO        e.g. retag.py mortar probe 0x0b 0x1d

Every abbreviation whose tag is FROM gets TO (same ULEB width required: both
below 0x80). Prints the abbreviation offsets changed. OUT is a new file.
"""
import struct
import sys


def uleb(d, o):
    v = s = 0
    while True:
        b = d[o]
        o += 1
        v |= (b & 0x7F) << s
        s += 7
        if b < 0x80:
            return v, o


def main():
    src, dst, frm, to = sys.argv[1], sys.argv[2], int(sys.argv[3], 0), int(sys.argv[4], 0)
    assert frm < 0x80 and to < 0x80
    d = bytearray(open(src, "rb").read())
    shoff, = struct.unpack_from("<Q", d, 0x28)
    shentsize, shnum, shstrndx = struct.unpack_from("<HHH", d, 0x3A)
    sh = [struct.unpack_from("<IIQQQQIIQQ", d, shoff + i * shentsize) for i in range(shnum)]
    stro = sh[shstrndx][4]
    name = lambda n: d[stro + n:d.index(b"\0", stro + n)].decode()
    ab = [s for s in sh if name(s[0]) == ".debug_abbrev"][0]
    o, end, changed = ab[4], ab[4] + ab[5], []
    while o < end:
        code, o = uleb(d, o)
        if code == 0:
            continue  # end of one unit's abbreviation table
        t = o
        tag, o = uleb(d, o)
        o += 1  # DW_CHILDREN
        while True:
            at, o = uleb(d, o)
            form, o = uleb(d, o)
            if form == 0x21:  # DW_FORM_implicit_const
                _, o = uleb(d, o)  # (an SLEB, same width rule)
            if at == 0 and form == 0:
                break
        if tag == frm:
            d[t] = to
            changed.append(hex(t - ab[4]))
    with open(dst, "xb") as f:
        f.write(d)
    print("retagged abbreviations at", " ".join(changed) or "none")


main()
