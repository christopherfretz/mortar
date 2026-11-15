# elfdyn.py: minimal independent ELF64/ELF32 little-endian reader for the dynamic tables
# (written for this review; shares nothing with the author's dyninfo.py).
import struct
class Elf:
    def __init__(self, path):
        b = self.b = open(path, 'rb').read()
        self.c64 = b[4] == 2; e = '<'
        if self.c64:
            (self.type, self.mach, _, _, self.phoff, self.shoff, _, _, self.phentsize, self.phnum,
             self.shentsize, self.shnum, self.shstrndx) = struct.unpack_from('<HHIQQQIHHHHHH', b, 16)
        else:
            (self.type, self.mach, _, _, self.phoff, self.shoff, _, _, self.phentsize, self.phnum,
             self.shentsize, self.shnum, self.shstrndx) = struct.unpack_from('<HHIIIIIHHHHHH', b, 16)
        self.secs = []
        for i in range(self.shnum):
            o = self.shoff + i * self.shentsize
            if self.c64: f = struct.unpack_from('<IIQQQQIIQQ', b, o)
            else: f = struct.unpack_from('<IIIIIIIIII', b, o)
            self.secs.append(dict(name=f[0], type=f[1], flags=f[2], addr=f[3], off=f[4], size=f[5], link=f[6], info=f[7], align=f[8], entsize=f[9]))
        sh = self.secs[self.shstrndx]
        for s in self.secs: s['name'] = self.cstr(sh['off'] + s['name'])
        self.phdrs = []
        for i in range(self.phnum):
            o = self.phoff + i * self.phentsize
            if self.c64: t, fl, off, va, pa, fs, ms, al = struct.unpack_from('<IIQQQQQQ', b, o)
            else: t, off, va, pa, fs, ms, fl, al = struct.unpack_from('<IIIIIIII', b, o)
            self.phdrs.append(dict(type=t, flags=fl, off=off, vaddr=va, filesz=fs, memsz=ms, align=al))
    def cstr(self, o): return self.b[o:self.b.index(0, o)].decode()
    def sec(self, name):
        for s in self.secs:
            if s['name'] == name: return s
    def sec_by_type(self, t): return [s for s in self.secs if s['type'] == t]
    def dynsyms(self):
        ds = self.sec('.dynsym'); st = self.secs[ds['link']]; out = []
        ent = 24 if self.c64 else 16
        for i in range(ds['size'] // ent):
            o = ds['off'] + i * ent
            if self.c64: nm, info, oth, shn, val, sz = struct.unpack_from('<IBBHQQ', self.b, o)
            else: nm, val, sz, info, oth, shn = struct.unpack_from('<IIIBBH', self.b, o)
            out.append(dict(name=self.cstr(st['off'] + nm), nameoff=nm, info=info, other=oth, shndx=shn, value=val, size=sz))
        return out
    def dynamic(self):
        d = self.sec('.dynamic'); ent = 16 if self.c64 else 8; out = []
        for i in range(d['size'] // ent):
            t, v = struct.unpack_from('<qQ' if self.c64 else '<iI', self.b, d['off'] + i * ent)
            out.append((t, v))
        return out
    def gnuhash(self):
        g = self.sec('.gnu.hash'); o = g['off']; W = 64 if self.c64 else 32
        nb, so, mw, sh = struct.unpack_from('<IIII', self.b, o); o += 16
        bloom = struct.unpack_from('<%d%s' % (mw, 'Q' if self.c64 else 'I'), self.b, o); o += mw * W // 8
        buckets = struct.unpack_from('<%dI' % nb, self.b, o); o += 4 * nb
        n = (g['size'] - (o - g['off'])) // 4
        chain = struct.unpack_from('<%dI' % n, self.b, o)
        return dict(nbuckets=nb, symoffset=so, maskwords=mw, shift=sh, bloom=bloom, buckets=buckets, chain=chain)
def gnu_hash(s):
    h = 5381
    for c in s.encode(): h = (h * 33 + c) & 0xffffffff
    return h
