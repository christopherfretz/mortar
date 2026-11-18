#!/usr/bin/env python3
"""classes.py: WP-249's survey of references from non-alloc sections into
non-alloc members of COMDAT groups (spec/image.md I-47, the class of
DW_MACRO_import into a dropped .debug_macro copy). Little-endian ELF,
ELFCLASS64 and 32, relocatable objects only. No third-party module.
The ELF reader follows tests/incr-design/wp228/elfdbg.py (WP-228).

  classes.py link CFG OBJ...
      the objects in link order. A COMDAT group whose signature an earlier
      object already defined is DROPPED (first one wins: GNU ld, lld, mold
      and mortar alike). For every relocation in a section that survives
      (not a member of a dropped group) whose symbol is defined in a
      NON-ALLOC member of a COMDAT group, one TSV line:
        CFG from-section to-section rtype fate kept-has-same-name same-size count
      fate: 'dropped' (the target's group is dropped: the class of WP-249),
      'kept' (the target's group is kept), 'self' (the referrer is in the
      target's own group, so the two live or die together).
      kept-has-same-name / same-size: for 'dropped', whether the kept group
      has a member of the target's name and whether its size (uncompressed
      when SHF_COMPRESSED) is the target's; '-' otherwise.

  classes.py groups OBJ...
      every group of every object: signature, flags, members (name, size).
"""
import struct
import sys
import zlib

SHT_RELA, SHT_REL, SHT_GROUP, SHT_SYMTAB = 4, 9, 17, 2
SHF_ALLOC, SHF_COMPRESSED = 0x2, 0x800
GRP_COMDAT = 1

RTYPES = {
    183: {257: 'ABS64', 258: 'ABS32', 259: 'ABS16', 260: 'PREL64', 261: 'PREL32', 262: 'PREL16'},
    62: {1: 'R_X86_64_64', 2: 'R_X86_64_PC32', 10: 'R_X86_64_32', 11: 'R_X86_64_32S',
         21: 'R_X86_64_DTPOFF32', 17: 'R_X86_64_DTPOFF64', 24: 'R_X86_64_PC64'},
    3: {1: 'R_386_32', 2: 'R_386_PC32', 32: 'R_386_TLS_LDO_32'},
}


class Sec:
    __slots__ = ('i', 'name', 'type', 'flags', 'addr', 'off', 'size', 'link', 'info', 'align', 'entsize')


