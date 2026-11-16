#!/usr/bin/env python3
"""Q21 option (a), two scopes, AArch64: from each `add xD, xN, :lo12:SEC+A` (anchor),
follow xD through `add xR, xD, #imm`, `mov xR, xD` and load/store `[xD, #imm]` and report
anchor+imm addresses inside an exported preemptible data definition.
scope 'block': stop at the end of the basic block (a branch, or a label/branch target).
scope 'func' : linear sweep to the end of the function (kill a register on any other write).
usage: anchordec.py OBJ...  -> per object 'DEC-block n' 'DEC-func n' and names."""
import sys, re, subprocess
from elf import load_any, EM_AARCH64
from rule import preemptible_data
INS = re.compile(r'^\s*([0-9a-f]+):\t[0-9a-f]{8} \t(\S+)\s*(.*)$')
REL = re.compile(r'^\s*([0-9a-f]+): (R_AARCH64_\w+)\s+(\S+?)(?:\+0x([0-9a-f]+)|-0x([0-9a-f]+))?$')
FN = re.compile(r'^[0-9a-f]+ <(.*)>:$')
BR = ('b', 'br', 'ret', 'b.', 'cbz', 'cbnz', 'tbz', 'tbnz', 'bl', 'blr')
def regs(s): return re.findall(r'\b([xw]\d+|sp)\b', s)
def norm(r): return 'x' + r[1:] if r[0] in 'xw' else r
tot = dict(objs=0, block=0, func=0)
for f in sys.argv[1:]:
    for name, o in load_any(f):
        if o.mach != EM_AARCH64: continue
        pd = preemptible_data(o)
        secname = {o.secs[k].name: k for k in pd}
        if not secname: continue
        tot['objs'] += 1
        out = subprocess.run(['objdump', '-dr', '--no-show-raw-insn', f] if False else ['objdump', '-dr', f], capture_output=True, text=True).stdout.splitlines()
        # targets of branches (basic-block leaders)
        leaders = set()
        for l in out:
            m = INS.match(l)
            if m and (m.group(2) in BR or m.group(2).startswith('b.')):
                t = re.match(r'([0-9a-f]+) <', m.group(3).split(',')[-1].strip())
                if t: leaders.add(int(t.group(1), 16))
        hits = {'block': set(), 'func': set()}
        for scope in ('block', 'func'):
            anchors = {}
            pending = None
            for l in out:
                if FN.match(l): anchors = {}; continue
                r = REL.match(l)
                if r:
                    if r.group(2) == 'R_AARCH64_ADD_ABS_LO12_NC' and r.group(3) in secname and pending:
                        add = int(r.group(4), 16) if r.group(4) else (-int(r.group(5), 16) if r.group(5) else 0)
                        anchors[pending] = (secname[r.group(3)], add)
                    continue
                m = INS.match(l)
                if not m: continue
                addr, op, args = int(m.group(1), 16), m.group(2), m.group(3)
                if scope == 'block' and addr in leaders: anchors = {}
                rs = [norm(x) for x in regs(args)]
                pending = rs[0] if op == 'add' and rs else None
                def chk(T, a):
                    for d in pd[T]:
                        if d.value <= a < d.value + max(d.size, 1): hits[scope].add(d.name)
                mm = re.match(r'([xw]\d+|sp), ([xw]\d+|sp), #(0x[0-9a-f]+|\d+)$', args)
                if op == 'add' and mm and norm(mm.group(2)) in anchors:
                    T, a = anchors[norm(mm.group(2))]; a += int(mm.group(3), 0); chk(T, a)
                    anchors[norm(mm.group(1))] = (T, a); continue
                if op == 'mov' and len(rs) == 2 and rs[1] in anchors:
                    anchors[rs[0]] = anchors[rs[1]]; continue
                mm = re.search(r'\[([xw]\d+|sp)(?:, #(-?0x[0-9a-f]+|-?\d+))?\]', args)
                if op.startswith(('ld', 'st')) and mm and norm(mm.group(1)) in anchors:
                    T, a = anchors[norm(mm.group(1))]; a += int(mm.group(2), 0) if mm.group(2) else 0; chk(T, a)
                    # the loaded/stored registers are written by loads
                    if op.startswith('ld'):
                        for x in rs[:-1]: anchors.pop(x, None)
                    continue
                if rs and not op.startswith(('st', 'cmp', 'tst', 'cb', 'tb', 'b')) and op != 'add':
                    anchors.pop(rs[0], None)
                elif op == 'add' and rs and pending is None:
                    anchors.pop(rs[0], None)
                if scope == 'block' and (op in BR or op.startswith('b.')): anchors = {}
                if op in ('bl', 'blr'):
                    for k in [k for k in anchors if k[0] == 'x' and k[1:].isdigit() and int(k[1:]) < 19]: anchors.pop(k)
        for s in ('block', 'func'):
            if hits[s]: tot[s] += 1
        print(name, 'DEC-block', ','.join(sorted(hits['block'])) or '-', 'DEC-func', ','.join(sorted(hits['func'])) or '-')
print('TOTAL', tot)
