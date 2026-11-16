#!/usr/bin/env python3
"""check.py MODE LIB...: an independent model of dyn.md D-22/D-26 (.dynsym
order, .gnu.hash) and D-30/D-102 (.dynamic order), written from the rule
text, run against reference libraries.
  MODE hash: for each LIB (lld's), (1) .dynsym is null, then undefined
  value-0 entries, then hashed entries sorted by bucket (nbuckets per D-26);
  (2) the .gnu.hash section rebuilt by the D-26 formulas from the hashed
  names equals the library's byte for byte.
  MODE dynamic: for each LIB (GNU ld's), the .dynamic tag sequence (tags
  mortar never writes in Shared removed: D-102's list) against D-102's order.
"""
import struct, sys

def elf(path):
    d = open(path, 'rb').read()
    c64 = d[4] == 2
    if c64:
        shoff = struct.unpack_from('<Q', d, 40)[0]; shentsize, shnum, shstrndx = struct.unpack_from('<HHH', d, 58)
        sh = [struct.unpack_from('<IIQQQQIIQQ', d, shoff + i * shentsize) for i in range(shnum)]
    else:
        shoff = struct.unpack_from('<I', d, 32)[0]; shentsize, shnum, shstrndx = struct.unpack_from('<HHH', d, 46)
        sh = [struct.unpack_from('<IIIIIIIIII', d, shoff + i * shentsize) for i in range(shnum)]
    strt = sh[shstrndx][4]
    def nm(o, base): e = d.index(b'\0', base + o); return d[base + o:e].decode()
    secs = {nm(s[0], strt): s for s in sh}
    return d, c64, secs, sh, nm

def gnu_hash(name):
    h = 5381
    for c in name.encode(): h = (h * 33 + c) & 0xffffffff
    return h

def dynsyms(d, c64, secs, sh, nm):
    s = secs['.dynsym']; strtab = sh[s[6]][4]; ent = 24 if c64 else 16
    out = []
    for i in range(s[5] // ent):
        o = s[4] + i * ent
        if c64: n, info, other, shndx, val, size = struct.unpack_from('<IBBHQQ', d, o)
        else: n, val, size, info, other, shndx = struct.unpack_from('<IIIBBH', d, o)
        out.append((nm(n, strtab), shndx, val))
    return out

def model_gnuhash(names, symoffset, c64):
    W = 64 if c64 else 32
    n = len(names)
    if n == 0:
        return struct.pack('<IIII', 1, symoffset, 1, 26) + (b'\0' * (W // 8)) + struct.pack('<I', 0)
    nb = max(1, n // 4); x = (12 * n) // W; mw = 1
    while mw <= x: mw *= 2
    bloom = [0] * mw; hs = [gnu_hash(s) for s in names]
    for h in hs:
        w = (h // W) % mw; bloom[w] |= (1 << (h % W)) | (1 << ((h >> 26) % W))
    buckets = [0] * nb; chains = []
    for i, h in enumerate(hs):
        b = h % nb
        if buckets[b] == 0: buckets[b] = symoffset + i
        last = (i + 1 == n) or (hs[i + 1] % nb != b)
        chains.append((h & ~1) | (1 if last else 0))
    out = struct.pack('<IIII', nb, symoffset, mw, 26)
    out += b''.join(struct.pack('<Q' if c64 else '<I', w) for w in bloom)
    out += b''.join(struct.pack('<I', b) for b in buckets) + b''.join(struct.pack('<I', c) for c in chains)
    return out

ORDER = ['NEEDED', 'SONAME', 'SYMBOLIC', 'RUNPATH', 'RPATH', 'INIT', 'FINI', 'PREINIT_ARRAY', 'PREINIT_ARRAYSZ',
         'INIT_ARRAY', 'INIT_ARRAYSZ', 'FINI_ARRAY', 'FINI_ARRAYSZ', 'GNU_HASH', 'STRTAB', 'SYMTAB', 'STRSZ', 'SYMENT',
         'PLTGOT', 'PLTRELSZ', 'PLTREL', 'JMPREL', 'RELA', 'RELASZ', 'RELAENT', 'REL', 'RELSZ', 'RELENT',
         'AARCH64_BTI_PLT', 'AARCH64_VARIANT_PCS', 'FLAGS', 'FLAGS_1', 'VERDEF', 'VERDEFNUM',
         'VERNEED', 'VERNEEDNUM', 'VERSYM', 'RELACOUNT', 'RELCOUNT', 'NULL']
NEVER = {'HASH', 'TLSDESC_PLT', 'TLSDESC_GOT', 'TEXTREL', 'DEBUG'}

mode = sys.argv[1]; bad = 0
for p in sys.argv[2:]:
    E = elf(p); d, c64, secs, sh, nm = E
    if mode == 'hash':
        syms = dynsyms(*E)
        g = secs['.gnu.hash']; gd = d[g[4]:g[4] + g[5]]
        nb, symoff = struct.unpack_from('<II', gd, 0)
        unh = syms[1:symoff]; hashed = syms[symoff:]
        part = all(s[1] == 0 and s[2] == 0 for s in unh) and not any(s[1] == 0 and s[2] == 0 for s in hashed)
        n = len(hashed); mnb = max(1, n // 4)
        bk = [gnu_hash(s[0]) % mnb for s in hashed]
        ordered = bk == sorted(bk)
        same = model_gnuhash([s[0] for s in hashed], symoff, c64) == gd
        ok = part and ordered and same; bad += not ok
        print('%s hash n=%d nbuckets(lib)=%d model=%d partition=%s bucket-order=%s gnu.hash-bytes-equal=%s' % (p, n, nb, mnb, part, ordered, same))
    else:
        import subprocess
        out = subprocess.run(['readelf', '-dW', p], capture_output=True, text=True).stdout
        tags = [l.split('(')[1].split(')')[0] for l in out.splitlines() if l.strip().startswith('0x')]
        kept = [t for t in tags if t not in NEVER]
        # collapse repeats (NEEDED x n, NULL x n) for the order comparison
        seq = [t for i, t in enumerate(kept) if i == 0 or kept[i - 1] != t]
        unknown = [t for t in seq if t not in ORDER]
        ranks = [ORDER.index(t) for t in seq if t in ORDER]
        ok = ranks == sorted(ranks) and not unknown; bad += not ok
        print('%s dynamic order-as-D-102=%s dropped=%s unknown=%s seq=%s' % (p, ranks == sorted(ranks), [t for t in tags if t in NEVER], unknown, ' '.join(seq)))
print('MISMATCHES', bad)
