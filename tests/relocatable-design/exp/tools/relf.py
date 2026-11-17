"""relf.py: a minimal little-endian ELF reader (ELF32/ELF64, ET_REL first) for the
WP-154 experiments: sections, groups, symbols, relocations with the addend
(RELA: the record; REL: the field, by i386 type width). Written for this
design; shares nothing with mortar's code."""
import struct

SHT_PROGBITS, SHT_SYMTAB, SHT_STRTAB, SHT_RELA, SHT_NOBITS, SHT_REL = 1, 2, 3, 4, 8, 9
SHT_GROUP, SHT_SYMTAB_SHNDX = 17, 18
SHF_WRITE, SHF_ALLOC, SHF_EXEC, SHF_MERGE, SHF_STRINGS = 1, 2, 4, 0x10, 0x20
SHF_INFO_LINK, SHF_LINK_ORDER, SHF_GROUP, SHF_TLS = 0x40, 0x80, 0x200, 0x400
SHF_COMPRESSED, SHF_GNU_RETAIN, SHF_EXCLUDE = 0x800, 0x200000, 0x80000000
EM_386, EM_X86_64, EM_AARCH64 = 3, 62, 183
SHN_UNDEF, SHN_ABS, SHN_COMMON, SHN_XINDEX = 0, 0xFFF1, 0xFFF2, 0xFFFF

# i386 REL field width by type (bytes); every other type is 4.
I386_WIDTH = {20: 2, 21: 2, 22: 1, 23: 1, 0: 0}


class Sec:
    __slots__ = ("idx", "name", "type", "flags", "addr", "off", "size", "link", "info", "align", "entsize")


class Sym:
    __slots__ = ("idx", "name", "value", "size", "bind", "type", "vis", "shndx", "other")


class Rel:
    __slots__ = ("off", "type", "sym", "addend", "explicit")


class Elf:
    def __init__(self, data, name="?"):
        self.data, self.name = data, name
        if data[:4] != b"\x7fELF" or data[5] != 1:
            raise ValueError(name + ": not a little-endian ELF file")
        self.c64 = data[4] == 2
        if self.c64:
            (self.etype, self.mach, _, self.entry, self.phoff, shoff, self.eflags, _, _, self.phnum,
             shentsize, shnum, shstrndx) = struct.unpack_from("<HHIQQQIHHHHHH", data, 16)
        else:
            (self.etype, self.mach, _, self.entry, self.phoff, shoff, self.eflags, _, _, self.phnum,
             shentsize, shnum, shstrndx) = struct.unpack_from("<HHIIIIIHHHHHH", data, 16)
        if shoff and shnum == 0:
            shnum = struct.unpack_from("<Q" if self.c64 else "<I", data, shoff + (32 if self.c64 else 20))[0]
        self.secs = []
        for i in range(shnum):
            o = shoff + i * shentsize
            s = Sec()
            s.idx = i
            if self.c64:
                (nm, s.type, s.flags, s.addr, s.off, s.size, s.link, s.info, s.align, s.entsize) = struct.unpack_from("<IIQQQQIIQQ", data, o)
            else:
                (nm, s.type, s.flags, s.addr, s.off, s.size, s.link, s.info, s.align, s.entsize) = struct.unpack_from("<IIIIIIIIII", data, o)
            s.name = nm
            self.secs.append(s)
        if shstrndx == SHN_XINDEX:
            shstrndx = self.secs[0].link
        strs = self.secs[shstrndx] if self.secs else None
        for s in self.secs:
            s.name = self.cstr(strs.off + s.name) if strs else ""
        self.syms = []
        st = [s for s in self.secs if s.type == SHT_SYMTAB]
        xs = [s for s in self.secs if s.type == SHT_SYMTAB_SHNDX]
        self.symtab = st[0] if st else None
        if st:
            st = st[0]
            ss = self.secs[st.link]
            for i in range(st.size // st.entsize):
                o = st.off + i * st.entsize
                y = Sym()
                y.idx = i
                if self.c64:
                    nm, info, y.other, y.shndx, y.value, y.size = struct.unpack_from("<IBBHQQ", data, o)
                else:
                    nm, y.value, y.size, info, y.other, y.shndx = struct.unpack_from("<IIIBBH", data, o)
                if y.shndx == SHN_XINDEX and xs:
                    y.shndx = struct.unpack_from("<I", data, xs[0].off + 4 * i)[0]
                y.name = self.cstr(ss.off + nm)
                y.bind, y.type, y.vis = info >> 4, info & 15, y.other & 3
                self.syms.append(y)

    def cstr(self, a):
        b = self.data.index(b"\0", a)
        return self.data[a:b].decode("latin-1")

    def bytes_of(self, s):
        if s.type == SHT_NOBITS:
            return b"\0" * s.size
        return self.data[s.off:s.off + s.size]

    def groups(self):
        """[(group section, flag word, signature, [member indexes])]"""
        out = []
        for s in self.secs:
            if s.type != SHT_GROUP:
                continue
            w = struct.unpack_from("<%dI" % (s.size // 4), self.data, s.off)
            sig = self.syms[s.info].name if s.info < len(self.syms) else "?"
            if sig == "" and s.info < len(self.syms) and self.syms[s.info].type == 3:
                sig = self.secs[self.syms[s.info].shndx].name
            out.append((s, w[0], sig, list(w[1:])))
        return out

    def width(self, rtype):
        if self.mach == EM_386:
            return I386_WIDTH.get(rtype, 4)
        return 0

    def rels(self, s):
        """the records of relocation section s, REL addends read from the target's bytes"""
        tgt = self.secs[s.info]
        out = []
        for i in range(s.size // s.entsize if s.entsize else 0):
            o = s.off + i * s.entsize
            r = Rel()
            r.explicit = s.type == SHT_RELA
            if self.c64:
                if s.type == SHT_RELA:
                    r.off, info, r.addend = struct.unpack_from("<QQq", self.data, o)
                else:
                    r.off, info = struct.unpack_from("<QQ", self.data, o)
                    r.addend = None
                r.type, r.sym = info & 0xffffffff, info >> 32
            else:
                if s.type == SHT_RELA:
                    r.off, info, r.addend = struct.unpack_from("<IIi", self.data, o)
                else:
                    r.off, info = struct.unpack_from("<II", self.data, o)
                    r.addend = None
                r.type, r.sym = info & 0xff, info >> 8
            if r.addend is None:
                w = self.width(r.type)
                if w and tgt.type != SHT_NOBITS and r.off + w <= tgt.size:
                    r.addend = int.from_bytes(self.data[tgt.off + r.off:tgt.off + r.off + w], "little", signed=True)
                else:
                    r.addend = 0
            out.append(r)
        return out

    def relsecs(self):
        for s in self.secs:
            if s.type in (SHT_RELA, SHT_REL) and s.info < len(self.secs):
                yield s, self.secs[s.info], self.rels(s)


def load(path):
    with open(path, "rb") as f:
        return Elf(f.read(), path)
