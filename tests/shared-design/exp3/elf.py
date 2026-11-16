"""Minimal ELF relocatable reader (ELF32/ELF64, LE), and an ar walker. Reviewer's own."""
import struct

SHT_SYMTAB, SHT_RELA, SHT_REL = 2, 4, 9
SHF_WRITE, SHF_ALLOC, SHF_EXEC, SHF_TLS = 1, 2, 4, 0x400
EM_386, EM_X86_64, EM_AARCH64 = 3, 62, 183


class Sec:
    __slots__ = ("idx", "name", "type", "flags", "off", "size", "link", "info", "entsize")


class Sym:
    __slots__ = ("idx", "name", "value", "size", "bind", "type", "vis", "shndx")


class Rel:
    __slots__ = ("off", "type", "sym", "addend")


class Obj:
    def __init__(self, data, name="?"):
        self.data, self.name = data, name
        if data[:4] != b"\x7fELF":
            raise ValueError("not ELF")
        self.c64 = data[4] == 2
        if data[5] != 1:
            raise ValueError("BE")
        if self.c64:
            (self.etype, self.mach, _, _, _, shoff, _, _, _, _, shentsize, shnum, shstrndx) = struct.unpack_from("<HHIQQQIHHHHHH", data, 16)
        else:
            (self.etype, self.mach, _, _, _, shoff, _, _, _, _, shentsize, shnum, shstrndx) = struct.unpack_from("<HHIIIIIHHHHHH", data, 16)
        if self.etype != 1:
            raise ValueError("not ET_REL")
        self.secs = []
        for i in range(shnum):
            o = shoff + i * shentsize
            s = Sec()
            s.idx = i
            if self.c64:
                (nm, s.type, s.flags, _, s.off, s.size, s.link, s.info, _, s.entsize) = struct.unpack_from("<IIQQQQIIQQ", data, o)
            else:
                (nm, s.type, s.flags, _, s.off, s.size, s.link, s.info, _, s.entsize) = struct.unpack_from("<IIIIIIIIII", data, o)
            s.name = nm
            self.secs.append(s)
        strs = self.secs[shstrndx]
        for s in self.secs:
            s.name = self._str(strs, s.name)
        self.syms = []
        symtab = [s for s in self.secs if s.type == SHT_SYMTAB]
        if symtab:
            st = symtab[0]
            ss = self.secs[st.link]
            n = st.size // st.entsize
            for i in range(n):
                o = st.off + i * st.entsize
                y = Sym()
                y.idx = i
                if self.c64:
                    nm, info, other, y.shndx, y.value, y.size = struct.unpack_from("<IBBHQQ", data, o)
                else:
                    nm, y.value, y.size, info, other, y.shndx = struct.unpack_from("<IIIBBH", data, o)
                y.name = self._str(ss, nm)
                y.bind, y.type, y.vis = info >> 4, info & 15, other & 3
                self.syms.append(y)

    def _str(self, sec, off):
        d = self.data
        a = sec.off + off
        b = d.index(b"\0", a)
        return d[a:b].decode("latin-1")

    def relsecs(self):
        """yield (target section, [Rel])"""
        for s in self.secs:
            if s.type not in (SHT_RELA, SHT_REL):
                continue
            tgt = self.secs[s.info]
            out = []
            n = s.size // s.entsize
            for i in range(n):
                o = s.off + i * s.entsize
                r = Rel()
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
                    # REL implicit addend: i386 32-bit fields (the only REL target here)
                    if tgt.type != 8 and r.off + 4 <= tgt.size:  # not NOBITS
                        r.addend = struct.unpack_from("<i", self.data, tgt.off + r.off)[0]
                    else:
                        r.addend = 0
                out.append(r)
            yield tgt, out

    def code(self, sec, off, n):
        return self.data[sec.off + off: sec.off + off + n]


def ar_members(data):
    """yield (name, bytes) of an ar archive (GNU, thin not supported)."""
    if data[:8] != b"!<arch>\n":
        return
    p = 8
    longnames = b""
    while p + 60 <= len(data):
        h = data[p:p + 60]
        name = h[:16].decode("latin-1").strip()
        size = int(h[48:58].decode().strip())
        body = data[p + 60:p + 60 + size]
        p += 60 + size + (size & 1)
        if name == "//":
            longnames = body
            continue
        if name in ("/", "/SYM64/"):
            continue
        if name.startswith("/") and name[1:].isdigit():
            o = int(name[1:])
            e = longnames.index(b"/\n", o)
            name = longnames[o:e].decode("latin-1")
        elif name.endswith("/"):
            name = name[:-1]
        yield name, body


def load_any(path):
    data = open(path, "rb").read()
    if data[:8] == b"!<arch>\n":
        for nm, body in ar_members(data):
            if body[:4] == b"\x7fELF":
                try:
                    yield f"{path}({nm})", Obj(body, nm)
                except ValueError:
                    pass
    elif data[:4] == b"\x7fELF":
        try:
            yield path, Obj(data, path)
        except ValueError:
            pass
