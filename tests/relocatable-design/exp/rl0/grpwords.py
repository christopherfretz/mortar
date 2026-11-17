"""grpwords.py OBJ: each SHT_GROUP section's sh_flags and its flag word (ELF64 LE)."""
import struct, sys
b = open(sys.argv[1], 'rb').read()
shoff, = struct.unpack_from('<Q', b, 0x28); shentsize, shnum, shstrndx = struct.unpack_from('<HHH', b, 0x3a)
def sh(i): return struct.unpack_from('<IIQQQQIIQQ', b, shoff + i * shentsize)
stro = sh(shstrndx)[4]
for i in range(shnum):
    name, typ, flags, addr, off, size, link, info, align, ent = sh(i)
    if typ == 17:
        n = b[stro + name: b.index(b'\0', stro + name)].decode()
        words = struct.unpack_from('<%dI' % (size // 4), b, off)
        print('  [%d] %s sh_flags=%#x flag word=%#x members=%s link=%d info=%d' % (i, n, flags, words[0], list(words[1:]), link, info))
