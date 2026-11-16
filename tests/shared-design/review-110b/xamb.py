#!/usr/bin/env python3
"""x86 clause attack: for every x86 PC-relative use in code through a local symbol of a
section holding exported preemptible data, compare D-117's 'least t' answer with the
TRUE named address (instruction end decoded by objdump), and list disagreements."""
import sys, subprocess, re, tempfile, os
from elf import load_any, SHF_ALLOC, SHF_EXEC, EM_386, EM_X86_64
from rule import preemptible_data, X64_PC, I386_PC
def inD(pd, T, a):
    for d in pd.get(T, ()):
        if d.value <= a < d.value + max(d.size, 1): return d.name
    return None
tot = sens = wrong = 0
for f in sys.argv[1:]:
    for name, o in load_any(f):
        if o.mach not in (EM_X86_64, EM_386): continue
        pd = preemptible_data(o)
        if not pd: continue
        starts = {}
        for y in o.syms:
            if 0 < y.shndx < 0xff00 and y.size: starts.setdefault(y.shndx, set()).add(y.value)
        cands = []
        for tgt, rels in o.relsecs():
            if not (tgt.flags & SHF_ALLOC and tgt.flags & SHF_EXEC): continue
            for r in rels:
                if r.sym >= len(o.syms): continue
                s = o.syms[r.sym]
                if s.bind != 0 or s.shndx not in pd: continue
                if not ((o.mach == EM_X86_64 and r.type in X64_PC) or (o.mach == EM_386 and r.type in I386_PC)): continue
                base = s.value + r.addend
                ans = {}
                for t in (0, 1, 2, 4): ans[t] = inD(pd, s.shndx, base + 4 + t)
                tot += 1
                if len(set(ans.values())) > 1:
                    t0 = next((t for t in (0,1,2,4) if base+4+t in starts.get(s.shndx, ())), 0)
                    cands.append((tgt, r, base, ans, t0, s.shndx))
        if not cands: continue
        # decode with objdump for the true end of the instruction holding each place
        with tempfile.NamedTemporaryFile(suffix='.o', delete=False, dir='.') as tf:
            tf.write(o.data); p = tf.name
        objd = 'x86_64-linux-gnu-objdump' if o.mach == EM_X86_64 else 'i686-linux-gnu-objdump'
        for tgt, r, base, ans, t0, T in cands:
            sens += 1
            out = subprocess.run([objd, '-d', '-j', tgt.name, p], capture_output=True, text=True).stdout
            ends = []
            for l in out.splitlines():
                m = re.match(r'\s*([0-9a-f]+):\t((?:[0-9a-f]{2} )+)', l)
                if m: ends.append((int(m.group(1), 16), len(m.group(2).split())))
            # merge continuation lines (objdump splits long insns): keep insn starts with full length
            k = None
            insns = []
            for a, n in ends:
                if insns and a == insns[-1][0] + insns[-1][1] and False: pass
                insns.append([a, n])
            # find the instruction containing r.off: the last start <= r.off whose start+len > r.off
            # objdump continuation lines appear as new 'addresses'; recombine by scanning
            full = []
            for a, n in insns:
                if full and a < full[-1][0] + full[-1][1]: continue
                if full and a == full[-1][0] + full[-1][1] and l: pass
                full.append([a, n])
            st = [x for x in insns if x[0] <= r.off]
            # true end: next instruction start after r.off as decoded (objdump -w gives one line per insn)
            out2 = subprocess.run([objd, '-dw', '-j', tgt.name, p], capture_output=True, text=True).stdout
            starts_l = []
            for l in out2.splitlines():
                m = re.match(r'\s*([0-9a-f]+):\t((?:[0-9a-f]{2} )+)', l)
                if m: starts_l.append((int(m.group(1), 16), len(m.group(2).split())))
            ins = [x for x in starts_l if x[0] <= r.off < x[0] + x[1]]
            if not ins: print('NODECODE', name, hex(r.off)); continue
            end = ins[0][0] + ins[0][1]
            k = end - (r.off + 4)
            true = inD(pd, T, base + 4 + k)
            rule = ans[t0]
            flag = 'WRONG' if (true is None) != (rule is None) or true != rule else 'ok'
            if flag == 'WRONG': wrong += 1
            print(flag, name, tgt.name, hex(r.off), 'imm', k, 'rule-t', t0, 'rule->', rule, 'true->', true)
        os.unlink(p)
print('TOTAL x86 uses', tot, 't-sensitive', sens, 'rule wrong', wrong)
