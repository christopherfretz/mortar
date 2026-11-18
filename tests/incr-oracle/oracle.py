#!/usr/bin/env python3
"""oracle.py BASE_DIR VARIANT_DIR REPORT [--drop ROWS]

The two-full-link oracle of spec/incr.md IN-63a, judging mortar's dry run
(WP-063, `--incremental-report`). Each DIR holds a full mortar link:
DIR/prog, DIR/prog.map (--mortar-map) and the inputs at the paths the map
names (relative paths resolve against DIR). REPORT is the variant link's
`--incremental-report` file: its `delta unit`, `delta hole`, `delta chunk`
and `delta fallback` lines are the prediction under test.

Adapted from WP-060's refill oracle (R3, `.build/review-incr/oracle.py`,
56 of 56 pairs): its ELF and map readers, its units and its by-value
comparison of moved units (unit_by_value) are kept; its own predictor is
replaced by the report. A file is identified across the two links by
(map path, member name, ordinal among the archive's members of that name)
(WP-067 §7.2, applied by WP-223: an earlier member's growth moves every
later member, which stays the same file, as mortar's change detection,
IN-24, and its report see it); it is CHANGED when its bytes differ, it is
missing in one link, or it is extracted in only one. A thin archive's
member is read from the file its header names.

Units: `place` (allocated input section), `nplace` (non-allocated, offset
in the output section) and `piece` groups (a file's contributions of one
input section to .eh_frame / merged allocated sections, as a whole). For
every unit of an UNCHANGED file:
  present in both, same placement, bytes differ  -> in the report's units;
  present in both, placement differs ("moved")   -> in the report's units;
    and if it differs beyond the move (unit_by_value) with a reason other
    than `moved`;
  present only in the variant                     -> in the report's units;
  present only in the base                        -> in the report's holes;
and every output section with bytes that differ outside every unit (or
that moved) must have a `delta chunk` line naming it. A correctness
fall-back (`delta fallback`) passes trivially: the update links in full.

--drop ROWS removes the named rows (IN-63b; the names and reasons of
crates/mortar_incr/src/delta.rs ROWS) from the report before judging.
Prints RESULT PASS/FAIL and an ORACLE line: the differing units, the
report's units of unchanged files, and their ratio.
"""
import os, sys, struct, collections, time

T0 = time.time()
# IN-63b's rows: crates/mortar_incr/src/delta.rs ROWS (name, reasons,
# fall-backs). Keep the two in step; `cargo test -p mortar --test
# incr_oracle` checks the names against the report's.
ROWS = {
    'changed': ({'file'}, set()),
    'gone': ({'gone'}, set()),
    'global': ({'global'}, set()),
    'comdat': ({'comdat', 'comdat-file'}, set()),
    'slots': ({'slots'}, set()),
    'moved': ({'moved', 'moved-ref', 'moved-file'}, set()),
    'frag': ({'frag'}, set()),
    'synthetic': ({'synthetic'}, set()),
    'patch': ({'patch'}, set()),
    'liveness': ({'liveness', 'liveness-file'}, set()),
    'eh-frame': ({'eh-frame'}, set()),
    'eh-ref': ({'eh-ref'}, set()),
    'chunks': ({'headers', 'gaps'}, set()),
    'fallbacks': (set(), {'tls-layout-changed', 'symbol-kind-changed', 'new-output-section',
                          'property-changed', 'link-order-moved', 'inputs-reordered', 'options-changed'}),
}
MOVE_ONLY = {'moved', 'moved-ref', 'moved-file'}
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
        self.holes = []        # (outsec name, start off, len): `hole`/`dead` lines
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
            elif t[0] in ('hole', 'dead'):
                self.holes.append((t[1], int(t[2], 16), int(t[3], 16)))
        for k, ps in pieces.items():
            ps.sort()
            self.units[k] = tuple(ps)
            self.ranges[k] = [self.at_addr(a, z) for (_i, a, z) in ps]
        self.by_name()
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

    def by_name(self):
        """Re-keys every archive member from (path, member, offset) to
        (path, member, ordinal among the members of that name), keeping the
        offset in real_offset for reading (WP-067 §7.2 patch 2)."""
        seen, remap, self.real_offset = {}, {}, {}
        for fid in sorted(self.files):
            path, mem, off = self.files[fid]
            if mem is None:
                continue
            n = seen.get((path, mem), 0)
            seen[(path, mem)] = n + 1
            new = (path, mem, n)
            remap[(path, mem, off)] = new
            self.real_offset[new] = off
        if not remap:
            return
        rk = lambda k: remap.get(k, k)
        self.files = {i: rk(k) for i, k in self.files.items()}
        self.units = {(rk(k), kind, sec): v for (k, kind, sec), v in self.units.items()}
        self.ranges = {(rk(k), kind, sec): v for (k, kind, sec), v in self.ranges.items()}
        self.discard = {(rk(k), sec) for (k, sec) in self.discard}

    def file_bytes(self, key):
        path, mem, off = key
        p = path if os.path.isabs(path) else os.path.join(self.d, path)
        if mem is not None:
            off = self.real_offset.get(key, off)
            member = thin_member(p, off)
            if member is not None:
                with open(member, 'rb') as f:
                    return f.read()
        with open(p, 'rb') as f:
            if mem is None:
                return f.read()
            f.seek(off); h = f.read(60)
            return f.read(int(h[48:58].decode().strip()))


