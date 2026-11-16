# o24.py: the prototype of spec/odr.md O-24 and O-50..O-52 (WP-075, scope item 1).
# No linker code: it reads what GNU ld or lld wrote, and the units the test producer
# (`cargo xtask meta`) put into the input objects, read back with `cargo xtask meta dump`.
#
#   o24.py note COUNT                           a .s placeholder note of COUNT rows (O-52)
#   o24.py rows SO [--bsf] OBJ...               the number of rows O-50 gives SO
#   o24.py patch SO [--bsf] OBJ...              write O-51's rows into SO's placeholder note
#   o24.py table FILE                           print the rows of FILE's table (PT_NOTE walk)
#   o24.py check EXE [--literal|--amended] OBJ... -- SO...   O-24 (a) and (b)
#
# OBJ are the objects the module was linked from, in link order (their units give the
# keeper's kind 2 rows: the first object, in link order, whose unit has a row for the key
# and which defines the name). --bsf: the module was linked -Bsymbolic-functions (no
# dynamic tag records it). --literal applies O-24's identity as written (a version node
# is a name or "unversioned"; unversioned and a node are different entities);
# --amended applies the binding rule glibc implements (see the WP-075 record).
import struct, subprocess, sys, os

XTASK = os.environ.get('XTASK', 'xtask')
OWNER = b'MasonMeta\0'
NT_MASON_ODR = 0x4d4f4401
NONPREEMPTIBLE = 0x8000
VAGUE, EXPLICIT_SPEC = 0x1, 0x8
DT_NEEDED, DT_SONAME, DT_SYMBOLIC, DT_FLAGS = 1, 14, 16, 30
DF_SYMBOLIC = 0x2


def fnv(name):
    h = 0xcbf29ce484222325
    for c in name:
        h = ((h ^ c) * 0x100000001b3) & 0xffffffffffffffff
    return h


