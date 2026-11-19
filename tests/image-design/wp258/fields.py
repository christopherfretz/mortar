#!/usr/bin/env python3
"""WP-258: what a linker wrote for each debug reference of one object (DROP,
the second object, whose COMDAT copies are dropped) into a COMDAT code section.

  fields.py OUT DROP.o [KEEP.o]

For each non-alloc debug section of DROP.o with relocations: find DROP.o's
contribution in OUT's section of that name (the input bytes with every
relocated field masked must match exactly once), then print, for each
relocation whose target is an SHF_ALLOC member of a COMDAT group, one line:

  section  offset-in-contribution  reloc-type  target(+addend)  width  written-hex  [kept=0x..]

'kept' is OUT's symbol value of the group's signature (the kept copy), when
OUT has it. No DWARF is parsed; ELF64 little-endian only (AArch64, x86-64).

  fields.py OUT DROP.o --patch DEST SECTIONS ones|zero|kept

also writes DEST: OUT with those fields of the named sections (comma
separated) set to all ones, to 0, or to the kept copy's address + addend (the probe
of the alternatives; DEST is a new file).
"""
import re
import struct
import sys

RELTYPES = {
    (62, 1): ("R_X86_64_64", 8), (62, 10): ("R_X86_64_32", 4), (62, 11): ("R_X86_64_32S", 4),
    (62, 24): ("R_X86_64_PC64", 8), (62, 2): ("R_X86_64_PC32", 4),
    (183, 257): ("R_AARCH64_ABS64", 8), (183, 258): ("R_AARCH64_ABS32", 4),
    (183, 260): ("R_AARCH64_PREL64", 8), (183, 261): ("R_AARCH64_PREL32", 4),
}


class Elf:
    def __init__(self, path):
        self.data = open(path, "rb").read()
        d = self.data
        if d[:4] != b"\x7fELF" or d[4] != 2 or d[5] != 1:
            raise SystemExit(f"{path}: not ELF64 LE")
        self.machine = struct.unpack_from("<H", d, 18)[0]
        shoff, = struct.unpack_from("<Q", d, 0x28)
        shentsize, shnum, shstrndx = struct.unpack_from("<HHH", d, 0x3A)
        self.sh = []
        for i in range(shnum):
            f = struct.unpack_from("<IIQQQQIIQQ", d, shoff + i * shentsize)
            self.sh.append(dict(name=f[0], type=f[1], flags=f[2], addr=f[3], off=f[4],
                                size=f[5], link=f[6], info=f[7], entsize=f[9]))
        st = self.sh[shstrndx]
        for s in self.sh:
            s["n"] = self.cstr(st["off"] + s["name"])
        self.syms = []
        for s in self.sh:
            if s["type"] == 2:  # SHT_SYMTAB
                strt = self.sh[s["link"]]
                for j in range(s["size"] // 24):
                    nm, info, oth, shndx, val, sz = struct.unpack_from("<IBBHQQ", d, s["off"] + j * 24)
                    self.syms.append(dict(n=self.cstr(strt["off"] + nm), type=info & 15,
                                          shndx=shndx, value=val, size=sz))

    def cstr(self, o):
        e = self.data.index(b"\0", o)
        return self.data[o:e].decode("latin-1")

    def body(self, s):
        return self.data[s["off"]:s["off"] + s["size"]]

    def by_name(self, n):
        return [s for s in self.sh if s["n"] == n]


def main():
    out, drop = Elf(sys.argv[1]), Elf(sys.argv[2])
    patch = None
    if len(sys.argv) > 3 and sys.argv[3] == "--patch":
        patch = (sys.argv[4], sys.argv[5].split(","), sys.argv[6], bytearray(out.data))
    # COMDAT groups of DROP: member index -> signature
    member = {}
    for s in drop.sh:
        if s["type"] == 17:  # SHT_GROUP
            words = struct.unpack_from(f"<{s['size'] // 4}I", drop.data, s["off"])
            if words[0] & 1:
                sig = drop.syms[s["info"]]["n"]
                for m in words[1:]:
                    member[m] = sig
    outsym = {y["n"]: y["value"] for y in out.syms if y["n"] and y["shndx"] != 0}
    for ri, rs in enumerate(drop.sh):
        if rs["type"] != 4:  # SHT_RELA
            continue
        tgt = drop.sh[rs["info"]]
        if tgt["flags"] & 2 or not tgt["n"].startswith(".debug"):
            continue
        rels = []
        for j in range(rs["size"] // 24):
            off, info, add = struct.unpack_from("<QQq", drop.data, rs["off"] + j * 24)
            rels.append((off, info >> 32, info & 0xFFFFFFFF, add))
        body = bytearray(drop.body(tgt))
        mask = bytearray(len(body))
        for off, _, t, _ in rels:
            w = RELTYPES.get((drop.machine, t), ("?", 4))[1]
            if tgt["n"] == ".debug_aranges" and w == 8 and off % 16 == 0:
                w = 16  # a tuple's address and length: I-112 writes the length 0
            for k in range(w):
                mask[off + k] = 1
        pat = b"".join(b"." if mask[k] else re.escape(bytes([body[k]])) for k in range(len(body)))
        outs = out.by_name(tgt["n"])
        if not outs:
            print(f"{tgt['n']}\t-\tsection absent in output")
            continue
        ob = out.body(outs[0])
        hits = [m.start() for m in re.finditer(b"(?=" + pat + b")", ob, re.DOTALL)]  # overlapping too
        if not hits:
            print(f"{tgt['n']}\t-\tcontribution not found")
            continue
        # DROP is the last contributor of every debug section (crt and libc
        # objects carry none): prefer the match that ends the section
        end = len(ob) - len(body)
        if end in hits:
            base, where = end, "" if len(hits) == 1 else f" (the section's end, of {len(hits)} matches)"
        else:
            base, where = hits[-1], "" if len(hits) == 1 else f" (last of {len(hits)} matches)"
        for off, si, t, add in rels:
            sym = drop.syms[si]
            sec = sym["shndx"]
            if sec >= len(drop.sh) or sec == 0 or sec not in member or not drop.sh[sec]["flags"] & 2:
                continue
            name, w = RELTYPES.get((drop.machine, t), (f"type{t}", 4))
            val = ob[base + off:base + off + w][::-1].hex()
            who = sym["n"] if sym["type"] != 3 else f"[{drop.sh[sec]['n']}]"
            sig = member[sec]
            kept = f"kept=0x{outsym[sig]:x}" if sig in outsym else "kept=?"
            extra = ""
            if tgt["n"] == ".debug_aranges" and w == 8:
                extra = f" length={ob[base + off + 8:base + off + 16][::-1].hex()}"
            print(f"{tgt['n']}\t0x{off:x}\t{name}\t{who}{add:+d}\t{w}\t{val}\t{kept}{extra}{where}")
            if patch and tgt["n"] in patch[1]:
                fo = outs[0]["off"] + base + off
                if patch[2] == "ones":
                    new = (1 << (8 * w)) - 1
                elif patch[2] == "zero":
                    new = 0
                else:
                    new = outsym[sig] + add
                patch[3][fo:fo + w] = new.to_bytes(w, "little")
    if patch:
        with open(patch[0], "xb") as f:
            f.write(patch[3])


main()
