#!/usr/bin/env python3
"""classify.py LIB: the M-6 decision for each exported OBJECT of LIB, from the
program headers and st_value only (the section headers are not read), next to
the section that holds it (read only to check the answer).  REFUSE = st_value
in a PT_LOAD with PF_W and outside every PT_GNU_RELRO; WARN = otherwise;
UNKNOWN (refused) = in no PT_LOAD."""
import struct, sys
d = open(sys.argv[1], 'rb').read()
assert d[:4] == b'\x7fELF' and d[4] == 2 and d[5] == 1
e_phoff, e_shoff = struct.unpack_from('<QQ', d, 32)
e_phentsize, e_phnum, e_shentsize, e_shnum, e_shstrndx = struct.unpack_from('<HHHHH', d, 54)
ph = [struct.unpack_from('<IIQQQQQQ', d, e_phoff + i * e_phentsize) for i in range(e_phnum)]
loads = [(p[3], p[3] + p[6], p[1]) for p in ph if p[0] == 1]
relro = [(p[3], p[3] + p[6]) for p in ph if p[0] == 0x6474e552]
dyn = next(p for p in ph if p[0] == 2)
def v2o(v):
    for p in ph:
        if p[0] == 1 and p[3] <= v < p[3] + p[5]: return v - p[3] + p[2]
tags = {}
o = dyn[2]
while True:
    t, v = struct.unpack_from('<qQ', d, o); o += 16
    if t == 0: break
    tags.setdefault(t, v)
symtab, strtab = v2o(tags[6]), v2o(tags[5])
# the symbol count from DT_GNU_HASH would be the phdr-only way; DT_HASH or the section header is the shortcut here
sh = [struct.unpack_from('<IIQQQQIIQQ', d, e_shoff + i * e_shentsize) for i in range(e_shnum)]
shstr = sh[e_shstrndx][4]
def nm(off, base):
    e = d.index(b'\0', base + off); return d[base + off:e].decode()
dynsym = next(s for s in sh if s[1] == 11)
n = dynsym[5] // 24
for i in range(1, n):
    st_name, st_info, st_other, st_shndx, st_value, st_size = struct.unpack_from('<IBBHQQ', d, symtab + i * 24)
    if st_shndx == 0 or (st_info & 15) != 1: continue
    L = next((l for l in loads if l[0] <= st_value < l[1]), None)
    if L is None: verdict = 'UNKNOWN(refuse)'
    elif (L[2] & 2) and not any(a <= st_value < b for a, b in relro): verdict = 'REFUSE'
    else: verdict = 'WARN'
    sec = nm(sh[st_shndx][0], shstr) if st_shndx < len(sh) else '?'
    print('%-34s %-16s %s' % (nm(st_name, strtab), verdict, sec))
