"""WP-239: a minimal ELF64 little-endian reader (objects, archive members by
their header offset, outputs), synth.md S-9/S-10's cut, and a format-4/5
state's table headers (spec/incr.md IN-18a: 64-byte header, then per table
a 40-byte record `(kind u32, pad u32, rows u64, bytes u64, L128)` and its
rows)."""
import struct


class Elf:
    def __init__(self, data, base=0):
        self.d = data
        self.b = base
        if data[base:base + 4] != b'\x7fELF' or data[base + 4] != 2 or data[base + 5] != 1:
            raise ValueError('not ELF64 LE')
        shoff, = struct.unpack_from('<Q', data, base + 0x28)
        shentsize, shnum, shstrndx = struct.unpack_from('<HHH', data, base + 0x3a)
        self.machine, = struct.unpack_from('<H', data, base + 0x12)
        self.h = []
        for i in range(shnum):
            n, t, fl, ad, of, sz, li, inf, al, es = struct.unpack_from('<IIQQQQIIQQ', data, base + shoff + i * shentsize)
            self.h.append(dict(name=n, type=t, flags=fl, addr=ad, offset=of, size=sz, link=li, info=inf, addralign=al, entsize=es))
        so = self.h[shstrndx]['offset'] if shnum else 0
        for x in self.h:
            s = base + so + x['name']
            x['sname'] = data[s:data.index(b'\0', s)].decode('latin1')

    @classmethod
    def open(cls, path, member=None):
        data = open(path, 'rb').read()
        return cls(data, 0 if member is None else member + 60)

    def shdr(self, i):
        return self.h[i]

    def section_bytes(self, i):
        x = self.h[i]
        if x['type'] == 8:
            return b''
        return self.d[self.b + x['offset']:self.b + x['offset'] + x['size']]

    def alloc_ranges(self):
        r = [(x['addr'], x['size'], x['sname']) for x in self.h if x['flags'] & 2 and x['size']]
        return sorted(r)

    def symbols(self):
        """(name, value, size, info, other, shndx) of .symtab."""
        for i, x in enumerate(self.h):
            if x['type'] == 2:
                strs = self.section_bytes(x['link'])
                b = self.section_bytes(i)
                out = []
                for k in range(0, len(b), 24):
                    nm, info, other, shndx, val, sz = struct.unpack_from('<IBBHQQ', b, k)
                    out.append((strs[nm:strs.index(b'\0', nm)].decode('latin1'), val, sz, info, other, shndx))
                return out
        return []

    def relas(self, target):
        """RELA entries (offset, type, sym, addend) applying to section `target`."""
        out = []
        for i, x in enumerate(self.h):
            if x['type'] == 4 and x['info'] == target:
                b = self.section_bytes(i)
                for k in range(0, len(b), 24):
                    off, info, add = struct.unpack_from('<QQq', b, k)
                    out.append((off, info & 0xffffffff, info >> 32, add))
        return out

    def groups(self):
        """(signature, [member section indices]) of each SHT_GROUP with GRP_COMDAT."""
        syms = None
        out = []
        for i, x in enumerate(self.h):
            if x['type'] == 17:
                b = self.section_bytes(i)
                flag, = struct.unpack_from('<I', b, 0)
                if not flag & 1:
                    continue
                if syms is None:
                    syms = self.symbols()
                mem = [struct.unpack_from('<I', b, k)[0] for k in range(4, len(b), 4)]
                out.append((syms[x['info']][0], mem))
        return out


def cut(data, strings, entsize):
    """synth.md S-9/S-10 (mortar_merge::cut::cut): (start, end) of each piece."""
    n = len(data)
    if entsize == 0 or n == 0 or n % entsize:
        return []
    if not strings:
        return [(o, o + entsize) for o in range(0, n, entsize)]
    out = []
    if entsize == 1:
        s = 0
        while s < n:
            z = data.find(b'\0', s)
            e = n if z < 0 else z + 1
            out.append((s, e))
            s = e
        return out
    s = 0
    for end in range(entsize, n + 1, entsize):
        if not any(data[end - entsize:end]):
            out.append((s, end))
            s = end
    return out


def state_tables(path):
    """{kind: (rows, bytes, body offset)} of a state file."""
    with open(path, 'rb') as f:
        d = f.read()
    if d[:8] != b'MORTARST':
        raise ValueError('not a state')
    at = 64
    out = {}
    while at + 40 <= len(d):
        kind, _, rows, nbytes = struct.unpack_from('<IIQQ', d, at)
        out[kind] = (rows, nbytes, at + 40)
        at += 40 + nbytes
    return out


def state_bytes(path):
    with open(path, 'rb') as f:
        return f.read()
