#!/usr/bin/env python3
"""xref.py OBJ...: WP-229 ruling 9. Can a relocation in one file name
another file's debug section? Only through a GLOBAL or WEAK symbol defined
in a debug section (a relocation names symbols of its own file). Counts,
over the objects given (ELF64 LE relocatables): symbols defined in a
non-alloc .debug_* section by binding, and relocations in .rela.debug_*
sections whose symbol is undefined (they name another file; the section
it is defined in is printed by name when another input defines it).
"""
import struct, sys, collections

def parse(path):
    b = open(path, 'rb').read()
    if b[:4] != b'\x7fELF' or b[4] != 2 or struct.unpack_from('<H', b, 16)[0] != 1:
        return None
    shoff, = struct.unpack_from('<Q', b, 0x28)
    ent, n, stx = struct.unpack_from('<HHH', b, 0x3a)
    sh = [struct.unpack_from('<IIQQQQIIQQ', b, shoff + i * ent) for i in range(n)]
    so = sh[stx][4]
    name = lambda o, base: b[base + o:b.index(b'\0', base + o)].decode()
    names = [name(s[0], so) for s in sh]
    syms, symstr = [], None
    for i, s in enumerate(sh):
        if s[1] == 2:
            stro = sh[s[6]][4]
            for k in range(s[5] // 24):
                nm, info, oth, shndx, val, sz = struct.unpack_from('<IBBHQQ', b, s[4] + k * 24)
                syms.append((name(nm, stro), info >> 4, shndx))
    relocs = []
    for i, s in enumerate(sh):
        if s[1] == 4 and names[s[7]].startswith('.debug_'):
            for k in range(s[5] // 24):
                off, info, add = struct.unpack_from('<QQq', b, s[4] + k * 24)
                relocs.append((names[s[7]], syms[info >> 32] if (info >> 32) < len(syms) else None))
    return names, sh, syms, relocs

defs = collections.Counter(); undef_relocs = collections.Counter(); globals_in_debug = {}
objs = 0
for p in sys.argv[1:]:
    r = parse(p)
    if not r: continue
    objs += 1
    names, sh, syms, relocs = r
    for nm, bind, shndx in syms:
        if 0 < shndx < len(names) and names[shndx].startswith('.debug_') and not sh[shndx][2] & 2:
            defs[('LOCAL', 'GLOBAL', 'WEAK')[bind] if bind < 3 else str(bind)] += 1
            if bind in (1, 2): globals_in_debug[nm] = (p, names[shndx])
    for sec, sym in relocs:
        if sym and sym[2] == 0 and sym[0]:
            undef_relocs[sec] += 1
print('objects=%d symbols defined in debug sections by binding: %s' % (objs, dict(defs)))
print('global/weak symbols in debug sections: %d %s' % (len(globals_in_debug), list(globals_in_debug.items())[:5]))
print('relocations in debug sections naming an undefined symbol, by section: %s' % dict(undef_relocs))