class Elf:
    def __init__(self, data):
        b = self.b = data
        if b[:4] != b'\x7fELF' or b[5] != 1:
            raise ValueError('not little-endian ELF')
        self.c64 = b[4] == 2
        self.machine = struct.unpack_from('<H', b, 18)[0]
        if self.c64:
            self.shoff, = struct.unpack_from('<Q', b, 0x28)
            self.shentsize, shnum, shstrndx = struct.unpack_from('<HHH', b, 0x3a)
        else:
            self.shoff, = struct.unpack_from('<I', b, 0x20)
            self.shentsize, shnum, shstrndx = struct.unpack_from('<HHH', b, 0x2e)
        if shnum == 0 and self.shoff:
            shnum = self._shdr(0)[5]
        self.secs = []
        for i in range(shnum):
            s = Sec()
            s.i = i
            (s.name, s.type, s.flags, s.addr, s.off, s.size, s.link, s.info, s.align, s.entsize) = self._shdr(i)
            self.secs.append(s)
        if shstrndx == 0xffff:
            shstrndx = self.secs[0].link
        so = self.secs[shstrndx].off
        for s in self.secs:
            e = b.index(b'\0', so + s.name)
            s.name = b[so + s.name:e].decode('latin-1')
        self._syms = None

    def _shdr(self, i):
        o = self.shoff + i * self.shentsize
        if self.c64:
            return struct.unpack_from('<IIQQQQIIQQ', self.b, o)
        return struct.unpack_from('<IIIIIIIIII', self.b, o)

    def usize(self, s):
        """The section's size, uncompressed when SHF_COMPRESSED."""
        if s.flags & SHF_COMPRESSED:
            if self.c64:
                return struct.unpack_from('<IIQ', self.b, s.off)[2]
            return struct.unpack_from('<III', self.b, s.off)[1]
        return s.size

    def contents(self, s):
        d = self.b[s.off:s.off + s.size]
        if s.flags & SHF_COMPRESSED:
            d = zlib.decompress(d[24 if self.c64 else 12:])
        return d

    def symbols(self):
        if self._syms is not None:
            return self._syms
        out = []
        st = next((s for s in self.secs if s.type == SHT_SYMTAB), None)
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
        """[(signature, flags, [member indices])]"""
        out = []
        syms = self.symbols()
        for s in self.secs:
            if s.type != SHT_GROUP:
                continue
            sig = syms[s.info][0] if s.info < len(syms) else '?'
            words = struct.unpack_from('<%dI' % (s.size // 4), self.b, s.off)
            out.append((sig, words[0], list(words[1:])))
        return out


def rtype(elf, t):
    return RTYPES.get(elf.machine, {}).get(t, 'type%d' % t)


def link(cfg, paths):
    elves = [Elf(open(p, 'rb').read()) for p in paths]
    kept = {}  # signature -> (elf, members)
    plans = []
    for elf in elves:
        member_of = {}  # section index -> (signature, dropped?)
        for sig, flags, mem in elf.groups():
            if not flags & GRP_COMDAT:
                for m in mem:
                    member_of[m] = (sig, False, 'nocomdat')
                continue
            dropped = sig in kept
            if not dropped:
                kept[sig] = (elf, mem)
            for m in mem:
                member_of[m] = (sig, dropped, 'comdat')
        plans.append(member_of)
    rows = {}
    for elf, member_of in zip(elves, plans):
        syms = elf.symbols()
        for s in elf.secs:
            if s.type in (SHT_RELA, SHT_REL, SHT_GROUP, SHT_SYMTAB) or s.flags & SHF_ALLOC:
                continue
            here = member_of.get(s.i)
            if here and here[1]:
                continue  # the referrer itself is dropped
            for off, t, si, add in elf.relocs(s):
                if si >= len(syms):
                    continue
                shndx = syms[si][5]
                if shndx == 0 or shndx >= 0xff00:
                    continue
                tgt = elf.secs[shndx]
                if tgt.flags & SHF_ALLOC:
                    continue
                g = member_of.get(shndx)
                if not g or g[2] != 'comdat':
                    continue
                sig, dropped, _ = g
                same_name = same_size = '-'
                if here and here[0] == sig:
                    fate = 'self'
                elif dropped:
                    fate = 'dropped'
                    kelf, kmem = kept[sig]
                    cands = [kelf.secs[m] for m in kmem if kelf.secs[m].name == tgt.name]
                    same_name = 'yes' if cands else 'no'
                    if cands:
                        same_size = 'yes' if any(kelf.usize(c) == elf.usize(tgt) for c in cands) else 'no'
                else:
                    fate = 'kept'
                key = (cfg, s.name, tgt.name, rtype(elf, t), fate, same_name, same_size)
                rows[key] = rows.get(key, 0) + 1
    for key in sorted(rows):
        print('\t'.join(key + (str(rows[key]),)))
    if not rows:
        print('\t'.join((cfg, '-', '-', '-', 'none', '-', '-', '0')))


def groups(paths):
    for p in paths:
        elf = Elf(open(p, 'rb').read())
        for sig, flags, mem in elf.groups():
            ms = ' '.join('%s:%d' % (elf.secs[m].name, elf.usize(elf.secs[m])) for m in mem)
            print('%s\t%s\t%d\t%s' % (p, sig, flags, ms))


if __name__ == '__main__':
    if len(sys.argv) > 3 and sys.argv[1] == 'link':
        link(sys.argv[2], sys.argv[3:])
    elif len(sys.argv) > 2 and sys.argv[1] == 'groups':
        groups(sys.argv[2:])
    else:
        sys.exit(__doc__)