def thin_member(archive, off):
    """The file a thin archive's member header at `off` names (relative to
    the archive), or None for a regular archive (WP-067 §7.2 patch 1: a
    thin archive holds only the members' headers)."""
    with open(archive, 'rb') as f:
        if f.read(8) != b'!<thin>\n':
            return None
        f.seek(off)
        name = f.read(16).decode('latin1').rstrip()
        if name.startswith('/') and name[1:].isdigit():
            f.seek(8)
            while True:          # the `//` long-name table
                h = f.read(60)
                if len(h) < 60:
                    return None
                size = int(h[48:58].decode().strip())
                if h[:3] == b'// ':
                    table = f.read(size).decode('latin1')
                    at = int(name[1:])
                    name = table[at:table.index('\n', at)]
                    break
                if h[:1] == b'/':
                    f.seek(size + (size & 1), 1)
                else:
                    return None
        name = name.rstrip('/')
    return os.path.join(os.path.dirname(archive), name)

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
def unfield(t):
    """The report's field encoding (docs/cli.md 3.13): '-' is empty, %XX a byte."""
    if t == '-':
        return ''
    out, i = bytearray(), 0
    b = t.encode()
    while i < len(b):
        if b[i] == 0x25 and i + 2 < len(b):
            out.append(int(b[i + 1:i + 3], 16)); i += 3
        else:
            out.append(b[i]); i += 1
    return out.decode('utf-8', 'surrogateescape')


# The reasons after which the dry run runs (spec/incr.md IN-10a): a state
# an update could start from.
DRY_RUN_REASONS = (None, 'update-not-implemented', 'small-output')


class Report:
    """The dry run's lines that make a prediction."""
    def __init__(self, path):
        self.units, self.holes, self.chunks, self.fallbacks, self.files = [], [], [], [], []
        self.policy = None
        self.ended = False
        self.elided = []
        self.reason = None
        for line in open(path):
            t = line.split()
            if len(t) >= 3 and t[0] == 'incremental:' and t[1] == 'full':
                self.reason = t[2].strip('()')
            if len(t) < 2 or t[0] != 'delta':
                continue
            if t[1] == 'unit':      # reasons path member shndx name
                self.units.append((set(t[2].split(',')), unfield(t[3]), unfield(t[4]) or None, int(t[5])))
            elif t[1] == 'hole':    # reasons path member shndx|- addr|- size name
                self.holes.append((set(t[2].split(',')), unfield(t[3]), unfield(t[4]) or None,
                                   None if t[5] == '-' else int(t[5])))
            elif t[1] == 'chunk':   # reasons tag name outsec
                self.chunks.append((set(t[2].split(',')), t[3], unfield(t[4]), unfield(t[5])))
            elif t[1] == 'fallback':
                self.fallbacks.append(t[2])
            elif t[1] == 'file':    # state path member
                self.files.append((t[2], unfield(t[3]), unfield(t[4]) or None))
            elif t[1] == 'policy':
                self.policy = t[2]
            elif t[1] == 'elided':  # kind listed=N total=N (WP-106's bound)
                self.elided.append(' '.join(t[2:]))
            elif t[1] == 'end':
                self.ended = True

    def view(self, drop):
        """The prediction with the rows in `drop` removed (IN-63b)."""
        reasons, falls = set(), set()
        for d in drop:
            if d in ROWS:
                reasons |= ROWS[d][0]; falls |= ROWS[d][1]
            else:
                falls.add(d)
        units = collections.defaultdict(set)
        for r, p, m, i in self.units:
            if r - reasons:
                units[(p, m, i)] |= r - reasons
        holes = set()
        for r, p, m, i in self.holes:
            if r - reasons:
                holes.add((p, m, i))
        chunks = collections.defaultdict(set)   # output section -> chunk tags
        for r, tag, name, out in self.chunks:
            if r - reasons:
                # a synthetic chunk has no OUTSECS row: its name is its section's
                chunks[out or name].add(tag)
        fb = [f for f in self.fallbacks if f not in falls]
        return units, holes, chunks, fb


