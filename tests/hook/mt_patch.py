#!/usr/bin/env python3
"""The load-time ODR matrix (spec/odr.md O-58): malformations of a linked
module's table (case MT) and of its dynamic section (the DT_PLTREL attack),
for the runtime to name (odr-hook-malformed-table), never to believe or crash
on. Little-endian ELF32 and ELF64.

usage: mt_patch.py IN OUT MODE
  count   the table's count past its descriptor
  trunc   the note's descriptor shorter than the table's header
  seg     the table's PT_NOTE runs a few rows past the end of its PT_LOAD
          (the descriptor and count with it): only a reader that trusts
          p_memsz finds rows there
  off     the table's PT_NOTE moved outside every PT_LOAD of the module
  sym     a row's .dynsym index past .dynsym
  order   the rows out of key order (needs two rows)
  pltrel  DT_PLTREL names the other relocation format (RELA <-> REL)
"""
import struct
import sys

PT_LOAD, PT_DYNAMIC, PT_NOTE = 1, 2, 4
NT_MASON_ODR = 0x4D4F4401
DT_NULL, DT_PLTREL = 0, 20
DT_RELA, DT_REL = 7, 17


def main():
    src, dst, mode = sys.argv[1], sys.argv[2], sys.argv[3]
    b = bytearray(open(src, 'rb').read())
    is64 = b[4] == 2
    if is64:
        phoff, = struct.unpack_from('<Q', b, 0x20)
        phentsize, phnum = struct.unpack_from('<HH', b, 0x36)
        ph_fmt, dyn_fmt = '<IIQQQQQQ', '<qQ'
    else:
        phoff, = struct.unpack_from('<I', b, 0x1C)
        phentsize, phnum = struct.unpack_from('<HH', b, 0x2A)
        ph_fmt, dyn_fmt = '<IIIIIIII', '<iI'
    dyn_size = struct.calcsize(dyn_fmt)

    def phdr(i):
        f = list(struct.unpack_from(ph_fmt, b, phoff + i * phentsize))
        if is64:  # type flags offset vaddr paddr filesz memsz align
            return {'type': f[0], 'offset': f[2], 'vaddr': f[3], 'filesz': f[5], 'memsz': f[6], 'align': f[7]}
        # type offset vaddr paddr filesz memsz flags align
        return {'type': f[0], 'offset': f[1], 'vaddr': f[2], 'filesz': f[4], 'memsz': f[5], 'align': f[7]}

    def set_phdr(i, key, value):
        if is64:
            pos = {'offset': 8, 'vaddr': 16, 'filesz': 32, 'memsz': 40}[key]
            struct.pack_into('<Q', b, phoff + i * phentsize + pos, value)
        else:
            pos = {'offset': 4, 'vaddr': 8, 'filesz': 16, 'memsz': 20}[key]
            struct.pack_into('<I', b, phoff + i * phentsize + pos, value)

    if mode == 'pltrel':
        for i in range(phnum):
            p = phdr(i)
            if p['type'] != PT_DYNAMIC:
                continue
            off = p['offset']
            while True:
                tag, val = struct.unpack_from(dyn_fmt, b, off)
                if tag == DT_NULL:
                    break
                if tag == DT_PLTREL:
                    struct.pack_into(dyn_fmt, b, off, tag, DT_REL if val == DT_RELA else DT_RELA)
                    open(dst, 'wb').write(b)
                    return
                off += dyn_size
        sys.exit('mt_patch: no DT_PLTREL')

    # the table: the MasonMeta note of type NT_MASON_ODR in some PT_NOTE
    for i in range(phnum):
        p = phdr(i)
        if p['type'] != PT_NOTE:
            continue
        al = 8 if p['align'] > 4 else 4
        off, end = p['offset'], p['offset'] + p['filesz']
        while off + 12 <= end:
            namesz, descsz, ntype = struct.unpack_from('<III', b, off)
            doff = (off + 12 + namesz + al - 1) & ~(al - 1)
            name = bytes(b[off + 12:off + 12 + namesz])
            if name == b'MasonMeta\0' and ntype == NT_MASON_ODR:
                count = struct.unpack_from('<I', b, doff + 4)[0]
                rows = doff + 16
                if mode == 'count':
                    struct.pack_into('<I', b, doff + 4, (descsz - 16) // 24 + 1)
                elif mode == 'trunc':
                    struct.pack_into('<I', b, off + 4, 8)
                elif mode == 'seg':
                    # just past the end of the PT_LOAD holding the note (still in
                    # its last mapped page, so the loader reads nothing unmapped)
                    lend = None
                    for j in range(phnum):
                        q = phdr(j)
                        if q['type'] == PT_LOAD and q['vaddr'] <= p['vaddr'] < q['vaddr'] + q['memsz']:
                            lend = q['vaddr'] + q['memsz']
                    if lend is None:
                        sys.exit('mt_patch: the note is in no PT_LOAD')
                    grow = (lend - (p['vaddr'] + p['memsz']) + 48 + 23) // 24 * 24
                    set_phdr(i, 'memsz', p['memsz'] + grow)
                    set_phdr(i, 'filesz', p['filesz'] + grow)
                    struct.pack_into('<I', b, off + 4, descsz + grow)
                    struct.pack_into('<I', b, doff + 4, count + grow // 24)
                elif mode == 'off':
                    set_phdr(i, 'vaddr', p['vaddr'] + 0x40000000)
                elif mode == 'sym':
                    if count < 1:
                        sys.exit('mt_patch: no row')
                    struct.pack_into('<I', b, rows + 16, 0x7FFFFFF0)
                elif mode == 'order':
                    if count < 2:
                        sys.exit('mt_patch: order needs two rows')
                    struct.pack_into('<Q', b, rows, 0xFFFFFFFFFFFFFFFF)
                else:
                    sys.exit('mt_patch: mode ' + mode + '?')
                open(dst, 'wb').write(b)
                return
            off = (doff + descsz + al - 1) & ~(al - 1)
    sys.exit('mt_patch: no table note')


main()
