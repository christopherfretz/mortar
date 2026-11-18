#!/usr/bin/env python3
"""oracle.py BASE_DIR VARIANT_DIR [--drop ROWS] [--quiet]

WP-060 refill oracle (spec/incr.md IN-27, as revised; IN-63a). Each DIR
holds a full mortar link: DIR/prog, DIR/prog.map (--mortar-map) and the
inputs at the paths the map names (relative paths resolve against DIR).
A file is identified across the two links by (map path, member, offset);
it is CHANGED when its bytes differ, it is missing in one link, or it is
extracted in only one.

Units: `place` (allocated input section), `nplace` (non-allocated, offset
in the output section) and `piece` groups (a file's contributions of one
input section to .eh_frame / merged allocated sections, as a whole).
For every unit of an UNCHANGED file present in both links:
  same placement, bytes differ -> must be in the predicted refill set;
  placement differs            -> "moved" (allowed only if predicted:
                                  the moved-section row predicts it);
plus bytes that differ outside every unit, per output section.

Predictor rows (--drop takes a comma list of these names):
  changed  a file changed/appeared/extracted: all its units
  global   a global's definition moved/changed (value, size, definer,
           kind): every unit whose relocations name it (alloc or not)
  comdat   a COMDAT keeper changed: new keeper's members; referrers of the
           group's symbols; every file whose copy became dead/live: its
           .eh_frame contribution and all its non-allocated units
  gc       a non-COMDAT section of an unchanged file became dead/live:
           referrers of its globals; the file's .eh_frame contribution and
           all its non-allocated units
  moved    a section of an unchanged file moved: it, the referrers of it
           (same file, via section/local symbols) and of its globals; if
           it is allocated, all non-allocated units of its file (a moved
           .eh_frame piece group or non-allocated unit: it and its
           referrers only)
  synth    (IN-27's synthetic-table row) a slot table (.got, .got.plt,
           .plt chunk) moved or resized: units whose relocations use a
           GOT/TLS slot, and referrers of IFUNC globals
  frag     (full-link artifact) a fragment of a NON-allocated merged
           section (.debug_str, .debug_line_str) got another output offset:
           the units whose relocated fields name it. The map has no pieces
           for these sections, so the fragment's offset is read from the
           field in both outputs. An update appends fragments and keeps
           old offsets (incr.md, merged strings row), so in an incremental
           link these units would not differ.
  tls      any TLS input section's TP offset changes -> full-link
           fallback (tls-layout-changed): everything predicted
  symattr  (IN-26 v1) a global's type, binding, st_other or kind
           (absolute/common/section) changed and an unchanged file
           references it -> full-link fallback symbol-kind-changed
The global row compares definitions by identity (definer, kind, section,
offset, size, live); the output value only for a changed definer, an
absolute, common or linker-defined symbol.
Moved units (re-review N4) are compared by value after undoing the move
(unit_by_value): bytes outside relocated fields equal, each field names
the same thing with the same residual; one that differs beyond the move
must be predicted by a row other than `moved`.
Prints RESULT PASS/FAIL (FAIL = some differing in-place unit, or moved
unit differing beyond its move, of an unchanged file is not predicted)
and a SUMMARY line.
"""
import os, sys, struct, collections, time

T0 = time.time()
ROWS = ['changed', 'global', 'symattr', 'comdat', 'gc', 'moved', 'synth', 'frag', 'tls']
# relocation types that go through a GOT / TLS slot (GOT, TLS GD/LD/IE/desc)
SLOT = {183: set(range(307, 314)) | set(range(513, 517)) | set(range(539, 544)) | set(range(560, 570)),
        62: {3, 9, 19, 20, 22, 25, 26, 27, 28, 29, 30, 34, 41, 42},
        3: {3, 9, 10, 15, 16, 18, 19, 39, 43}}


# ---------------------------------------------------------------- ELF ---
def elf(b):
    assert b[:4] == b'\x7fELF'
    c64 = b[4] == 2
    if c64:
        (shoff,) = struct.unpack_from('<Q', b, 0x28)
        (phoff,) = struct.unpack_from('<Q', b, 0x20)
        phent, phnum, shent, shnum, shstr = struct.unpack_from('<HHHHH', b, 0x36)
        mach = struct.unpack_from('<H', b, 0x12)[0]
    else:
        (phoff, shoff) = struct.unpack_from('<II', b, 0x1c)
        phent, phnum, shent, shnum, shstr = struct.unpack_from('<HHHHH', b, 0x2a)
        mach = struct.unpack_from('<H', b, 0x12)[0]
    S = []
    for i in range(shnum):
        o = shoff + i * shent
        if c64:
            nm, ty, fl, ad, off, sz, ln, inf, al, es = struct.unpack_from('<IIQQQQIIQQ', b, o)
        else:
            nm, ty, fl, ad, off, sz, ln, inf, al, es = struct.unpack_from('<IIIIIIIIII', b, o)
        S.append(dict(nm=nm, type=ty, flags=fl, addr=ad, off=off, size=sz, link=ln, info=inf, align=al, entsize=es))
    if S:
        so = S[shstr]['off']
        for s in S:
            s['name'] = b[so + s['nm']:b.index(b'\0', so + s['nm'])].decode('latin1')
    P = []
    for i in range(phnum):
        o = phoff + i * phent
        if c64:
            ty, fl, off, va, pa, fs, ms, al = struct.unpack_from('<IIQQQQQQ', b, o)
        else:
            ty, off, va, pa, fs, ms, fl, al = struct.unpack_from('<IIIIIIII', b, o)
        P.append(dict(type=ty, off=off, vaddr=va, filesz=fs, memsz=ms, align=al))
    return c64, mach, S, P