class Facts:
    """What two full links say, computed once per pair."""
    def __init__(self, da, db):
        A, B = Link(da), Link(db)
        self.A, self.B = A, B
        objs, fb = {}, {}
        for L in (A, B):
            for key in L.keys:
                b = L.file_bytes(key)
                fb.setdefault(key, []).append(b)
                if key not in objs or L is B:
                    objs[key] = Obj(b)
        objsA = {k: (Obj(fb[k][0]) if (k in B.keys and len(fb[k]) == 2 and fb[k][0] != fb[k][1]) else objs[k]) for k in A.keys}
        for L, ob in ((A, objsA), (B, objs)):
            L.nobits = {u for u in L.units if u[1] == 'p' and ob[u[0]].S[u[2]]['type'] == 8}
        self.objs, self.objsA = objs, objsA
        changed = {k for k in A.keys ^ B.keys} | {k for k in A.keys & B.keys if fb[k][0] != fb[k][1]}
        self.changed = changed
        unch = (A.keys & B.keys) - changed
        self.unch = unch
        allunits = set(B.units) | set(A.units)
        self.same_diff, self.moved, self.appear, self.vanish = [], [], [], []
        for u in sorted(allunits, key=lambda u: (short(u[0]), u[1], u[2])):
            if u[0] not in unch:
                continue
            if (u in A.units) != (u in B.units):
                (self.appear if u in B.units else self.vanish).append(u)
                continue
            if A.units[u] != B.units[u]:
                self.moved.append(u)
            elif A.unit_bytes(u) != B.unit_bytes(u):
                self.same_diff.append(u)
        ra, rb = resolution(A, objsA), resolution(B, objs)
        VA, VB = Valuer(A, objsA, ra), Valuer(B, objs, rb)
        self.fstats = collections.Counter()
        self.mv_diff = []
        for u in self.moved:
            w = unit_by_value(A, B, VA, VB, objs[u[0]], u, self.fstats)
            if w:
                self.mv_diff.append((u, w))
        self.changed_units = sorted((u for u in B.units if u[0] in changed), key=lambda u: (short(u[0]), u[1], u[2]))
        self.unch_units = {u for u in B.units if u[0] in unch}
        self.left_units = sorted((u for u in A.units if u[0] not in B.keys and u[1] in 'pn'
                                  and self.size(A, u)), key=lambda u: (short(u[0]), u[1], u[2]))
        # bytes outside every unit, per output section (as R3)
        cover = collections.defaultdict(set)
        for L in (A, B):
            for u, rs in L.ranges.items():
                for (sn, o, z) in rs:
                    if o is not None:
                        cover[sn].add((o, z))
        self.outside = []
        for s in B.OS:
            if s['type'] == 8 or not s['name']:
                continue
            sa = A.osec(s['name'])
            if sa is None:
                self.outside.append((s['name'], 'new section', s['size'], s['size'])); continue
            x = A.out[sa['off']:sa['off'] + sa['size']]
            y = B.out[s['off']:s['off'] + s['size']]
            if x == y and sa['addr'] == s['addr']:
                continue
            mask = bytearray(max(len(x), len(y)))
            for (o, z) in cover.get(s['name'], ()):
                mask[o:o + z] = b'\1' * len(mask[o:o + z])
            # the tail: past the new link's last unit or hole in the section,
            # where mortar's own chunks (patches, thunks) sit; a `body` chunk
            # (fill between units) does not name those bytes. A hole or dead
            # range (an incremental-mode layout's slack, WP-062) is the body
            # chunk's: its bytes are rewritten with the body's fill.
            last = max((o + z for L in (B,) for rs in L.ranges.values()
                        for (sn, o, z) in rs if sn == s['name'] and o is not None), default=0)
            last = max([last] + [o + z for (sn, o, z) in B.holes if sn == s['name']])
            n = tail = 0
            for i in range(0, min(len(x), len(y)), 256):
                if x[i:i + 256] != y[i:i + 256]:
                    for j in range(i, min(i + 256, len(x), len(y))):
                        if x[j] != y[j] and not mask[j]:
                            n += 1
                            tail += j >= last
            n += abs(len(x) - len(y))
            tail += max(0, len(y) - max(len(x), last))
            if n or sa['addr'] != s['addr']:
                self.outside.append((s['name'], 'addr %x->%x size %x->%x' % (sa['addr'], s['addr'], sa['size'], s['size']), n, tail))

    @staticmethod
    def size(L, u):
        pl = L.units[u]
        return pl[1] if u[1] == 'p' else pl[2]

    def differing(self):
        return len(self.same_diff) + len(self.moved) + len(self.appear)

    def uname(self, u):
        key, kind, sec = u
        ob = self.objs if key in self.objs else self.objsA
        return '%s:%s[%s]' % (short(key), ob[key].S[sec]['name'], {'p': 'alloc', 'n': 'nonalloc', 'e': 'pieces'}[kind])