class Elf:
    def __init__(self, path):
        self.path = path
        b = self.b = bytearray(open(path, 'rb').read())
        assert b[:4] == b'\x7fELF' and b[5] == 1, path
        self.c64 = b[4] == 2
        if self.c64:
            f = struct.unpack_from('<HHIQQQIHHHHHH', b, 16)
        else:
            f = struct.unpack_from('<HHIIIIIHHHHHH', b, 16)
        (self.type, _, _, _, phoff, shoff, _, _, phent, phnum, shent, shnum, shstrndx) = f
        self.secs = []
        for i in range(shnum):
            o = shoff + i * shent
            s = struct.unpack_from('<IIQQQQIIQQ' if self.c64 else '<IIIIIIIIII', b, o)
            self.secs.append(dict(name=s[0], type=s[1], flags=s[2], addr=s[3], off=s[4],
                                  size=s[5], link=s[6], info=s[7]))
        for s in self.secs:
            s['name'] = self.cstr(self.secs[shstrndx]['off'] + s['name']).decode()
        self.phdrs = []
        for i in range(phnum):
            o = phoff + i * phent
            if self.c64:
                t, fl, off, va, pa, fs, ms, al = struct.unpack_from('<IIQQQQQQ', b, o)
            else:
                t, off, va, pa, fs, ms, fl, al = struct.unpack_from('<IIIIIIII', b, o)
            self.phdrs.append(dict(type=t, off=off, vaddr=va, filesz=fs, align=al))

    def cstr(self, o):
        return bytes(self.b[o:self.b.index(0, o)])

    def sec(self, name):
        return next((s for s in self.secs if s['name'] == name), None)

    def dynamic(self):
        d = self.sec('.dynamic')
        if d is None:
            return []
        ent = 16 if self.c64 else 8
        out = []
        for i in range(d['size'] // ent):
            t, v = struct.unpack_from('<qQ' if self.c64 else '<iI', self.b, d['off'] + i * ent)
            if t == 0:
                break
            out.append((t, v))
        return out

    def dynstr(self, off):
        return self.cstr(self.sec('.dynstr')['off'] + off)

    def needed(self):
        return [self.dynstr(v) for t, v in self.dynamic() if t == DT_NEEDED]

    def soname(self):
        s = [self.dynstr(v) for t, v in self.dynamic() if t == DT_SONAME]
        return s[0] if s else os.path.basename(self.path).encode()

    def symbolic(self):
        return any(t == DT_SYMBOLIC or (t == DT_FLAGS and v & DF_SYMBOLIC) for t, v in self.dynamic())

    def verdefs(self):
        """vd_ndx -> (name, is_base)"""
        out = {}
        s = self.sec('.gnu.version_d')
        if s is None:
            return out
        o = s['off']
        while True:
            ver, flags, ndx, cnt, h, aux, nxt = struct.unpack_from('<HHHHIII', self.b, o)
            name_off, _ = struct.unpack_from('<II', self.b, o + aux)
            out[ndx] = (self.dynstr(name_off), bool(flags & 1))
            if nxt == 0:
                break
            o += nxt
        return out

    def dynsyms(self):
        ds = self.sec('.dynsym')
        vs = self.sec('.gnu.version')
        vd = self.verdefs()
        ent = 24 if self.c64 else 16
        out = []
        for i in range(ds['size'] // ent):
            o = ds['off'] + i * ent
            if self.c64:
                nm, info, oth, shn, val, sz = struct.unpack_from('<IBBHQQ', self.b, o)
            else:
                nm, val, sz, info, oth, shn = struct.unpack_from('<IIIBBH', self.b, o)
            versym = struct.unpack_from('<H', self.b, vs['off'] + 2 * i)[0] if vs else 1
            ndx = versym & 0x7fff
            node = None  # unversioned: no .gnu.version, VER_NDX_GLOBAL, or the base node
            if ndx >= 2 and ndx in vd and not vd[ndx][1]:
                node = vd[ndx][0]
            out.append(dict(index=i, name=self.dynstr(nm), bind=info >> 4, type=info & 15,
                            vis=oth & 3, shndx=shn, versym=versym, node=node,
                            has_versym=vs is not None))
        return out

    def exports(self):
        """O-50: defined, STV_DEFAULT or STV_PROTECTED, not local (versym 0)."""
        return [s for s in self.dynsyms()[1:]
                if s['shndx'] != 0 and s['bind'] in (1, 2, 10) and s['vis'] in (0, 3)
                and s['versym'] != 0]

    def notes(self):
        """Every note of every PT_NOTE: (owner, type, desc offset, desc size)."""
        out = []
        for p in self.phdrs:
            if p['type'] != 4:
                continue
            o, end = p['off'], p['off'] + p['filesz']
            al = max(4, p['align'])
            while o + 12 <= end:
                nsz, dsz, typ = struct.unpack_from('<III', self.b, o)
                name = bytes(self.b[o + 12:o + 12 + nsz])
                d = o + ((12 + nsz + al - 1) // al) * al
                out.append((name, typ, d, dsz))
                o = d + ((dsz + al - 1) // al) * al
        return out

    def table_at(self):
        for name, typ, d, dsz in self.notes():
            if name == OWNER and typ == NT_MASON_ODR:
                return d, dsz
        return None

    def table(self):
        """O-51's rows, read back: [(key, hash8, dynsym, flags)]."""
        at = self.table_at()
        if at is None:
            return None
        d, dsz = at
        version, count, row_size, _ = struct.unpack_from('<IIII', self.b, d)
        assert version == 1 and row_size == 24 and 16 + count * 24 == dsz, 'malformed table'
        return [struct.unpack_from('<QQIH', self.b, d + 16 + 24 * i) for i in range(count)]

    def defines(self):
        """Names of the global/weak symbols an object defines (.symtab)."""
        st = self.sec('.symtab')
        strs = self.secs[st['link']]
        ent = 24 if self.c64 else 16
        out = set()
        for i in range(st['size'] // ent):
            o = st['off'] + i * ent
            if self.c64:
                nm, info, oth, shn, val, sz = struct.unpack_from('<IBBHQQ', self.b, o)
            else:
                nm, val, sz, info, oth, shn = struct.unpack_from('<IIIBBH', self.b, o)
            if shn != 0 and info >> 4 in (1, 2, 10):
                out.add(self.cstr(strs['off'] + nm))
        return out


def units(obj):
    """The kind 2 rows of an object's units: {key: (hash16, flags)} (first row per key)."""
    out = {}
    txt = subprocess.run([XTASK, 'meta', 'dump', obj], capture_output=True, text=True,
                         check=True).stdout
    for line in txt.splitlines():
        w = line.split()
        if not w or w[0] != 'definition':
            continue
        f = dict(x.split('=', 1) for x in w[1:] if '=' in x)
        out.setdefault(int(f['key'], 0), (bytes.fromhex(f['hash']), int(f['flags'], 0)))
    return out


def keeper_rows(objs):
    """key -> (hash16, flags, object): the first object in link order with a row that
    defines the name the key anchors to (O-51: the keeper's unit)."""
    out = {}
    for o in objs:
        defs = {fnv(n): n for n in Elf(o).defines()}
        for key, (h, fl) in units(o).items():
            if key in defs and key not in out:
                out[key] = (h, fl, o)
    return out


def rows_for(so, objs, bsf):
    e = Elf(so)
    keep = keeper_rows(objs)
    symbolic = e.symbolic()
    rows = []
    for s in e.exports():
        k = fnv(s['name'])
        if k not in keep or not keep[k][1] & (VAGUE | EXPLICIT_SPEC):
            continue
        h, fl, _ = keep[k]
        flags = fl & 0x7fff
        if s['vis'] == 3 or symbolic or (bsf and s['type'] == 2):
            flags |= NONPREEMPTIBLE
        rows.append((k, struct.unpack('<Q', h[:8])[0], s['index'], flags))
    rows.sort()
    return rows


def note_s(count):
    return ('\t.section .note.mason,"a",%%note\n\t.p2align 3\n\t.long 10\n\t.long %d\n'
            '\t.long 0x4d4f4401\n\t.asciz "MasonMeta"\n\t.p2align 3\n\t.long 1, %d, 24, 0\n'
            '\t.zero %d\n' % (16 + 24 * count, count, 24 * count))


def node_str(n):
    return n.decode() if n else '(unversioned)'


def check(exe, objs, libs, mode):
    """O-24 (a) and (b). Returns the findings as text lines."""
    e = Elf(exe)
    needed = e.needed()
    keep = keeper_rows(objs)
    exports = {(s['name'], s['node']): s for s in e.exports()}
    by_name = {}
    for (n, node), s in exports.items():
        by_name.setdefault(n, []).append(node)
    out = []
    scope = []
    for so in libs:
        l = Elf(so)
        if l.soname() not in needed:
            out.append('  not in scope: %s (not in DT_NEEDED)' % l.soname().decode())
            continue
        t = l.table()
        if t is None:
            out.append('  no table: %s' % l.soname().decode())
            continue
        syms = l.dynsyms()
        rows = []
        for key, h8, idx, flags in t:
            s = syms[idx]
            assert fnv(s['name']) == key, 'row key is not FNV-1a 64 of its .dynsym name'
            rows.append(dict(name=s['name'], node=s['node'], hash=h8, flags=flags,
                             lib=l.soname().decode(), order=needed.index(l.soname()),
                             has_versym=s['has_versym']))
        scope.append(rows)
    for n in e.needed():
        if not any(Elf(so).soname() == n for so in libs):
            out.append('  not read: %s (DT_NEEDED, not an input of this check)' % n.decode())

    def same_entity(a_node, b_node):
        if mode == 'literal':
            return a_node == b_node
        # amended: glibc's check_match. An unversioned definition satisfies a versioned
        # reference, and an unversioned reference binds the default version: only two
        # different node names are different entities.
        return a_node == b_node or a_node is None or b_node is None

    # (a) output against library
    defined_by_output = set()  # (name, node) the output defines and exports
    output_defines = set()     # every name a live object of the output defines
    for o in objs:
        output_defines |= Elf(o).defines()
    for rows in scope:
        for r in rows:
            for node in by_name.get(r['name'], []):
                if not same_entity(node, r['node']):
                    continue
                k = fnv(r['name'])
                if k not in keep:
                    continue
                defined_by_output.add((r['name'], r['node']))
                h8 = struct.unpack('<Q', keep[k][0][:8])[0]
                if h8 != r['hash']:
                    who = ('%s runs its own copy (NONPREEMPTIBLE); the output runs its own'
                           % r['lib']) if r['flags'] & NONPREEMPTIBLE else \
                          ('the output\'s copy runs for %s too' % r['lib'])
                    out.append('  odr-violation %s@%s / %s: defined in the output, defined '
                               'differently in %s; %s'
                               % (r['name'].decode(), node_str(node), node_str(r['node']),
                                  r['lib'], who))
    # (b) library against library: (name, version) the output does not define
    groups = {}
    for rows in scope:
        for r in rows:
            groups.setdefault(r['name'], []).append(r)
    for name, rs in sorted(groups.items()):
        rs.sort(key=lambda r: r['order'])
        for i in range(len(rs)):
            for j in range(i + 1, len(rs)):
                a, b = rs[i], rs[j]
                if a['lib'] == b['lib'] or not same_entity(a['node'], b['node']):
                    continue
                # as written: "a (name, version) that the output does not define" (hidden
                # definitions included: the output's objects define the name)
                if mode == 'literal' and name in output_defines:
                    continue
                if mode == 'amended' and any(nm == name for nm, _ in defined_by_output):
                    continue
                if a['hash'] == b['hash']:
                    continue
                if not a['flags'] & NONPREEMPTIBLE and not b['flags'] & NONPREEMPTIBLE:
                    who = 'the one earlier in DT_NEEDED order (%s) provides the definition ' \
                          'to both' % a['lib']
                else:
                    np = [x['lib'] for x in (a, b) if x['flags'] & NONPREEMPTIBLE]
                    who = '%s run(s) its own copy; the other binds the earlier one in ' \
                          'lookup order (%s)' % (', '.join(np), a['lib'])
                out.append('  odr-violation %s %s/%s: defined in %s, defined differently in '
                           '%s; %s' % (name.decode(), node_str(a['node']), node_str(b['node']),
                                       a['lib'], b['lib'], who))
    return out


def main(argv):
    cmd = argv[1]
    if cmd == 'note':
        sys.stdout.write(note_s(int(argv[2])))
        return 0
    if cmd in ('rows', 'patch'):
        so, rest = argv[2], argv[3:]
        bsf = '--bsf' in rest
        objs = [a for a in rest if a != '--bsf']
        rows = rows_for(so, objs, bsf)
        if cmd == 'rows':
            print(len(rows))
            return 0
        e = Elf(so)
        at = e.table_at()
        assert at is not None, 'no placeholder note in a PT_NOTE of ' + so
        d, dsz = at
        assert dsz == 16 + 24 * len(rows), 'placeholder of the wrong size'
        struct.pack_into('<IIII', e.b, d, 1, len(rows), 24, 0)
        for i, r in enumerate(rows):
            struct.pack_into('<QQIHH', e.b, d + 16 + 24 * i, r[0], r[1], r[2], r[3], 0)
        open(so, 'wb').write(e.b)
        return 0
    if cmd == 'table':
        e = Elf(argv[2])
        t = e.table()
        if t is None:
            print('no table')
            return 0
        syms = e.dynsyms()
        for key, h8, idx, flags in t:
            print('%016x %016x %s@%s flags=%#x' % (key, h8, syms[idx]['name'].decode(),
                                                  node_str(syms[idx]['node']), flags))
        return 0
    if cmd == 'check':
        exe = argv[2]
        mode = 'literal'
        rest = argv[3:]
        if rest and rest[0] in ('--literal', '--amended'):
            mode = rest[0][2:]
            rest = rest[1:]
        i = rest.index('--')
        for line in check(exe, rest[:i], rest[i + 1:], mode):
            print(line)
        return 0
    raise SystemExit('unknown command ' + cmd)


if __name__ == '__main__':
    sys.exit(main(sys.argv))
