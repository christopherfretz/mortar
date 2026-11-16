#!/usr/bin/env python3
"""D-117's address-level rule, written by the second reviewer from the TEXT of
D-117 alone (default line: no -Bsymbolic, no --dynamic-list, no script).

usage: rule.py [--xt=least|none|exact] [--hits] FILE...   (objects or archives)
prints per object:  REFUSE <obj> <n> | OK <obj>   and with --hits each use.
Also classifies each object: PIC / NONPIC (a table-refusable relocation
against a default-visibility global by symbol) and SELFGOT (a GOT relocation
against one of its own exported data).
"""
import sys
from elf import load_any, SHF_ALLOC, SHF_EXEC, EM_386, EM_X86_64, EM_AARCH64

STB_LOCAL, STB_GLOBAL, STB_WEAK, STB_UNIQUE = 0, 1, 2, 10
STV_DEFAULT = 0
SHN_UNDEF, SHN_ABS, SHN_COMMON = 0, 0xfff1, 0xfff2

X64_PC = {2, 4, 13, 15, 24, 9, 41, 42, 26}   # PC32 PLT32 PC16 PC8 PC64 GOTPCREL GOTPCRELX REX_GOTPCRELX GOTPC32
I386_PC = {2, 4}
X64_SIZE = {32, 33}
# non-PIC signatures by symbol (D-117's table, approximated): absolute / PC forms that need a local def
A64_NONPIC = {275, 274, 277, 278, 284, 285, 286, 299, 273, 263, 264, 265, 266, 267, 268, 269, 270, 271}
# 275 ADR_PREL_PG_HI21 274 ADR_PREL_LO21 277 ADD_ABS_LO12 278.. LDST*, 299 LDST128, 273 LD_PREL_LO19, 263.. MOVW
A64_GOT = {311, 312, 313, 309, 310}  # ADR_GOT_PAGE LD64_GOT_LO12 LD64_GOTPAGE_LO15 ...
X64_NONPIC = {2, 10, 11, 1}           # PC32 32 32S 64 (by symbol, against a preemptible key)
X64_GOT = {9, 41, 42}


def preemptible_data(o):
    out = {}
    for y in o.syms:
        if y.bind in (STB_GLOBAL, STB_WEAK, STB_UNIQUE) and y.vis == STV_DEFAULT and 0 < y.shndx < 0xff00:
            sec = o.secs[y.shndx]
            if not (sec.flags & SHF_EXEC):
                out.setdefault(y.shndx, []).append(y)
    return out


def scan(o, xt="least", hits=None):
    pd = preemptible_data(o)
    sized_starts = {}
    for y in o.syms:
        if 0 < y.shndx < 0xff00 and y.size:
            sized_starts.setdefault(y.shndx, set()).add(y.value)
    nonpic = selfgot = False
    own = {y.name for v in pd.values() for y in v}
    n = 0
    for tgt, rels in o.relsecs():
        if not (tgt.flags & SHF_ALLOC):
            continue
        if tgt.name == ".eh_frame" or tgt.name.startswith(".gcc_except_table"):
            continue
        for r in rels:
            if r.type == 0:
                continue
            if o.mach == EM_X86_64 and r.type in X64_SIZE:
                continue
            if r.sym >= len(o.syms):
                continue
            s = o.syms[r.sym]
            # classification by symbol
            if s.bind != STB_LOCAL:
                defd = 0 < s.shndx < 0xff00 or s.shndx == SHN_COMMON
                if o.mach == EM_AARCH64:
                    if r.type in A64_NONPIC and s.vis == STV_DEFAULT and s.type != 2:
                        nonpic = True
                    if r.type in A64_GOT and s.name in own:
                        selfgot = True
                elif o.mach == EM_X86_64:
                    if r.type in (10, 11) or (r.type == 2 and s.vis == STV_DEFAULT) or (r.type == 1 and tgt.flags & SHF_EXEC):
                        nonpic = True
                    if r.type in X64_GOT and s.name in own:
                        selfgot = True
                continue
            # the rule: a use through a local symbol defined in T
            T = s.shndx
            if not (0 < T < 0xff00) or T not in pd:
                continue
            base = s.value + r.addend
            named = base
            pc = (o.mach == EM_X86_64 and r.type in X64_PC) or (o.mach == EM_386 and r.type in I386_PC)
            if pc and (tgt.flags & SHF_EXEC):
                if xt == "least":
                    t = 0
                    for c in (0, 1, 2, 4):
                        if base + 4 + c in sized_starts.get(T, ()):
                            t = c
                            break
                    named = base + 4 + t
                elif xt == "none":
                    named = base + 4
            for d in pd[T]:
                if d.value <= named < d.value + max(d.size, 1):
                    n += 1
                    if hits is not None:
                        end_local = any(y.shndx == T and y.size and y.value + y.size == named and y.bind == STB_LOCAL for y in o.syms)
                        hits.append((tgt.name, hex(r.off), r.type, s.name or o.secs[T].name, r.addend, d.name, hex(named - d.value), "start=end-of-local" if end_local else ""))
                    break
    return n, nonpic, selfgot


def main():
    args = sys.argv[1:]
    xt, show = "least", False
    files = []
    for a in args:
        if a.startswith("--xt="):
            xt = a[5:]
        elif a == "--hits":
            show = True
        else:
            files.append(a)
    tot = ref = 0
    for f in files:
        for name, o in load_any(f):
            tot += 1
            h = []
            try:
                n, nonpic, selfgot = scan(o, xt, h)
            except Exception as e:  # noqa
                print("ERR", name, e)
                continue
            cls = ("NONPIC" if nonpic else "PIC") + ("+SELFGOT" if selfgot else "")
            if n:
                ref += 1
                print("REFUSE", name, n, cls)
                if show:
                    for x in h:
                        print("   ", *x)
            elif show:
                print("OK", name, cls)
    print(f"TOTAL objects={tot} refused={ref}")


if __name__ == "__main__":
    main()
