#!/usr/bin/env python3
"""macros.py: what each DW_MACRO_import of a linked output names, judged
without a map (WP-249 step 1, item 2).

  macros.py judge OUTPUT TAG OBJ...
  macros.py patch OUTPUT NEW MODE OBJ...      MODE: redirect | zero | ones

OBJ... are the link's -g3 objects in link order. The output's .debug_macro
is cut into units (DWARF 5 / GNU v4 header, ops to the 0 op). A unit with
a DW_MACRO_start_file is a compile unit's; the compile units come in the
objects' order, one per object. Each object's own .debug_macro (not in a
group) holds its imports as relocations against a group member's section
symbol; their order in the object is their order in the output unit, so
each output import gets its group signature. The KEPT copy's offset of a
signature is what the first object having the group (the keeper) imports
for it (its own copy: relocated like any live target). A judged import of
a later object is then 'redirect' (the kept copy's offset + the addend),
'zero', 'ones', 'copy' (a unit
byte-identical to the kept copy, i.e. the linker kept a second copy) or
'other'. One line per output:
  TAG units=N cu=M imports=I keeper=K keeper_bad=B redirect=R zero=Z ones=O copy=C other=X
'patch' writes NEW: OUTPUT with every import of a dropped copy set to MODE.
"""
import os
import struct
import sys

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
from classes import Elf, GRP_COMDAT, SHT_GROUP  # noqa: E402


def uleb(b, p):
    v = s = 0
    while True:
        c = b[p]
        p += 1
        v |= (c & 0x7f) << s
        s += 7
        if c < 0x80:
            return v, p


def cstr(b, p):
    return b.index(b'\0', p) + 1


def units(b):
    """[(start, end, is_cu, [(field position, width)])] of a .debug_macro."""
    out = []
    p = 0
    while p < len(b):
        start = p
        ver, flags = struct.unpack_from('<HB', b, p)
        if ver not in (4, 5):
            raise ValueError('macro unit version %d at %#x' % (ver, p))
        p += 3
        w = 8 if flags & 1 else 4
        if flags & 2:
            p += w
        table = {}
        if flags & 4:
            n = b[p]
            p += 1
            for _ in range(n):
                op = b[p]
                k, p = uleb(b, p + 1)
                table[op] = b[p:p + k]
                p += k
        is_cu = False
        imports = []
        while True:
            op = b[p]
            p += 1
            if op == 0:
                break
            if op in (1, 2):
                _, p = uleb(b, p)
                p = cstr(b, p)
            elif op == 3:
                is_cu = True
                _, p = uleb(b, p)
                _, p = uleb(b, p)
            elif op == 4:
                pass
            elif op in (5, 6, 8, 9):
                _, p = uleb(b, p)
                p += w
            elif op in (7, 10):
                imports.append((p, w))
                p += w
            elif op in (11, 12):
                _, p = uleb(b, p)
                _, p = uleb(b, p)
            else:
                raise ValueError('macro op %#x at %#x (no table support)' % (op, p - 1))
        out.append((start, p, is_cu, imports))
    return out


def object_imports(path):
    """Per compile unit of the object's own (non-group) .debug_macro
    sections, in order: [(signature, addend, comdat)] of each import, by
    the relocation at its field (signature None: no relocation against a
    group member there, so not judged). An `ld -r` output has several."""
    elf = Elf(open(path, 'rb').read())
    syms = elf.symbols()
    member = {}
    for sig, flags, mem in elf.groups():
        for m in mem:
            member[m] = (sig, bool(flags & GRP_COMDAT))
    out = []
    for own in elf.secs:
        if own.name != '.debug_macro' or own.i in member or own.type == SHT_GROUP:
            continue
        at = {}
        for off, t, si, add in elf.relocs(own):
            shndx = syms[si][5]
            if shndx in member and elf.secs[shndx].name == '.debug_macro':
                at[off] = (member[shndx][0], (add or 0) + syms[si][1], member[shndx][1])
        for start, end, is_cu, fields in units(elf.contents(own)):
            if is_cu:
                out.append([at.get(pos, (None, 0, False)) for pos, w in fields])
    return out


