# symfill.py IN OUT REMOVE MODE: write OUT = IN with .symtab rebuilt as IN-49a
# prescribes for an update: every local block (an STT_FILE starts one;
# the internal block before the first STT_FILE is one) followed by
# max(2, count/8) fillers, then a local tail reserve of max(64, locals/32)
# fillers; the global area dense, the global named REMOVE removed and its
# entry taken by the area's last; .symtab placed at the file's end with
# max(4 KiB, size/16) zero bytes of file slack after sh_size; .strtab kept
# (REMOVE's name stays as a dead name). Filler: name 0, info 0x03, other
# 0, shndx 1, value = section 1's sh_addr, size 0 (WP-207). ELF64 and
# ELF32, little endian.
import sys, struct
inp, outp, remove, mode = sys.argv[1], sys.argv[2], sys.argv[3].encode(), sys.argv[4]
# mode 'fill': IN-49a's fillers (WP-207 form: st_value = section 1's sh_addr);
# 'nofill': the same table and file layout without fillers (the control).
b = bytearray(open(inp, 'rb').read())
c64 = b[4] == 2
if c64:
    shoff, = struct.unpack_from('<Q', b, 0x28); shentsize, shnum, shstrndx = struct.unpack_from('<HHH', b, 0x3a)
    SH = '<IIQQQQIIQQ'; SYM = '<IBBHQQ'; ES = 24
else:
    shoff, = struct.unpack_from('<I', b, 0x20); shentsize, shnum, shstrndx = struct.unpack_from('<HHH', b, 0x2e)
    SH = '<IIIIIIIIII'; SYM = '<IIIBBH'; ES = 16
shdrs = [list(struct.unpack_from(SH, b, shoff + i*shentsize)) for i in range(shnum)]
si = [i for i, s in enumerate(shdrs) if s[1] == 2][0]
st = shdrs[si]; strt = shdrs[st[6]]
def name(off):
    o = strt[4] + off; e = b.index(b'\0', o); return bytes(b[o:e])
syms = []
for k in range(st[5] // ES):
    raw = bytes(b[st[4] + k*ES: st[4] + (k+1)*ES])
    if c64:
        n, info, other, shndx, val, size = struct.unpack(SYM, raw)
    else:
        n, val, size, info, other, shndx = struct.unpack(SYM, raw)
    syms.append((n, info, other, shndx, val, size))
def pack(n, info, other, shndx, val, size):
    return struct.pack(SYM, n, info, other, shndx, val, size) if c64 else struct.pack(SYM, n, val, size, info, other, shndx)
FILL = (0, 0x03, 0, 1, shdrs[1][3], 0)
nloc = st[7]
locs, globs = syms[1:nloc], syms[nloc:]
blocks, cur = [], []
for s in locs:
    if (s[1] & 0xf) == 4 and cur: blocks.append(cur); cur = []
    cur.append(s)
if cur: blocks.append(cur)
newloc, nfill = [syms[0]], 0
for blk in blocks:
    newloc += blk; f = max(2, len(blk)//8) if mode == 'fill' else 0; newloc += [FILL]*f; nfill += f
tail = max(64, len(locs)//32) if mode == 'fill' else 0; newloc += [FILL]*tail; nfill += tail
idx = [i for i, s in enumerate(globs) if name(s[0]) == remove]
if not idx: raise SystemExit('no global ' + remove.decode())
g = list(globs); i = idx[0]; last = g.pop()
if i < len(g): g[i] = last
body = b''.join(pack(*s) for s in newloc + g)
slack = max(4096, len(body)//16)
al = 8 if c64 else 4
newoff = (len(b) + al - 1) // al * al
b += b'\0' * (newoff - len(b)) + body + b'\0' * slack
st[4], st[5], st[7] = newoff, len(body), len(newloc)
newshoff = (len(b) + 7) // 8 * 8
b += b'\0' * (newshoff - len(b))
for s in shdrs: b += struct.pack(SH, *s)
if c64: struct.pack_into('<Q', b, 0x28, newshoff)
else: struct.pack_into('<I', b, 0x20, newshoff)
open(outp, 'wb').write(b)
print(f'{outp}: locals {nloc}->{len(newloc)} ({nfill} fillers, section 1 = shndx 1), globals {len(globs)}->{len(g)} ({remove.decode()} removed, last moved to its index {nloc+i} -> {len(newloc)+i}), slack {slack}')
