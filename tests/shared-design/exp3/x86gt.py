#!/usr/bin/env python3
"""x86gt.py: how x86 non-PIC code reaches its own exported data (M-1 ruling).

usage: x86gt.py LABEL @PAIRFILE   (lines "PIC_OBJ NONPIC_OBJ"; x86-64 or i386)

Bound definitions: own exported data the -fPIC twin reaches through the GOT.
In the non-PIC twin, for each bound definition, every relocation of an alloc
place (not .eh_frame / LSDA) that reaches it is classified:
  SYMBOL   names the definition's symbol (D-117's table judges it)
  EXACT    names a local/section symbol of its section with an EXACT address
           inside it: absolute words, GOTOFF forms, PC-relative words in a
           non-exec place (the address rule applies, M-1 ruling)
  CODEPC   a PC-relative field in an SHF_EXECINSTR place against a local or
           section symbol whose S+A+4 or S+A+4+t (t in 1,2,4) lies inside it
           (outside the exact rule on x86)
A definition is counted under the first of SYMBOL, EXACT, CODEPC that occurs,
else UNSEEN.  Prints per object and a TABLE line; with CODEPC listed per use.
"""
import sys
from elf import load_any, SHF_ALLOC, SHF_EXEC, EM_386, EM_X86_64
import s1d

X64_GOT = {9, 41, 42, 3, 25, 26, 27, 28, 29, 30}
I386_GOT = {3, 43}
X64_PC = {2, 4, 13, 15, 24}            # PC32 PLT32 PC16 PC8 PC64
I386_PC = {2, 4, 23}                   # PC32 PLT32 PC8? (23 = PC8 on i386)
X64_GOTOFF = {25}                      # GOTOFF64
I386_GOTOFF = {9}


def one(p):
    return next(iter(load_any(p)))[1]


def main():
    label, pf = sys.argv[1], sys.argv[2][1:]
    t = dict(objs=0, users=0, defs=0, symbol=0, exact=0, codepc=0, unseen=0)
    for line in open(pf):
        a, b = line.split()
        try:
            pa, pb = one(a), one(b)
        except Exception:
            continue
        t['objs'] += 1
        got = X64_GOT if pa.mach == EM_X86_64 else I386_GOT
        pdb = s1d.preemptible_data(pb)
        ownb = {y.name: y for v in pdb.values() for y in v}
        bound = set()
        for tgt, rels in pa.relsecs():
            for r in rels:
                if r.type in got and r.sym < len(pa.syms) and pa.syms[r.sym].bind != 0 and pa.syms[r.sym].name in ownb:
                    bound.add(pa.syms[r.sym].name)
        if not bound:
            continue
        t['users'] += 1
        how = {d: set() for d in bound}
        notes = []
        x64 = pb.mach == EM_X86_64
        PC = X64_PC if x64 else I386_PC
        GOTOFF = X64_GOTOFF if x64 else I386_GOTOFF
        for tgt, rels in pb.relsecs():
            if not tgt.flags & SHF_ALLOC or tgt.name == '.eh_frame' or tgt.name.startswith('.gcc_except'):
                continue
            for r in rels:
                if r.type == 0 or r.sym >= len(pb.syms):
                    continue
                y = pb.syms[r.sym]
                if y.bind != 0:
                    if y.name in bound and r.type not in got:
                        how[y.name].add('symbol')
                    continue
                if y.shndx not in pdb:
                    continue
                pc = r.type in PC
                code = bool(tgt.flags & SHF_EXEC)
                for d in pdb[y.shndx]:
                    if d.name not in bound:
                        continue
                    lo, hi = d.value, d.value + max(d.size, 1)
                    if pc and code:
                        cands = [y.value + r.addend + 4 + k for k in (0, 1, 2, 4)]
                        if any(lo <= c < hi for c in cands):
                            how[d.name].add('codepc')
                            notes.append('    CODEPC %s+%#x type=%d %s%+d -> %s' % (tgt.name, r.off, r.type, y.name or pb.secs[y.shndx].name, r.addend, d.name))
                    else:
                        if lo <= y.value + r.addend < hi:
                            how[d.name].add('exact')
                            notes.append('    EXACT %s+%#x type=%d %s%+d -> %s' % (tgt.name, r.off, r.type, y.name or pb.secs[y.shndx].name, r.addend, d.name))
        cnt = {}
        for d in sorted(bound):
            t['defs'] += 1
            k = next((k for k in ('symbol', 'exact', 'codepc') if k in how[d]), 'unseen')
            t[k] += 1
            cnt[k] = cnt.get(k, 0) + 1
        print(b, 'bound', len(bound), ' '.join('%s=%d' % kv for kv in sorted(cnt.items())))
        for n in notes:
            print(n)
    print('TABLE', label, ' '.join('%s=%d' % kv for kv in t.items()))


if __name__ == '__main__':
    main()