def plan(output, objs):
    elf = Elf(open(output, 'rb').read())
    sec = next(s for s in elf.secs if s.name == '.debug_macro')
    b = elf.contents(sec)
    us = units(b)
    cus = [u for u in us if u[2]]
    per = [u for o in objs for u in object_imports(o)]
    if len(cus) != len(per):
        raise SystemExit('%s: %d compile units, %d in the objects' % (output, len(cus), len(per)))
    kept = {}
    rows = []  # (field pos, width, signature, addend, value, keeper?)
    for fi, (cu, imps) in enumerate(zip(cus, per)):
        fields = cu[3]
        if len(fields) != len(imps):
            raise SystemExit('%s: unit at %#x has %d imports, its object %d' % (output, cu[0], len(fields), len(imps)))
        for (pos, w), (sig, add, comdat) in zip(fields, imps):
            v = int.from_bytes(b[pos:pos + w], 'little')
            if comdat and sig not in kept:
                kept[sig] = (v - add, fi)
            keeper = sig is None or not comdat or kept[sig][1] == fi
            rows.append((pos, w, sig, add, v, keeper))
    kept = {k: v[0] for k, v in kept.items()}
    return elf, sec, b, us, cus, kept, rows


def judge(output, tag, objs):
    elf, sec, b, us, cus, kept, rows = plan(output, objs)
    n = {'keeper': 0, 'redirect': 0, 'zero': 0, 'ones': 0, 'copy': 0, 'other': 0}
    starts = {u[0]: u for u in us}
    # a keeper's own import that names no unit start: its copy is not there
    bad = sum(1 for pos, w, sig, add, v, keeper in rows if keeper and v - add not in starts)
    for pos, w, sig, add, v, keeper in rows:
        if keeper:
            n['keeper'] += 1
        elif v == kept[sig] + add:
            n['redirect'] += 1
        elif v == 0:
            n['zero'] += 1
        elif v == (1 << (8 * w)) - 1:
            n['ones'] += 1
        elif v - add in starts and kept[sig] in starts and \
                b[v - add:starts[v - add][1]] == b[kept[sig]:starts[kept[sig]][1]]:
            n['copy'] += 1  # a second copy, byte for byte the kept one's (kept in the output)
        else:
            n['other'] += 1
    print('%s units=%d cu=%d imports=%d keeper=%d keeper_bad=%d redirect=%d zero=%d ones=%d copy=%d other=%d' % (
        tag, len(us), len(cus), len(rows), n['keeper'], bad, n['redirect'], n['zero'], n['ones'], n['copy'], n['other']))


def patch(output, new, mode, objs):
    elf, sec, b, us, cus, kept, rows = plan(output, objs)
    if sec.flags & 0x800:
        raise SystemExit('compressed .debug_macro: not patched')
    data = bytearray(elf.b)
    k = 0
    for pos, w, sig, add, v, keeper in rows:
        if keeper:
            continue
        val = {'redirect': kept[sig] + add, 'zero': 0, 'ones': (1 << (8 * w)) - 1}[mode]
        data[sec.off + pos:sec.off + pos + w] = val.to_bytes(w, 'little')
        k += 1
    with open(new, 'wb') as f:
        f.write(data)
    os.chmod(new, 0o755)
    print('%s: %d imports set to %s' % (new, k, mode))


if __name__ == '__main__':
    a = sys.argv
    if len(a) > 4 and a[1] == 'judge':
        judge(a[2], a[3], a[4:])
    elif len(a) > 5 and a[1] == 'patch':
        patch(a[2], a[3], a[4], a[5:])
    else:
        sys.exit(__doc__)
