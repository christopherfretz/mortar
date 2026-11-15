"""patch_vis.py FILE NAME VIS: set st_other visibility of NAME's .dynsym entry (ELF64 LE)."""
import struct, sys
path, name, vis = sys.argv[1], sys.argv[2].encode(), int(sys.argv[3])
b = bytearray(open(path, 'rb').read())
shoff, = struct.unpack_from('<Q', b, 0x28)
shentsize, shnum = struct.unpack_from('<HH', b, 0x3a)
secs = [struct.unpack_from('<IIQQQQIIQQ', b, shoff + i * shentsize) for i in range(shnum)]
done = 0
for s in secs:
    if s[1] != 11:  # SHT_DYNSYM
        continue
    strtab = secs[s[6]]
    for off in range(s[4], s[4] + s[5], 24):
        st_name, = struct.unpack_from('<I', b, off)
        o = strtab[4] + st_name
        if bytes(b[o:o + len(name) + 1]) == name + b'\0':
            b[off + 5] = (b[off + 5] & ~3) | vis
            done += 1
open(path, 'wb').write(b)
print('patch_vis', path.rsplit('/', 1)[-1], name.decode(), 'vis', vis, 'entries', done)