def syms_of(b, c64, S):
    out = []
    st = next((s for s in S if s['type'] == 2), None)
    if not st:
        return out
    so = S[st['link']]['off']
    ent = 24 if c64 else 16
    for i in range(st['size'] // ent):
        o = st['off'] + i * ent
        if c64:
            n, info, oth, shn, val, size = struct.unpack_from('<IBBHQQ', b, o)
        else:
            n, val, size, info, oth, shn = struct.unpack_from('<IIIBBH', b, o)
        out.append(dict(name=b[so + n:b.index(b'\0', so + n)].decode('latin1'),
                        bind=info >> 4, type=info & 15, shn=shn, value=val, size=size, other=oth))
    return out


class Obj:
    """An input object: sections, symbols, relocations per target section
    as lists of symbol indices, groups (signature -> member indices)."""
    def __init__(self, b):
        self.bytes = b
        c64, self.mach, S, _ = elf(b)
        self.S = S
        self.syms = syms_of(b, c64, S)
        self.rels = collections.defaultdict(set)
        self.relo = collections.defaultdict(list)
        self.rela = collections.defaultdict(list)   # sec -> (off, sym, type, addend or None for REL)
        for s in S:
            if s['type'] in (4, 9):
                ent = (24 if c64 else 12) if s['type'] == 4 else (16 if c64 else 8)
                for i in range(s['size'] // ent):
                    o = s['off'] + i * ent
                    if c64:
                        (info,) = struct.unpack_from('<Q', b, o + 8); sym, ty = info >> 32, info & 0xffffffff
                    else:
                        (info,) = struct.unpack_from('<I', b, o + 4); sym, ty = info >> 8, info & 0xff
                    self.rels[s['info']].add((sym, ty))
                    (roff,) = struct.unpack_from('<Q' if c64 else '<I', b, o)
                    self.relo[s['info']].append((roff, sym, ty))
                    add = None
                    if s['type'] == 4:
                        (add,) = struct.unpack_from('<q' if c64 else '<i', b, o + (16 if c64 else 8))
                    self.rela[s['info']].append((roff, sym, ty, add))
        self.groups = {}
        for s in S:
            if s['type'] == 17:
                sig = self.syms[s['info']]['name'] if s['info'] < len(self.syms) else '?'
                if sig == '' and s['info'] < len(self.syms) and self.syms[s['info']]['type'] == 3:
                    sig = S[self.syms[s['info']]['shn']]['name']
                mem = struct.unpack_from('<%dI' % (s['size'] // 4), b, s['off'])[1:]
                self.groups[sig] = set(mem)
        self.grouped = set().union(*self.groups.values()) if self.groups else set()

    def merged_fields(self, sec):
        """Offsets and widths of `sec`'s relocated fields that name a
        non-allocated merged (SHF_MERGE) section through a local symbol."""
        W = {183: {257: 8, 258: 4}, 62: {1: 8, 10: 4, 11: 4}, 3: {1: 4}}.get(self.mach, {})
        out = []
        for off, i, ty in self.relo.get(sec, ()):
            if i < len(self.syms) and ty in W:
                y = self.syms[i]
                if y['bind'] == 0 and 0 < y['shn'] < len(self.S):
                    t = self.S[y['shn']]
                    if t['flags'] & 0x10 and not t['flags'] & 2:
                        out.append((off, W[ty]))
        return out

    def globals_in(self, sec):
        return {y['name'] for y in self.syms if y['bind'] != 0 and y['shn'] == sec and y['name']}

    def names_by_sec(self, sec):
        """Relocations of `sec`: (global names, own sections named)."""
        g, own, slot = set(), set(), set()
        st = SLOT.get(self.mach, set())
        for i, ty in self.rels.get(sec, ()):
            if i >= len(self.syms):
                continue
            y = self.syms[i]
            if y['bind'] != 0 and y['name']:
                g.add(y['name'])
                if ty in st:
                    slot.add(y['name'])
            elif 0 < y['shn'] < 0xff00:
                own.add(y['shn'])
                if ty in st:
                    slot.add(('local', y['shn']))
        return g, own, slot


# --------------------------------------------------------------- link ---
class Link:
    def __init__(self, d):
        self.d = d
        self.out = open(os.path.join(d, 'prog'), 'rb').read()
        self.c64, self.mach, self.OS, self.P = elf(self.out)
        self.files = {}        # id -> key
        self.units = {}        # (key, kind, sec) -> placement tuple
        self.ranges = {}       # unit -> list of (outsec name, start off, len)
        self.discard = set()
        self.defines = {}
        self.chunks = {}
        pieces = collections.defaultdict(list)
        for line in open(os.path.join(d, 'prog.map')):
            t = line.split()
            if not t:
                continue
            if t[0] == 'file':
                self.files[int(t[1])] = (t[2], t[3] if len(t) > 3 else None, int(t[4], 16) if len(t) > 4 else None)
            elif t[0] == 'place':
                k = (self.files[int(t[1])], 'p', int(t[2]))
                a, z = int(t[3], 16), int(t[4], 16)
                self.units[k] = (a, z)
                self.ranges[k] = [self.at_addr(a, z)]
            elif t[0] == 'nplace':
                k = (self.files[int(t[1])], 'n', int(t[2]))
                o, z = int(t[4], 16), int(t[5], 16)
                self.units[k] = (t[3], o, z)
                self.ranges[k] = [(t[3], o, z)]
            elif t[0] == 'piece':
                pieces[(self.files[int(t[1])], 'e', int(t[2]))].append((int(t[3], 16), int(t[4], 16), int(t[5], 16)))
            elif t[0] == 'discard':
                self.discard.add((self.files[int(t[1])], int(t[2])))
            elif t[0] == 'chunk' and t[3] != '-':
                self.chunks[t[2]] = (int(t[3], 16), int(t[4], 16))
            elif t[0] == 'define':
                self.defines[t[1]] = int(t[2], 16)
        for k, ps in pieces.items():
            ps.sort()
            self.units[k] = tuple(ps)
            self.ranges[k] = [self.at_addr(a, z) for (_i, a, z) in ps]
        self.keys = set(self.files.values())
        self._eh_rev = None
        self.nobits = set()    # units whose input section is NOBITS (set by main)
        self.osyms = {y['name']: y['value'] for y in syms_of(self.out, self.c64, self.OS) if y['bind'] != 0 and y['name']}
        tls = next((p for p in self.P if p['type'] == 7), None)
        self.tls_vaddr = tls['vaddr'] if tls else None
        self.tp_base = None
        if tls:
            al = max(tls['align'], 1)
            if self.mach == 183:   # AArch64, variant 1: TCB of 16 bytes
                self.tp_base = -tls['vaddr'] + ((16 + al - 1) // al) * al
            else:                  # x86 / x86-64, variant 2
                self.tp_base = -tls['vaddr'] - ((tls['memsz'] + al - 1) // al) * al

    def unit_pieces(self, u):
        """[(input offset, address (or non-alloc output-section offset), size, file offset or None)]"""
        key, kind, sec = u
        pl = self.units[u]
        if kind == 'p':
            (sn, o, z) = self.ranges[u][0]
            s = self.osec(sn)
            fp = None if (o is None or s is None or s['type'] == 8 or u in self.nobits) else s['off'] + o
            return [(0, pl[0], pl[1], fp)]
        if kind == 'n':
            s = self.osec(pl[0])
            return [(0, s['addr'] + pl[1], pl[2], s['off'] + pl[1])]
        out = []
        for (i, a, z), (sn, o, _z) in zip(pl, self.ranges[u]):
            s = self.osec(sn)
            out.append((i, a, z, None if (o is None or s is None or s['type'] == 8) else s['off'] + o))
        return out

    def eh_owner(self, a):
        """the .eh_frame piece (unit, input offset) whose output range holds address a"""
        if self._eh_rev is None:
            import bisect
            rows = []
            for u, pl in self.units.items():
                if u[1] == 'e':
                    for (i, pa, z) in pl:
                        rows.append((pa, z, u, i))
            rows.sort(key=lambda r: r[0])
            self._eh_rev = ([r[0] for r in rows], rows, bisect)
        ks, rows, bisect = self._eh_rev
        j = bisect.bisect_right(ks, a) - 1
        if j >= 0 and rows[j][0] <= a < rows[j][0] + rows[j][1]:
            return (rows[j][2], rows[j][3] + a - rows[j][0])
        return None

    def at_addr(self, a, z):
        # PROGBITS first: a .tbss's address range overlaps the sections
        # after it (fixed in this round: it hid .data.rel.ro's first bytes)
        for s in self.OS:
            if s['flags'] & 2 and s['addr'] <= a < s['addr'] + max(s['size'], 1) and s['type'] != 8:
                return (s['name'], a - s['addr'], z)
        for s in self.OS:
            if s['flags'] & 2 and s['type'] == 8 and s['addr'] <= a <= s['addr'] + s['size']:
                return (s['name'], None, z)
        return ('?', None, z)

    def osec(self, name):
        return next((s for s in self.OS if s['name'] == name), None)

    def unit_bytes(self, u):
        if u in self.nobits:
            return b''
        out = b''
        for (sn, o, z) in self.ranges[u]:
            s = self.osec(sn)
            if o is None or s is None or s['type'] == 8:
                continue
            out += self.out[s['off'] + o:s['off'] + o + z]
        return out

    def file_bytes(self, key):
        path, mem, off = key
        p = path if os.path.isabs(path) else os.path.join(self.d, path)
        with open(p, 'rb') as f:
            if mem is None:
                return f.read()
            f.seek(off); h = f.read(60)
            return f.read(int(h[48:58].decode().strip()))

    def tls_offsets(self, objs):
        """TP offset of every placed TLS input section."""
        if self.tp_base is None:
            return {}
        base = self.tp_base
        r = {}
        for (key, kind, sec), pl in self.units.items():
            if kind == 'p' and objs[key].S[sec]['flags'] & 0x400:
                r[(key, sec)] = pl[0] + base
        return r


def resolution(L, objs):
    """global name -> (definer key, kind, output value, size, type)."""
    r = {}
    for fid in sorted(L.files):
        key = L.files[fid]
        o = objs[key]
        for y in o.syms:
            if y['bind'] == 0 or not y['name'] or y['shn'] == 0:
                continue
            if 0 < y['shn'] < 0xff00 and (key, y['shn']) in L.discard:
                continue
            if 0 < y['shn'] < 0xff00 and any((key, m) in L.discard for g in o.groups.values() if y['shn'] in g for m in g):
                continue
            kind = 'abs' if y['shn'] == 0xfff1 else 'common' if y['shn'] == 0xfff2 else 'def'
            if 0 < y['shn'] < 0xff00:
                pl = L.units.get((key, 'p', y['shn']))
                val = (pl[0] + y['value']) if pl else ('dead', y['shn'], y['value'])
            else:
                val = y['value']
            cur = r.get(y['name'])
            strong = y['bind'] == 1 and kind != 'common'
            if cur is None or (strong and not cur[5]):
                r[y['name']] = (key, kind, val, y['size'], y['type'], strong, y['shn'], y['value'], y['bind'], y['other'])
    out = {n: v for n, v in r.items()}
    for n, v in L.defines.items():
        out.setdefault(n, ('linker', 'def', v, 0, 0, True, None, v, 1, 0))
    return out


def gview(v, changed):
    """IN-27's global row key: (definer, kind, section, offset in it, size);
    the output value only where no unchanged input section carries it
    (a changed definer, absolute, common, linker-defined): a definition in
    an unchanged file whose section moved is the moved row's."""
    key, kind, val, size = v[0], v[1], v[2], v[3]
    alive = not isinstance(val, tuple)
    if key == 'linker' or kind != 'def' or key in changed:
        return (key, kind, v[6], v[7], size, val)
    return (key, kind, v[6], v[7], size, alive)


def aview(v):
    """IN-26's symbol attributes: type, binding, st_other, and absolute /
    common / section."""
    return (v[4], v[8], v[9], v[1])


def keepers(L, objs):
    k = {}
    for fid in sorted(L.files):
        key = L.files[fid]
        for sig, mem in objs[key].groups.items():
            if sig in k:
                continue
            if not any((key, m) in L.discard for m in mem):
                k[sig] = key
    return k


# ------------------------------------------- by-value comparison (N4) ---
# A unit of an unchanged file that the full link moved is compared after
# undoing the move: outside its relocated fields byte for byte; each
# relocated field must name the same thing in both links (the record's
# symbol resolved to the same (file, section, offset), the same absolute
# value, the same linker symbol or the same undefined name) and hold the
# same RESIDUAL: the field's decoded value minus what the relocation's
# formula gives for that thing's address in that link (0 for a plain
# application; non-zero, e.g., for a call routed through an IPLT entry or
# a veneer). Slot fields (GOT, TLS GD/LD/IE/desc) are compared by the
# record's symbol and the instruction class only: the slot's own address
# is the synthetic table's (synth row), not the unit's.
A64_SLOT = set(range(307, 314)) | set(range(512, 517)) | set(range(539, 544)) | set(range(560, 570))
X64_SLOT = {3, 9, 19, 20, 22, 25, 26, 27, 28, 29, 30, 34, 41, 42}
M26, M19, M14, M12, M16 = 0x3ffffff, 0x7ffff << 5, 0x3fff << 5, 0xfff << 10, 0xffff << 5
MADR = (3 << 29) | (0x7ffff << 5)


def a64_class_mask(w):
    if w & 0x1f000000 == 0x10000000: return MADR            # adr / adrp
    if w & 0x1f000000 == 0x11000000: return M12             # add/sub imm
    if w & 0x3b000000 == 0x39000000: return M12             # ldr/str unsigned imm
    if w & 0x1f800000 == 0x12800000: return M16             # movz/movk/movn
    if w & 0x3b000000 == 0x18000000: return M19             # ldr literal
    if w & 0x7c000000 == 0x14000000: return M26             # b / bl
    return 0


def field(mach, ty):
    """(width, kind, extra) or None (unknown type). kind: abs pc page lo
    movw tp tphi tplo dtp slot none."""
    if mach == 183:
        if ty in (0, 256): return (0, 'none', 0)
        if ty == 257: return (8, 'abs', 64)
        if ty == 258: return (4, 'abs', 32)
        if ty == 259: return (2, 'abs', 16)
        if ty == 260: return (8, 'pc', 64)
        if ty == 261: return (4, 'pc', 32)
        if ty == 262: return (2, 'pc', 16)
        if 263 <= ty <= 269: return (4, 'movw', (ty - 263) // 2)
        if ty in (273, 280): return (4, 'br', 19)
        if ty == 279: return (4, 'br', 14)
        if ty in (282, 283): return (4, 'br', 26)
        if ty == 274: return (4, 'adr', 0)
        if ty in (275, 276): return (4, 'page', 0)
        if ty in (277, 278): return (4, 'lo', 0)
        if ty in (284, 285, 286, 299): return (4, 'lo', {284: 1, 285: 2, 286: 3, 299: 4}[ty])
        if ty in A64_SLOT: return (4, 'slot', 0)
        if 544 <= ty <= 548: return (4, 'tpmovw', (2, 1, 1, 0, 0)[ty - 544])
        if ty == 549: return (4, 'tphi', 0)
        if ty in (550, 551): return (4, 'tplo', 0)
        if 552 <= ty <= 559: return (4, 'tplo', (ty - 552) // 2)
        if ty == 1029: return (8, 'dtp', 64)
        return None
    if mach == 62:
        if ty == 0: return (0, 'none', 0)
        if ty == 1: return (8, 'abs', 64)
        if ty in (10, 11): return (4, 'abs', 32)
        if ty in (2, 4): return (4, 'pc', 32)
        if ty == 24: return (8, 'pc', 64)
        if ty in X64_SLOT: return (4, 'slot', 0)
        if ty == 35: return (0, 'none', 0)    # TLSDESC_CALL marks an instruction
        if ty == 23: return (4, 'tp', 32)
        if ty == 18: return (8, 'tp', 64)
        if ty == 21: return (4, 'dtp', 32)
        if ty == 17: return (8, 'dtp', 64)
        return None
    return None


def sext(v, n):
    v &= (1 << n) - 1
    return v - (1 << n) if v >> (n - 1) else v


class Valuer:
    """Per link: where a relocation record's symbol + addend points."""
    def __init__(self, L, objs, res):
        self.L, self.objs, self.res = L, objs, res
        self.strs = {}

    def loc(self, key, sym, add):
        o = self.objs[key]
        y = o.syms[sym] if sym < len(o.syms) else None
        add = add or 0
        if y is None:
            return ('nosym',)
        if y['bind'] == 0 or not y['name']:
            shn = y['shn']
            if shn == 0:
                return ('undef-local', y['name'], add)
            if shn == 0xfff1:
                return ('abs', y['value'] + add)
            if shn >= 0xff00:
                return ('special', shn, y['value'] + add)
            return ('sec', key, shn, y['value'] + add)
        r = self.res.get(y['name'])
        if r is None:
            return ('undef', y['name'], add)
        if r[0] == 'linker':
            return ('linker', y['name'], add)
        if r[1] == 'abs':
            return ('abs', r[7] + add)
        if r[1] == 'common':
            return ('common', y['name'], add)
        return ('sec', r[0], r[6], r[7] + add)

    def addr(self, lc):
        """address of a location in this link; None if it is not in the
        output; ('str', bytes) for a non-alloc merged section."""
        L = self.L
        t = lc[0]
        if t == 'abs':
            return lc[1]
        if t in ('linker', 'common'):
            v = L.defines.get(lc[1], L.osyms.get(lc[1]))
            return None if v is None else v + lc[2]
        if t == 'undef':
            return 0 + lc[2]
        if t != 'sec':
            return None
        _, key, shn, off = lc
        pl = L.units.get((key, 'p', shn))
        if pl:
            return pl[0] + off
        pl = L.units.get((key, 'n', shn))
        if pl:
            return L.osec(pl[0])['addr'] + pl[1] + off
        pl = L.units.get((key, 'e', shn))
        if pl:
            # the fragment or record that CONTAINS off first; one-past-the-end
            # (an end label) only if none does. (Fixed in the final round: the
            # end clause used to match a de-duplicated empty-string fragment
            # just before the containing one, e.g. pieces 0x1f+1 and 0x20+0x17
            # for off 0x20, and mapped the field to the wrong fragment.)
            for (i, a, z) in pl:
                if i <= off < i + z:
                    return a + off - i
            for (i, a, z) in pl:
                if off == i + z:
                    return a + off - i
            # x86-64's PC32 to a section symbol of a merged section carries
            # -4 in the addend (value + addend lies before the fragment):
            # extrapolate from the next fragment
            nxt = [(i, a) for (i, a, z) in pl if i > off]
            if nxt:
                i, a = min(nxt)
                return a + off - i
            return None
        s = self.objs[key].S[shn] if shn < len(self.objs[key].S) else None
        # synth.md S-50 rule 1: offset 0 of an input .eh_frame (crtbeginT.o's
        # empty one on x86: no record, so no piece line) is the start of the
        # output .eh_frame, whatever the input holds
        if s and s['name'] == '.eh_frame' and off == 0 and L.osec('.eh_frame'):
            return L.osec('.eh_frame')['addr']
        if s and s['flags'] & 0x10 and not s['flags'] & 2:
            b = self.objs[key].bytes
            p = s['off'] + off
            e = b.find(b'\0', p, s['off'] + s['size'])
            return ('str', b[p:e if e >= 0 else s['off'] + s['size']])
        return None

    def out_str(self, sname, off):
        s = self.L.osec(sname)
        if s is None:
            return None
        p = s['off'] + off
        e = self.L.out.find(b'\0', p, s['off'] + s['size'])
        return self.L.out[p:e if e >= 0 else s['off'] + s['size']]


def field_value(L, V, mach, spec, word, P, lc, wfield, tgt_sec_name):
    """(mask over the field's bytes, residual) for one relocated field."""
    width, kind, x = spec
    T = V.addr(lc)
    if kind == 'none':
        return 0, None
    full = (1 << (8 * width)) - 1
    if kind == 'slot':
        # the whole word: a per-site relaxation (ADRP -> ADR, LDR -> ADD /
        # NOP, IE -> LE) may rewrite it; the caller counts class flips
        return full, ('slot', lc)
    if lc[0] == 'undef' and kind == 'br':
        return full, ('undef-branch', word & full)    # a call to an undefined weak: linker's choice
    if isinstance(T, tuple):     # non-alloc merged string: compare the string the field names
        if kind == 'abs':
            got = V.out_str(tgt_sec_name, word & full)
            return full, ('str', got if got == T[1] else ('WRONG', got))
        return full, ('str?', word)
    if T is None:                # target not in the output (dead): compare the raw field
        return full, ('dead', word & full)
    tp = (T + V.L.tp_base) if V.L.tp_base is not None else None
    dtp = (T - V.L.tls_vaddr) if V.L.tls_vaddr is not None else None
    if kind in ('abs', 'pc', 'tp', 'dtp'):
        e = {'abs': T, 'pc': T - P, 'tp': tp, 'dtp': dtp}[kind]
        if e is None:
            return full, ('raw', word & full)
        return full, ((word - e) & ((1 << x) - 1))
    if kind == 'movw':
        return M16, (((word >> 5) & 0xffff) - ((T >> (16 * x)) & 0xffff)) & 0xffff
    if kind == 'tpmovw':
        return M16, (((word >> 5) & 0xffff) - ((tp >> (16 * x)) & 0xffff)) & 0xffff if tp is not None else ('raw', word)
    if kind == 'br':
        m = {26: M26, 19: M19, 14: M14}[x]
        f = word & M26 if x == 26 else (word & m) >> 5
        return m, (f - ((T - P) >> 2)) & ((1 << x) - 1)
    if kind in ('adr', 'page'):
        f = ((word >> 29) & 3) | (((word >> 5) & 0x7ffff) << 2)
        e = (T - P) if kind == 'adr' else ((T >> 12) - (P >> 12))
        return MADR, (f - e) & 0x1fffff
    if kind == 'lo':
        return M12, (((word >> 10) & 0xfff) - ((T & 0xfff) >> x)) & 0xfff
    if kind == 'tphi':
        return M12, (((word >> 10) & 0xfff) - ((tp >> 12) & 0xfff)) & 0xfff if tp is not None else ('raw', word)
    if kind == 'tplo':
        return M12, (((word >> 10) & 0xfff) - ((tp & 0xfff) >> x)) & 0xfff if tp is not None else ('raw', word)
    return full, ('raw', word)


def unit_by_value(A, B, VA, VB, ob, u, stats):
    """None if the moved unit u holds the same value in A and B after
    undoing the move, else a short reason."""
    pa, pb = A.unit_pieces(u), B.unit_pieces(u)
    if [(i, z) for (i, _a, z, _f) in pa] != [(i, z) for (i, _a, z, _f) in pb]:
        return 'piece set or sizes differ'
    key, kind, sec = u

    def gather(L, ps):
        buf = bytearray()
        idx = []
        for (i, a, z, fp) in ps:
            idx.append((i, a, z, len(buf)))
            buf += L.out[fp:fp + z] if fp is not None else b'\0' * z
        return buf, idx
    ba, ia = gather(A, pa)
    bb, ib = gather(B, pb)
    ma = bytearray(b'\xff' * len(ba))
    mach = ob.mach
    tname = {}
    why = None

    def at(idx, off):
        for (i, a, z, p) in idx:
            if i <= off < i + z:
                return a + off - i, p + off - i
        return None, None
    for (off, sym, ty, add) in ob.rela.get(sec, ()):
        spec = field(mach, ty)
        Pa, qa = at(ia, off)
        Pb, qb = at(ib, off)
        if qa is None:
            continue          # the field lies in a piece the link dropped
        if spec is None:
            stats['unknown'] += 1
            w = 8 if (mach, ty) in ((183, 1030), (62, 16)) else 4
            for j in range(qa, min(qa + w, len(ma))):
                ma[j] = 0
            continue
        width = spec[0]
        if width == 0:
            stats['none'] += 1
            continue
        fmt = {2: '<H', 4: '<I', 8: '<Q'}[width]
        if qa + width > len(ba):
            continue
        wa = struct.unpack_from(fmt, ba, qa)[0]
        wb = struct.unpack_from(fmt, bb, qb)[0]
        la, lb = VA.loc(key, sym, add), VB.loc(key, sym, add)
        if la != lb:
            why = why or 'field at +%#x names %s in one link, %s in the other' % (off, la, lb)
        tsn = None
        if la[0] == 'sec':
            s = VA.objs[la[1]].S[la[2]]
            tsn = s['name']
        # a reference to an IFUNC goes to its IPLT entry: a slot, compared by symbol
        nm = ob.syms[sym]['name'] if sym < len(ob.syms) and ob.syms[sym]['bind'] != 0 else None
        spa = spb = spec
        if nm and spec[1] in ('abs', 'pc', 'br', 'adr', 'page', 'lo'):
            if VA.res.get(nm, (0,) * 5)[4] == 10: spa = (width, 'slot', 0)
            if VB.res.get(nm, (0,) * 5)[4] == 10: spb = (width, 'slot', 0)
        ka, ra = field_value(A, VA, mach, spa, wa, Pa, la, width, tsn)
        kb, rb = field_value(B, VB, mach, spb, wb, Pb, lb, width, tsn)
        stats['slot' if spa[1] == 'slot' else 'checked'] += 1
        if spa[1] == 'slot' and spb[1] == 'slot' and mach == 183 and (wa ^ wb) & ~a64_class_mask(wa) & 0xffffffff:
            stats['relaxflip'] += 1
            stats.setdefault('flipunits', set()).add(u)
        if ra != rb:
            why = why or 'field at +%#x (type %d, %s): residual %s vs %s' % (off, ty, la, ra, rb)
        m = ka | kb
        for j in range(width):
            if (m >> (8 * j)) & 0xff:
                ma[qa + j] &= ~((m >> (8 * j)) & 0xff) & 0xff
        if qa != qb:
            why = why or 'piece layout differs'
    # an FDE's CIE pointer: it must name the same CIE (by its input identity)
    if kind == 'e' and ob.S[sec]['name'].startswith('.eh_frame'):
        for (i, a, z, p), (_i, a2, _z, p2) in zip(ia, ib):
            if z >= 8 and struct.unpack_from('<I', ba, p + 4)[0] != 0:
                ca = A.eh_owner(a + 4 - struct.unpack_from('<I', ba, p + 4)[0])
                cb = B.eh_owner(a2 + 4 - struct.unpack_from('<I', bb, p2 + 4)[0])
                ca = (ca[0], ca[1]) if ca else None
                if ca != cb:
                    why = why or 'FDE at +%#x names CIE %s vs %s' % (i, ca, cb)
                for j in range(p + 4, p + 8):
                    ma[j] = 0
    if why is None and ba != bb:
        m = int.from_bytes(ma, 'little')
        d = (int.from_bytes(ba, 'little') ^ int.from_bytes(bb, 'little')) & m
        if d:
            j = ((d & -d).bit_length() - 1) // 8
            why = 'bytes differ outside relocated fields at +%#x' % j
    return why


def short(key):
    p, m, o = key
    return (os.path.basename(p) + '(' + m + ')') if m else p


# ---------------------------------------------------------------- main ---
def main():
    args = [a for a in sys.argv[1:] if not a.startswith('--')]
    drop = set()
    if '--drop' in sys.argv:
        drop = set(sys.argv[sys.argv.index('--drop') + 1].split(','))
        args = [a for a in args if a != sys.argv[sys.argv.index('--drop') + 1]]
    quiet = '--quiet' in sys.argv
    A, B = Link(args[0]), Link(args[1])
    objs = {}
    fb = {}
    for L in (A, B):
        for key in L.keys:
            b = L.file_bytes(key)
            fb.setdefault(key, []).append(b)
            if key not in objs or L is B:
                objs[key] = Obj(b)
    # objs: variant's view for files in B; base's for files only in A
    objsA = {k: (Obj(fb[k][0]) if (k in B.keys and len(fb[k]) == 2 and fb[k][0] != fb[k][1]) else objs[k]) for k in A.keys}
    for L, ob in ((A, objsA), (B, objs)):
        L.nobits = {u for u in L.units if u[1] == 'p' and ob[u[0]].S[u[2]]['type'] == 8}
    changed = {k for k in A.keys ^ B.keys} | {k for k in A.keys & B.keys if fb[k][0] != fb[k][1]}
    unch = (A.keys & B.keys) - changed

    pred = collections.defaultdict(set)   # unit -> rows
    why = collections.OrderedDict()

    def add(u, row):
        if row not in drop:
            pred[u].add(row)

    allunits = set(B.units) | set(A.units)
    by_file = collections.defaultdict(set)
    for u in allunits:
        by_file[u[0]].add(u)

    # referrer indexes over both links' units (unchanged files: same inputs)
    ref_global = collections.defaultdict(set)
    ref_own = collections.defaultdict(set)  # (key, sec) -> units of key naming sec
    slot_users = set()
    for L, ob in ((A, objsA), (B, objs)):
        for u in L.units:
            key, kind, sec = u
            g, own, slot = ob[key].names_by_sec(sec)
            if slot:
                slot_users.add(u)
            for n in g:
                ref_global[n].add(u)
            for s in own:
                ref_own[(key, s)].add(u)

    def nonalloc(key):
        return {u for u in by_file[key] if u[1] == 'n'}

    def eh(key, ob):
        return {u for u in by_file[key] if u[1] == 'e' and ob[key].S[u[2]]['name'].startswith('.eh_frame')}

    # row changed
    for k in changed:
        for u in by_file[k]:
            add(u, 'changed')
    why['changed'] = len(changed)

    # row tls
    ta, tb = A.tls_offsets(objsA), B.tls_offsets(objs)
    tls_moves = [(k, ta[k], tb[k]) for k in ta.keys() & tb.keys() if ta[k] != tb[k]]
    tls_fallback = bool(tls_moves) and 'tls' not in drop

    # row global (by definition identity; see gview)
    ra, rb = resolution(A, objsA), resolution(B, objs)
    gv = lambda r, n: gview(r[n], changed) if n in r else None
    gch = sorted(n for n in ra.keys() | rb.keys() if gv(ra, n) != gv(rb, n))
    for n in gch:
        for u in ref_global.get(n, ()):
            add(u, 'global')
    why['global'] = len(gch)

    # row symattr (IN-26 v1): a global's type, binding, st_other or kind
    # (absolute / common / section) changed and an UNCHANGED file
    # references it -> full-link fallback symbol-kind-changed
    refd = set()
    for key in unch:
        o = objs[key]
        refd |= {y['name'] for y in o.syms if y['bind'] != 0 and y['name'] and y['shn'] == 0}
        for sec in o.rels:
            refd |= o.names_by_sec(sec)[0]
    ach = sorted(n for n in ra.keys() & rb.keys() if aview(ra[n]) != aview(rb[n]) and n in refd)
    symattr_fallback = bool(ach) and 'symattr' not in drop

    # row comdat
    ka, kb = keepers(A, objsA), keepers(B, objs)
    kch = sorted(s for s in ka.keys() | kb.keys() if ka.get(s) != kb.get(s))
    for sig in kch:
        old, new = ka.get(sig), kb.get(sig)
        names = set()
        for key, ob in ((old, objsA), (new, objs)):
            if key is None:
                continue
            for m in ob[key].groups.get(sig, ()):
                names |= ob[key].globals_in(m)
                for kind in ('p', 'e'):
                    add((key, kind, m), 'comdat')
            for u in eh(key, ob) | nonalloc(key):
                add(u, 'comdat')
        for n in names:
            for u in ref_global.get(n, ()):
                add(u, 'comdat')
    why['comdat'] = len(kch)

    # row gc: liveness of non-COMDAT allocated sections of unchanged files
    gcs = []
    for key in unch:
        o = objs[key]
        la = {u[2] for u in A.units if u[0] == key and u[1] == 'p'}
        lb = {u[2] for u in B.units if u[0] == key and u[1] == 'p'}
        flip = {s for s in la ^ lb if s not in o.grouped}
        if flip:
            gcs.append((key, sorted(o.S[s]['name'] for s in flip)))
            for s in flip:
                for n in o.globals_in(s):
                    for u in ref_global.get(n, ()):
                        add(u, 'gc')
            for u in eh(key, objs) | nonalloc(key):
                add(u, 'gc')
    why['gc'] = len(gcs)

    # classify units of unchanged files; row moved
    same_diff, moved, appear = [], [], []
    for u in sorted(allunits, key=lambda u: (short(u[0]), u[1], u[2])):
        if u[0] not in unch:
            continue
        if (u in A.units) != (u in B.units):
            appear.append(u)
            continue
        if A.units[u] != B.units[u]:
            moved.append(u)
        elif A.unit_bytes(u) != B.unit_bytes(u):
            same_diff.append(u)
    for u in moved:
        key, kind, sec = u
        add(u, 'moved')
        for r in ref_own.get((key, sec), ()):
            add(r, 'moved')
        for n in objs[key].globals_in(sec):
            for r in ref_global.get(n, ()):
                add(r, 'moved')
        if kind == 'p':   # an allocated section moved: debug info names it
            for r in nonalloc(key):
                add(r, 'moved')
    why['moved'] = len(moved)

    # row frag: merged non-alloc fragments whose output offset changed
    nfrag = 0
    for u in A.units:
        if u[0] not in unch or u[1] != 'n' or u not in B.units or A.units[u] != B.units[u]:
            continue
        x, y = A.unit_bytes(u), B.unit_bytes(u)
        if x == y:
            continue
        if any(x[o:o + w] != y[o:o + w] for o, w in objs[u[0]].merged_fields(u[2])):
            add(u, 'frag'); nfrag += 1

    # row synth (IN-27 "a synthetic table changed"; in a full-link
    # comparison: a slot table moved): its slot users; IFUNC callers
    sch = sorted(n for n in A.chunks.keys() | B.chunks.keys()
                 if n in ('.got', '.got.plt', '.plt', '.iplt') and A.chunks.get(n) != B.chunks.get(n))
    if sch:
        for u in slot_users:
            add(u, 'synth')
        for n, v in rb.items():
            if v[4] == 10:
                for u in ref_global.get(n, ()):
                    add(u, 'synth')

    fallback = tls_fallback or symattr_fallback

    def okp(u):
        return fallback or bool(pred.get(u))

    # moved units: compare by value after undoing the move (N4); one that
    # differs beyond the move must be predicted by a row other than `moved`
    # (an update re-applies a moved unit's relocations with its stored
    # decisions: the moved row accounts for the move only)
    VA, VB = Valuer(A, objsA, ra), Valuer(B, objs, rb)
    fstats = collections.Counter()
    mv_same, mv_diff = [], []
    for u in moved:
        w = unit_by_value(A, B, VA, VB, objs[u[0]], u, fstats)
        (mv_diff if w else mv_same).append((u, w))

    def okm(u):
        return fallback or bool(pred.get(u, set()) - {'moved'})

    # bytes outside units, per output section
    cover = collections.defaultdict(set)
    for L in (A, B):
        for u, rs in L.ranges.items():
            for (sn, o, z) in rs:
                if o is not None:
                    cover[sn].add((o, z))
    outside = []
    for s in B.OS:
        if s['type'] == 8 or not s['name']:
            continue
        sa = A.osec(s['name'])
        if sa is None:
            outside.append((s['name'], 'new section', s['size'])); continue
        x = A.out[sa['off']:sa['off'] + sa['size']]
        y = B.out[s['off']:s['off'] + s['size']]
        if x == y and sa['addr'] == s['addr']:
            continue
        mask = bytearray(max(len(x), len(y)))
        for (o, z) in cover.get(s['name'], ()):
            mask[o:o + z] = b'\1' * len(mask[o:o + z])
        n = 0
        for i in range(0, min(len(x), len(y)), 256):
            if x[i:i + 256] != y[i:i + 256]:
                n += sum(1 for j in range(i, min(i + 256, len(x), len(y))) if x[j] != y[j] and not mask[j])
        n += abs(len(x) - len(y))
        if n or sa['addr'] != s['addr']:
            outside.append((s['name'], 'addr %x->%x size %x->%x' % (sa['addr'], s['addr'], sa['size'], s['size'])
                            if (sa['addr'], sa['size']) != (s['addr'], s['size']) else '', n))

    def uname(u):
        key, kind, sec = u
        ob = objs if key in objs else objsA
        return '%s:%s[%s]' % (short(key), ob[key].S[sec]['name'], {'p': 'alloc', 'n': 'nonalloc', 'e': 'pieces'}[kind])

    def guess_row(u):
        key, kind, sec = u
        if any(u in ref_global.get(n, ()) for n in gch): return 'global'
        if kch: return 'comdat'
        if gcs: return 'gc'
        if tls_moves: return 'tls'
        if sch and u in slot_users: return 'synth'
        if u[1] == 'n' and objs[u[0]].merged_fields(u[2]): return 'frag?'
        if moved: return 'moved'
        return '?'

    nd_pred = [u for u in same_diff if okp(u)]
    nd_not = [u for u in same_diff if not okp(u)]
    mv_not = [(u, w) for u, w in mv_diff if not okm(u)]
    predicted = {u for u in pred} if not fallback else allunits
    fail = bool(nd_not or mv_not)
    print('== %s -> %s  (dropped rows: %s)' % (args[0], args[1], ','.join(sorted(drop)) or 'none'))
    print('changed files:', ', '.join(sorted(short(k) for k in changed)) or 'none')
    print('causes: globals changed %d %s; comdat keepers changed %d %s; gc flips %d %s; moved units %d; TLS TP-offset changes %d'
          % (len(gch), gch[:6], len(kch), kch, len(gcs), [(short(k), s) for k, s in gcs][:4], len(moved), len(tls_moves)))
    print('units naming a non-alloc merged fragment that moved (frag): %d' % nfrag)
    print('slot tables moved/resized:', ', '.join('%s %s->%s' % (n, fmtc(A.chunks.get(n)), fmtc(B.chunks.get(n))) for n in sch) or 'none')
    for k, a, b in tls_moves:
        print('  TLS %s:%s tpoff %#x -> %#x' % (short(k[0]), objs[k[0]].S[k[1]]['name'], a, b))
    if tls_fallback:
        print('  -> predictor: FALLBACK tls-layout-changed (full link)')
    print('symbol attributes changed for keys an unchanged file references: %d %s' % (len(ach), [(n, aview(ra[n]), aview(rb[n])) for n in ach[:4]]))
    if symattr_fallback:
        print('  -> predictor: FALLBACK symbol-kind-changed (full link)')
    print('predicted refill set: %d units' % len(predicted))
    print('same-place differing units of unchanged files: %d (predicted %d, NOT predicted %d)' % (len(same_diff), len(nd_pred), len(nd_not)))
    for u in nd_pred[: (0 if quiet else 12)]:
        print('   pred  %s  by %s' % (uname(u), ','.join(sorted(pred[u])) or 'tls-fallback'))
    for u in nd_not:
        print('   MISS  %s  (row that should catch it: %s)' % (uname(u), guess_row(u)))
    print('moved units of unchanged files: %d; equal by value after the move %d; differing beyond the move %d (predicted %d, NOT predicted %d); appeared/disappeared %d'
          % (len(moved), len(mv_same), len(mv_diff), len(mv_diff) - len(mv_not), len(mv_not), len(appear)))
    print('  relocated fields of moved units: residual checked %d, slot/IPLT (by symbol) %d [of which per-site relaxation flips %d, in %d units equal otherwise], no field %d, unknown type (masked) %d'
          % (fstats['checked'], fstats['slot'], fstats['relaxflip'], len({u for u, w in mv_same} & fstats.get('flipunits', set())), fstats['none'], fstats['unknown']))
    for u in moved[: (0 if quiet else 4)]:
        print('   moved %s %s -> %s' % (uname(u), fmtpl(A.units[u]), fmtpl(B.units[u])))
    for u, w in mv_diff[: (0 if quiet else 8)]:
        if okm(u):
            print('   by-value-diff pred %s  by %s: %s' % (uname(u), ','.join(sorted(pred[u] - {'moved'})) or 'fallback', w))
    for u, w in mv_not:
        print('   MOVED-MISS %s: %s' % (uname(u), w))
    in_place = sum(1 for u in allunits if u[0] in unch and u in A.units and u in B.units and A.units[u] == B.units[u])
    print('SUMMARY in-place %d  by-value %d  differing-predicted %d  differing-unpredicted %d'
          % (in_place, len(moved), len(nd_pred) + len(mv_diff) - len(mv_not), len(nd_not) + len(mv_not)))
    for u in appear[: (0 if quiet else 8)]:
        print('   %s %s' % ('appeared' if u in B.units else 'gone', uname(u)))
    print('differing bytes outside units:', '; '.join('%s %d%s' % (n, c, (' (' + d + ')') if d else '') for n, d, c in outside) or 'none')
    print('RESULT %s  (%.2fs)' % ('FAIL' if fail else 'PASS', time.time() - T0))
    sys.exit(1 if fail else 0)


def fmtc(c):
    return '%x+%x' % c if c else '-'


def fmtpl(p):
    if len(p) == 2 and isinstance(p[0], int):
        return '%x+%x' % p
    if isinstance(p[0], str):
        return '%s@%x+%x' % p
    return '%x..(%d pieces)' % (p[0][1], len(p))


if __name__ == '__main__':
    main()
