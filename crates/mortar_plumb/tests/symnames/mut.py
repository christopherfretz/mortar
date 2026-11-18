#!/usr/bin/env python3
# mut.py IN OUT OP: WP-246's mutations of tests/symnames.rs.
#
# Edits one entry of a static ELF64 little-endian output's .symtab in
# place (bytes only, no linker; nothing else moves but for `drop`). An
# entry is found by its name: the first entry of that name, a local
# unless the op says otherwise.
#
#   mid=<name>:<k>    the entry's st_name moves k bytes into its string
#                     (the evidence of WP-246: `readed` for
#                     __libc_single_threaded is mid=__libc_single_threaded:16)
#   fileempty=<name>  the STT_FILE entry <name> gets st_name 0 (no name)
#   fill=<name>       the local becomes a filler (spec/plumb.md §6.9: name
#                     0, st_info 0x03, st_other 0, st_shndx 1, value section
#                     1's sh_addr, size 0): an incremental-mode output's
#                     dropped local
#   drop=<name>       the local is removed: the entries after it move up
#                     one, .symtab's sh_size shrinks by one entry and sh_info
#                     by one (a plain output's dropped local)
#   swap=<a>,<b>      two locals exchange their st_name
#   gmid=<name>:<k>   as mid, for the global entry of the name
#   value=<name>      the local's st_value plus 1
#
# Prints the entry's index (the first entry for swap).
import struct
import sys

SH = '<IIQQQQIIQQ'
SYM = '<IBBHQQ'
ES = 24


def main():
    inp, outp, op = sys.argv[1:4]
    b = bytearray(open(inp, 'rb').read())
    assert b[4] == 2 and b[5] == 1, 'ELF64 little endian only'
    shoff, = struct.unpack_from('<Q', b, 0x28)
    shentsize, shnum, _ = struct.unpack_from('<HHH', b, 0x3a)

    def shdr(i):
        return list(struct.unpack_from(SH, b, shoff + i * shentsize))
    shdrs = [shdr(i) for i in range(shnum)]
    si = next(i for i, s in enumerate(shdrs) if s[1] == 2)  # SHT_SYMTAB
    st = shdrs[si]
    ti = st[6]
    stroff = shdrs[ti][4]
    n = st[5] // ES
    nloc = st[7]

    def sym(k):
        return list(struct.unpack_from(SYM, b, st[4] + k * ES))

    def put(k, s):
        struct.pack_into(SYM, b, st[4] + k * ES, *s)

    def name(s):
        o = stroff + s[0]
        return bytes(b[o:b.index(b'\0', o)]).decode()

    def find(nm, local=True, ftype=None):
        for k in range(1, n):
            s = sym(k)
            if name(s) != nm or ((s[1] >> 4) == 0) != local:
                continue
            if ftype is not None and (s[1] & 0xf) != ftype:
                continue
            return k
        sys.exit('no entry ' + nm)

    word, arg = op.split('=', 1)
    if word in ('mid', 'gmid'):
        nm, k = arg.rsplit(':', 1)
        i = find(nm, local=(word == 'mid'))
        s = sym(i)
        s[0] += int(k)
        put(i, s)
    elif word == 'fileempty':
        i = find(arg, ftype=4)
        s = sym(i)
        s[0] = 0
        put(i, s)
    elif word == 'fill':
        i = find(arg)
        assert i < nloc
        put(i, [0, 0x03, 0, 1, shdrs[1][3], 0])
    elif word == 'drop':
        i = find(arg)
        assert i < nloc
        start = st[4] + i * ES
        end = st[4] + n * ES
        b[start:end - ES] = b[start + ES:end]
        b[end - ES:end] = bytes(ES)
        st[5] -= ES
        st[7] -= 1
        struct.pack_into(SH, b, shoff + si * shentsize, *st)
    elif word == 'swap':
        x, y = arg.split(',')
        i, j = find(x), find(y)
        s, t = sym(i), sym(j)
        s[0], t[0] = t[0], s[0]
        put(i, s)
        put(j, t)
    elif word == 'value':
        i = find(arg)
        s = sym(i)
        s[4] += 1
        put(i, s)
    else:
        sys.exit('unknown op ' + op)
    open(outp, 'wb').write(b)
    print(i)


main()