def ukey(u):
    (path, mem, _off), _kind, sec = u
    return (path, mem, sec)


def judge(F, R, drop, quiet=True):
    """IN-63a: the prediction covers every difference. Returns (ok,
    predicted units of unchanged files, lines)."""
    if R.reason not in DRY_RUN_REASONS:
        # the state check sent the link to a full link before any delta
        return True, None, ['full link (%s) before the dry run' % R.reason]
    if R.elided:
        # the report's bound (REPORT_BOUND per kind) left lines out: the
        # prediction is not all there, so it cannot be judged
        return False, None, ['the report elided lines (%s): raise the bound or shrink the scenario' % '; '.join(R.elided)]
    units, holes, chunks, fb = R.view(drop)
    out = []
    unch_keys = {ukey(u) for u in F.unch_units}
    pred_unch = {k for k in units if k in unch_keys}
    if fb:
        return True, None, ['fallback %s: the update links in full' % ','.join(fb)]
    miss = []
    for u in F.same_diff:
        if ukey(u) not in units:
            miss.append('MISS same-place differing %s' % F.uname(u))
    for u in F.moved:
        if ukey(u) not in units:
            miss.append('MISS moved %s' % F.uname(u))
    for u, w in F.mv_diff:
        if not (units.get(ukey(u), set()) - MOVE_ONLY):
            miss.append('MISS moved and differing beyond the move %s: %s (reasons %s)'
                        % (F.uname(u), w, ','.join(sorted(units.get(ukey(u), ()))) or 'none'))
    for u in F.appear:
        if ukey(u) not in units:
            miss.append('MISS appeared %s' % F.uname(u))
    for u in F.vanish:
        if ukey(u) not in holes:
            miss.append('MISS vanished (no hole) %s' % F.uname(u))
    for u in F.changed_units:
        if ukey(u) not in units:
            miss.append('MISS unit of a changed file %s' % F.uname(u))
    # an old unit of a file that left: its bytes are dead (IN-27 row 2)
    left = {(p, m) for (p, m, i) in holes}
    for u in F.left_units:
        if (u[0][0], u[0][1]) not in left:
            miss.append('MISS unit of a file that left, no hole: %s' % F.uname(u))
    for name, what, n, tail in F.outside:
        if name not in chunks:
            miss.append('MISS output section %s differs outside units (%d bytes; %s) and no chunk names it' % (name, n, what))
        elif tail and not chunks[name] - {'body'}:
            miss.append('MISS output section %s differs past its last unit (%d bytes; %s) and only its body chunk is named'
                        % (name, tail, what))
    return not miss, len(pred_unch), miss


def ratio(pred, diff):
    if pred is None:
        return 'full'
    if diff == 0:
        return '1.00' if pred == 0 else 'inf'
    return '%.2f' % (pred / diff)


def main():
    args = [a for a in sys.argv[1:]]
    drops = []
    i = 0
    pos = []
    while i < len(args):
        if args[i] == '--drop':
            drops.append(set(x for x in args[i + 1].split(',') if x)); i += 2
        elif args[i] == '--mutations':
            drops = [set()] + [{r} for r in ROWS] + [{f} for f in sorted(ROWS['fallbacks'][1])]; i += 1
        else:
            pos.append(args[i]); i += 1
    if not drops:
        drops = [set()]
    F = Facts(pos[0], pos[1])
    R = Report(pos[2])
    if not R.ended and R.reason in DRY_RUN_REASONS[1:]:
        print('RESULT FAIL  (the report has no `delta end`)')
        sys.exit(1)
    fail = False
    for drop in drops:
        ok, npred, lines = judge(F, R, drop)
        tag = ','.join(sorted(drop)) or 'none'
        if not drop:
            print('changed files:', ', '.join(sorted(short(k) for k in F.changed)) or 'none')
            print('report fall-backs:', ','.join(R.fallbacks) or 'none', ' policy:', R.policy or 'none')
            print('ORACLE differing=%d (same-place %d, moved %d [beyond the move %d], appeared %d, vanished %d) predicted=%s ratio=%s outside=%s'
                  % (F.differing(), len(F.same_diff), len(F.moved), len(F.mv_diff), len(F.appear), len(F.vanish),
                     npred, ratio(npred, F.differing()), ';'.join(n for n, _w, _c, _t in F.outside) or 'none'))
            for l in lines[:40]:
                print('  ' + l)
            print('RESULT %s  (%.2fs)' % ('PASS' if ok else 'FAIL', time.time() - T0))
            fail |= not ok
        else:
            # a mutation is caught when the judgement fails without the row
            print('MUTATION %s %s%s' % (tag, 'caught' if not ok else 'survives', (' (' + lines[0] + ')') if lines and not ok else ''))
    sys.exit(1 if fail else 0)


if __name__ == '__main__':
    main()
