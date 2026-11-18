#!/usr/bin/env python3
# edit.py IN IN.map OUT OUT.map OP...: WP-222's editor of tests/symfill.rs.
#
# Rewrites a mortar output (ELF64 little endian, static) and its map into
# the shape spec/incr.md IN-49a gives an incremental-mode output, and
# mutates it, without the linker (it reads and writes bytes and map lines
# only). The output's last sections in file order must be .symtab,
# .strtab, .shstrtab, then the section header table; they are laid out
# again from .symtab's offset, so nothing else moves.
#
# OPs, applied in this order whatever the order given:
#   fill        IN-49a's fillers: after each local block (an STT_FILE starts
#               one; the entries before the first are one) max(2, count/8),
#               then a local tail reserve of max(64, locals/32); sh_info
#               counts them. The filler: st_name 0, st_info 0x03, st_other
#               0, st_shndx 1, st_value section 1's sh_addr, st_size 0.
#   slack       max(4096, size/16) zero bytes after .symtab and after
#               .strtab, listed `hole .symtab <sh_size> <n>`, `hole .strtab
#               <sh_size> <n>`; .strtab gains an old name ("afun", a copy of
#               a live one) and a region's slack (16 zeros), listed `dead
#               .strtab <off> <size> strings`.
#   shndx       a .symtab_shndx section (one zero word per entry), last.
#   decisions   one decision record of each kind (spec/plumb.md §3.2) and
#               `<file>` on every `common` (the file of a-<arch>.o).
#   plain       the map's `incremental` line removed (and no slack lines).
# Mutations, one sentence each (tests/symfill.rs names the rule):
#   filler-after-info   one more filler at the end, after the globals
#   near=<field>        the first filler differs in <field>: st_name,
#                       st_info, st_other, st_shndx, st_value, st_size,
#                       .symtab_shndx
#   slack-long          .symtab's slack line one byte longer than its room
#   slack-nonzero       a byte of .symtab's slack is 0x5a
#   slack-off           .symtab's slack line starts 8 bytes past sh_size
#   hole-nonalloc       `hole .comment 8 4` (inside .comment, no symbol there)
#   strtab-named        the global afun's st_name names the old name
#   strtab-nonul        the old name's dead line leaves its NUL out
#   region-out          the region runs one byte past .text's end
#   region-none         the region names `.nosuch`
#   region-overlap      a second region of .text meets the first
#   symblock-past       the symbol block runs past sh_info
#   common-file=<id>    every `common` names file <id>
#   l1                  a.c's `$d` of .debug_info is moved to offset 0
#                       (start.c's unit: another unit's identity)
import struct
import sys

SH = '<IIQQQQIIQQ'
SYM = '<IBBHQQ'
ES = 24


