#!/usr/bin/env python3
"""phdr.py [--patch-align A] LIB: print each PT_LOAD's p_align and the
congruence, and whether PT_GNU_RELRO ends on a p_align boundary; with
--patch-align, rewrite every PT_LOAD's p_align to A in place first."""
import struct, sys
args = sys.argv[1:]; patch = None
if args[0] == '--patch-align': patch = int(args[1], 0); args = args[2:]
p = args[0]; d = bytearray(open(p, 'rb').read())
phoff = struct.unpack_from('<Q', d, 32)[0]; phentsize, phnum = struct.unpack_from('<HH', d, 54)
loads = []; relro = None
for i in range(phnum):
    o = phoff + i * phentsize
    t, fl, off, va, pa, fs, ms, al = struct.unpack_from('<IIQQQQQQ', d, o)
    if t == 1 and patch: struct.pack_into('<Q', d, o + 48, patch); al = patch
    if t == 1: loads.append((off, va, al, fl))
    if t == 0x6474e552: relro = (va, ms)
if patch: open(p, 'wb').write(d)
cong = all(off % al == va % al for off, va, al, fl in loads)
maxal = max(al for _, _, al, _ in loads)
print('PT_LOADs %d p_align %s congruent=%s relro-end=%s aligned-to-max-page=%s' % (
    len(loads), ','.join(hex(a) for _, _, a, _ in loads), cong,
    hex(relro[0] + relro[1]) if relro else None, (relro[0] + relro[1]) % maxal == 0 if relro else None))
