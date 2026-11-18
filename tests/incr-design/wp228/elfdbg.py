#!/usr/bin/env python3
"""elfdbg.py: a small ELF reader (little-endian, ELFCLASS64 and 32) for
WP-228's experiments: sections, symbols, relocations, groups, and the
DWARF unit walks the hand-made placements need. No third-party module.

As a program:
  elfdbg.py survey OBJ...     per object: every .debug_* section, its size,
                              whether it is in a COMDAT group, and its
                              relocations by target class (one TSV line per
                              (object, section, target class))
"""
import struct
import sys

SHT_RELA, SHT_REL, SHT_GROUP, SHT_SYMTAB = 4, 9, 17, 2
SHF_ALLOC = 2


class Sec:
    __slots__ = ('i', 'name', 'type', 'flags', 'addr', 'off', 'size', 'link', 'info', 'align', 'entsize')


class Elf:
    def __init__(self, data):
        self.b = data
        b = data
        if b[:4] != b'\x7fELF':
            raise ValueError('not ELF')
        self.c64 = b[4] == 2
        if b[5] != 1:
            raise ValueError('big-endian')
        self.machine = struct.unpack_from('<H', b, 18)[0]
        if self.c64:
            shoff, = struct.unpack_from('<Q', b, 0x28)
            shentsize, shnum, shstrndx = struct.unpack_from('<HHH', b, 0x3a)
            self.shoff, self.shentsize = shoff, shentsize
        else:
            shoff, = struct.unpack_from('<I', b, 0x20)
            shentsize, shnum, shstrndx = struct.unpack_from('<HHH', b, 0x2e)
            self.shoff, self.shentsize = shoff, shentsize
        if shnum == 0 and shoff:
            shnum = self._shdr(0)[5] if self.c64 else self._shdr(0)[5]
        self.secs = []
        for i in range(shnum):
            s = Sec()
            s.i = i
            (nm, s.type, s.flags, s.addr, s.off, s.size, s.link, s.info, s.align, s.entsize) = self._shdr(i)
            s.name = nm
            self.secs.append(s)
        if shstrndx == 0xffff:
            shstrndx = self.secs[0].link
        so = self.secs[shstrndx].off
        for s in self.secs:
            e = b.index(b'\0', so + s.name)
            s.name = b[so + s.name:e].decode('latin-1')
        self.by_name = {}
        for s in self.secs:
            self.by_name.setdefault(s.name, s)
        self._syms = None

    def _shdr(self, i):
        o = self.shoff + i * self.shentsize
        if self.c64:
            return struct.unpack_from('<IIQQQQIIQQ', self.b, o)
        return struct.unpack_from('<IIIIIIIIII', self.b, o)

    def data(self, s):
        return self.b[s.off:s.off + s.size]

    def symbols(self):
        """[(name, value, size, info, other, shndx)] of .symtab"""
        if self._syms is not None:
            return self._syms
        st = next((s for s in self.secs if s.type == SHT_SYMTAB), None)
        out = []
        if st is not None:
            strs = self.secs[st.link]
            es = 24 if self.c64 else 16
            for k in range(st.size // es):
                o = st.off + k * es
                if self.c64:
                    nm, info, other, shndx, val, sz = struct.unpack_from('<IBBHQQ', self.b, o)
                else:
                    nm, val, sz, info, other, shndx = struct.unpack_from('<IIIBBH', self.b, o)
                e = self.b.index(b'\0', strs.off + nm)
                out.append((self.b[strs.off + nm:e].decode('latin-1'), val, sz, info, other, shndx))
        self._syms = out
        return out

    def relocs(self, target):
        """[(offset, type, symindex, addend)] of every SHT_RELA/SHT_REL
        section applying to section `target` (a Sec)."""
        out = []
        for s in self.secs:
            if s.type not in (SHT_RELA, SHT_REL) or s.info != target.i:
                continue
            if self.c64:
                es = 24 if s.type == SHT_RELA else 16
                for k in range(s.size // es):
                    o = s.off + k * es
                    off, info = struct.unpack_from('<QQ', self.b, o)
                    add = struct.unpack_from('<q', self.b, o + 16)[0] if s.type == SHT_RELA else None
                    out.append((off, info & 0xffffffff, info >> 32, add))
            else:
                es = 12 if s.type == SHT_RELA else 8
                for k in range(s.size // es):
                    o = s.off + k * es
                    off, info = struct.unpack_from('<II', self.b, o)
                    add = struct.unpack_from('<i', self.b, o + 8)[0] if s.type == SHT_RELA else None
                    out.append((off, info & 0xff, info >> 8, add))
        return out

    def groups(self):
        """{member section index: group signature}"""
        out = {}
        syms = None
        for s in self.secs:
            if s.type != SHT_GROUP:
                continue
            if syms is None:
                syms = self.symbols()
            sig = syms[s.info][0] if s.info < len(syms) else '?'
            words = struct.unpack_from('<%dI' % (s.size // 4), self.b, s.off)
            for m in words[1:]:
                out[m] = sig
        return out


def target_class(elf, sym):
    """What a relocation's symbol names, by class."""
    name, val, size, info, other, shndx = sym
    bind, typ = info >> 4, info & 0xf
    if shndx == 0:
        return 'undef-global' if bind else 'undef-local'
    if shndx >= 0xff00:
        return 'abs'
    t = elf.secs[shndx]
    if t.name.startswith('.debug_'):
        where = 'debug:' + t.name
    elif t.flags & SHF_ALLOC:
        where = 'alloc'
    else:
        where = 'nonalloc:' + t.name
    kind = 'section' if typ == 3 else ('global' if bind else 'local')
    return '%s-%s' % (kind, where)


def survey(paths):
    for p in paths:
        elf = Elf(open(p, 'rb').read())
        grp = elf.groups()
        syms = elf.symbols()
        for s in elf.secs:
            if not (s.name.startswith('.debug_') or s.name in ('.gdb_index', '.eh_frame')):
                continue
            if s.type in (SHT_RELA, SHT_REL):
                continue
            rs = elf.relocs(s)
            cls = {}
            for off, typ, si, add in rs:
                c = target_class(elf, syms[si]) if si < len(syms) else 'none'
                cls[c] = cls.get(c, 0) + 1
            g = 'group' if s.i in grp else '-'
            if not cls:
                print('%s\t%s\t%d\t%s\t-\t0' % (p, s.name, s.size, g))
            for c, n in sorted(cls.items()):
                print('%s\t%s\t%d\t%s\t%s\t%d' % (p, s.name, s.size, g, c, n))


# ---------------------------------------------------------------------------
# DWARF walks over a LINKED output (DWARF32 only; DWARF64 is refused).

def units(b, off, size):
    """[(start, end)] of the length-prefixed units of a section's bytes."""
    out = []
    p = 0
    while p + 4 <= size:
        ln, = struct.unpack_from('<I', b, off + p)
        if ln >= 0xfffffff0:
            raise ValueError('DWARF64')
        out.append((p, p + 4 + ln))
        p += 4 + ln
    return out


def uleb(b, p):
    v = 0
    s = 0
    while True:
        c = b[p]
        p += 1
        v |= (c & 0x7f) << s
        s += 7
        if c < 0x80:
            return v, p


if __name__ == '__main__':
    if len(sys.argv) > 2 and sys.argv[1] == 'survey':
        survey(sys.argv[2:])
    else:
        sys.exit(__doc__)
