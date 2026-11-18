#!/usr/bin/env python3
"""WP-255: a same-size "function body" edit of one archive member, in a copy.

    edit_member.py ARCHIVE MEMBER OUT [NTH]

Copies ARCHIVE to OUT and flips bit 0 of one 32-bit word (AArch64: the
Rd/Rt field of nearly every instruction; x86-64: one byte of an opcode
stream, so use it on AArch64 only) in the NTH (default 0) executable
SHT_PROGBITS section of MEMBER of at least 16 bytes, at an offset no
relocation of that section touches. Same size, same symbols, same
relocations: the layout replays. Prints the section's name and the offset.
"""
import struct
import sys

arch, member, out = sys.argv[1], sys.argv[2].encode(), sys.argv[3]
nth = int(sys.argv[4]) if len(sys.argv) > 4 else 0
data = bytearray(open(arch, "rb").read())
assert data[:8] == b"!<arch>\n"
at, names, found = 8, b"", None
while at < len(data):
    h = data[at:at + 60]
    name, size = h[:16].rstrip(), int(h[48:58])
    body = at + 60
    if name == b"//":
        names = bytes(data[body:body + size])
    elif name.startswith(b"/") and name[1:].isdigit():
        off = int(name[1:])
        name = names[off:names.index(b"/\n", off)]
    else:
        name = name.rstrip(b"/")
    if name == member:
        found = (body, size)
        break
    at = body + size + (size & 1)
assert found, "member not found"
base, _ = found
e = data[base:]
assert e[:4] == b"\x7fELF" and e[4] == 2
shoff, = struct.unpack_from("<Q", e, 0x28)
shentsize, shnum, shstrndx = struct.unpack_from("<HHH", e, 0x3A)
sh = [struct.unpack_from("<IIQQQQIIQQ", e, shoff + i * shentsize) for i in range(shnum)]
strtab = sh[shstrndx]
def sname(i):
    o = strtab[4] + sh[i][0]
    return bytes(e[o:e.index(b"\0", o)]).decode()
relocs = {}
for s in sh:
    if s[1] == 4:  # SHT_RELA
        offs = relocs.setdefault(s[7], set())
        for k in range(s[5] // 24):
            r_off, = struct.unpack_from("<Q", e, s[4] + k * 24)
            offs.add(r_off)
k = 0
for i, s in enumerate(sh):
    if s[1] == 1 and s[2] & 4 and s[5] >= 16:
        busy = relocs.get(i, set())
        for w in range(0, s[5] - 4, 4):
            if not any(w - 7 <= r <= w + 3 for r in busy):
                if k == nth:
                    data[base + s[4] + w] ^= 1
                    open(out, "wb").write(data)
                    print(f"edited {sname(i)} +{w:#x} (member at {base:#x})")
                    sys.exit(0)
                k += 1
                break
sys.exit("no editable section")