def main():
    inp, inmap, outp, outmap = sys.argv[1:5]
    ops = sys.argv[5:]
    flags = {o.split('=')[0]: (o.split('=', 1)[1] if '=' in o else True) for o in ops}
    b = bytearray(open(inp, 'rb').read())
    lines = open(inmap).read().split('\n')
    assert b[4] == 2 and b[5] == 1, 'ELF64 little endian only'
    shoff, = struct.unpack_from('<Q', b, 0x28)
    shentsize, shnum, shstrndx = struct.unpack_from('<HHH', b, 0x3a)
    shdrs = [list(struct.unpack_from(SH, b, shoff + i * shentsize)) for i in range(shnum)]
    shstr = bytes(b[shdrs[shstrndx][4]:shdrs[shstrndx][4] + shdrs[shstrndx][5]])

    def sname(s):
        o = s[0]
        return shstr[o:shstr.index(b'\0', o)].decode()
    names = [sname(s) for s in shdrs]
    si = names.index('.symtab')
    ti = shdrs[si][6]
    order = sorted(range(1, shnum), key=lambda i: shdrs[i][4])
    assert [names[i] for i in order[-3:]] == ['.symtab', '.strtab', '.shstrtab'], order
    assert shoff >= shdrs[shstrndx][4] + shdrs[shstrndx][5]
    st = shdrs[si]
    strtab = bytearray(b[shdrs[ti][4]:shdrs[ti][4] + shdrs[ti][5]])
    syms = [list(struct.unpack_from(SYM, b, st[4] + k * ES)) for k in range(st[5] // ES)]
    nloc = st[7]
    sec1 = shdrs[1][3]
    FILL = [0, 0x03, 0, 1, sec1, 0]

    def symname(s):
        return bytes(strtab[s[0]:strtab.index(b'\0', s[0])]).decode()

    def find(name, sec=None):
        for k, s in enumerate(syms):
            if symname(s) == name and (sec is None or s[3] == sec):
                return k
        raise SystemExit(f'no symbol {name}')

    if 'l1' in flags:
        # a.c's $d in .debug_info: the second one there (start.c's first).
        di = names.index('.debug_info')
        ds = [k for k, s in enumerate(syms) if symname(s) == '$d' and s[3] == di]
        syms[ds[1]][4] = 0
    if 'fill' in flags:
        locs = syms[1:nloc]
        blocks, cur = [], []
        for s in locs:
            if (s[1] & 0xf) == 4 and cur:
                blocks.append(cur)
                cur = []
            cur.append(s)
        if cur:
            blocks.append(cur)
        new = [syms[0]]
        for blk in blocks:
            new += blk + [list(FILL) for _ in range(max(2, len(blk) // 8))]
        new += [list(FILL) for _ in range(max(64, len(locs) // 32))]
        syms = new + syms[nloc:]
        nloc = len(new)
    if 'filler-after-info' in flags:
        syms.append(list(FILL))
    if 'near' in flags:
        k = next(k for k in range(1, nloc) if syms[k] == FILL)
        field = flags['near']
        col = {'st_name': 0, 'st_info': 1, 'st_other': 2, 'st_shndx': 3, 'st_value': 4,
               'st_size': 5}.get(field)
        if col is not None:
            syms[k][col] = {0: 1, 1: 0x00, 2: 1, 3: 2, 4: sec1 + 4, 5: 4}[col]
        near_shndx = k if field == '.symtab_shndx' else None
    else:
        near_shndx = None
    dead = []
    if 'slack' in flags:
        old = len(strtab)
        strtab += b'afun\0'
        dead.append((old, 5 if 'strtab-nonul' not in flags else 4))
        reg = len(strtab)
        strtab += b'\0' * 16
        dead.append((reg, 16))
        if 'strtab-named' in flags:
            syms[find('afun')][0] = old
    body = b''.join(struct.pack(SYM, *s) for s in syms)
    slack1 = max(4096, len(body) // 16) if 'slack' in flags else 0
    slack2 = max(4096, len(strtab) // 16) if 'slack' in flags else 0
    # The tail, laid out again from .symtab's offset.
    pos = st[4]
    tail = bytearray()
    st[5], st[7] = len(body), nloc
    tail += body + b'\0' * slack1
    if 'slack-nonzero' in flags:
        tail[len(body) + slack1 // 2] = 0x5a
    shdrs[ti][4] = pos + len(tail)
    shdrs[ti][5] = len(strtab)
    tail += strtab + b'\0' * slack2
    if 'shndx' in flags:
        shstr += b'.symtab_shndx\0'
    shdrs[shstrndx][4] = pos + len(tail)
    shdrs[shstrndx][5] = len(shstr)
    tail += shstr
    if 'shndx' in flags:
        while (pos + len(tail)) % 4:
            tail += b'\0'
        words = [0] * len(syms)
        if near_shndx is not None:
            words[near_shndx] = 1
        shdrs.append([len(shstr) - 14, 18, 0, 0, pos + len(tail), 4 * len(syms), si, 0, 4, 4])
        tail += b''.join(struct.pack('<I', w) for w in words)
    while (pos + len(tail)) % 8:
        tail += b'\0'
    newshoff = pos + len(tail)
    for s in shdrs:
        tail += struct.pack(SH, *s)
    del b[pos:]
    b += tail
    struct.pack_into('<Q', b, 0x28, newshoff)
    struct.pack_into('<H', b, 0x3c, len(shdrs))
    open(outp, 'wb').write(b)

    # The map.
    def tok(n):
        return n
    for i, l in enumerate(lines):
        w = l.split(' ')
        if w[:2] == ['chunk', 'synthetic'] and w[2] in ('.symtab', '.strtab', '.shstrtab'):
            size = {'.symtab': st[5], '.strtab': shdrs[ti][5], '.shstrtab': len(shstr)}[w[2]]
            lines[i] = ' '.join(w[:4] + [f'{size:x}'])
    files = {}
    for l in lines:
        w = l.split(' ')
        if w[0] == 'file':
            files[w[2]] = int(w[1])
    afile = next(v for k, v in files.items() if k.startswith('a-'))
    if 'decisions' in flags or 'common-file' in flags:
        fid = int(flags['common-file']) if 'common-file' in flags else afile
        lines = [l + f' {fid}' if l.startswith('common ') else l for l in lines]
    extra = []
    if 'slack' in flags and 'plain' not in flags:
        off = st[5] + (8 if 'slack-off' in flags else 0)
        n = slack1 + (shdrs[ti][4] - (st[4] + st[5] + slack1)) + (1 if 'slack-long' in flags else 0)
        extra.append(f'hole .symtab {off:x} {n:x}')
        extra.append(f'hole .strtab {shdrs[ti][5]:x} {slack2:x}')
    if 'slack' in flags:
        for o, z in dead:
            extra.append(f'dead .strtab {o:x} {z:x} strings')
    if 'hole-nonalloc' in flags:
        extra.append('hole .comment 8 4')
    if 'decisions' in flags:
        ti_text = names.index('.text')
        a, z = shdrs[ti_text][3], shdrs[ti_text][5]
        cap = z + (1 if 'region-out' in flags else 0)
        out = '.nosuch' if 'region-none' in flags else '.text'
        if 'region-overlap' in flags:
            extra.append(f'region {afile} {out} 0 {a:x} {cap // 2:x}')
            extra.append(f'region 0 {out} 0 {a + cap // 4:x} {cap // 2:x}')
        else:
            extra.append(f'region {afile} {out} 0 {a:x} {cap:x}')
        extra.append(f'capacity .text 0 b0 {a:x} {z:x} {z:x}')
        extra.append('capacity .symtab 1 s - 0 0')
        extra.append('order .text 0')
        extra.append(f'slot got afun {a:x}')
        extra.append(f'slot tls-gd {afile} 3 {a:x}')
        extra.append(f'slot tls-ld {a:x}')
        extra.append(f'redirect {afile} 1 4 {a:x}')
        extra.append('frag .comment 30,1,1 0123456789abcdef0123456789abcdef 0')
        first, count = 1, 3
        capacity = (nloc - first + 1) if 'symblock-past' in flags else 4
        extra.append(f'symblock - {first} {count} {capacity} 1 {len(strtab) - 1:x}')
    # After `incremental` and the map's own hole/dead lines.
    at = max(i for i, l in enumerate(lines)
             if l.split(' ')[0] in ('incremental', 'hole', 'dead', 'common', 'define', 'chunk')) + 1
    lines[at:at] = extra
    if 'plain' in flags:
        lines = [l for l in lines if l != 'incremental']
    open(outmap, 'w').write('\n'.join(lines))
    print(f'{outp}: {len(syms)} entries, sh_info {nloc}, slack {slack1}+{slack2}, map +{len(extra)} lines')


main()
