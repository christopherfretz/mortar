#!/usr/bin/env python3
"""s1d.py: the reference implementation of S1d, the AArch64 anchor-following
analysis of spec/dyn.md D-117 ("Bound use through a register"), WP-110 third
pass.  It is the oracle S1d's tests are checked against (record WP-110 §G).

It also implements D-117's relocation-level address rule with the m-1
boundary refinement (so one run reports both), on AArch64 objects.

usage: s1d.py [--mode=flow|linear|reset] [--calls=keep|kill] [--hits]
              [--boundary=refined|always|never] FILE...      (objects or archives)

Per object it prints one line:
  <obj> pre=<0|1> rule=<n> ana=<n> words=<n> funcs=<n> [selfgot]
  rule: uses refused by the address rule; ana: bound uses the analysis adds.
With --hits each use:  "  RULE|ANA <func>+<off> <form> addr=<T>+<a> def=<name> [boundary]".
Last line: TOTAL ... .

The default line is assumed (no -Bsymbolic, no --dynamic-list, no script): every
STV_DEFAULT global/weak/unique data definition is exported and preemptible.
"""
import sys
from elf import load_any, SHF_ALLOC, SHF_EXEC, EM_AARCH64

STB_LOCAL = 0
STT_FUNC, STT_SECTION = 2, 3
SHN_LORESERVE = 0xff00
SHF_TLS = 0x400

# relocation kinds (AArch64 ELF ABI)
R_ABS64 = 257
GOT_BY_MACH = {62: {3, 9, 41, 42, 27, 28, 30}, 3: {3, 43}}   # GOT-slot kinds: x86-64 GOT32, GOTPCREL, GOTPCRELX, REX_GOTPCRELX, GOT64, GOTPCREL64, GOTPLT64; i386 GOT32, GOT32X
R_LD_PREL_LO19 = 273
R_ADR_PREL_LO21 = 274
R_ADR_PREL_PG_HI21, R_ADR_PREL_PG_HI21_NC = 275, 276
R_ADD_ABS_LO12_NC = 277
LDST_LO12 = {278: 0, 284: 1, 285: 2, 286: 3, 299: 4}   # LDST8/16/32/64/128_ABS_LO12_NC -> log2 size
GOT_KINDS = {309, 310, 311, 312, 313, 300, 301, 302, 303, 304, 305, 306}  # GOT_LD_PREL19, ADR_GOT_PAGE, LD64_GOT_LO12_NC, LD64_GOTPAGE_LO15, MOVW_GOTOFF*, GOTREL*
# x86 PC-relative kinds whose field ends before the instruction does (the named address of a
# code field is not exact without an instruction-length decoder): x86-64 PC8/16/32/64, PLT32,
# GOTPCREL*, GOTPC32/64; i386 PC8/16/32, PLT32, GOTPC.  SIZE32/64 (x86-64) are not uses.
X86_PC = {62: {2, 4, 9, 13, 15, 24, 26, 41, 42, 29}, 3: {2, 4, 10, 23, 21}}
X86_SIZE = {62: {32, 33}, 3: {38}}


def preemptible_data(o):
    """{shndx: [Sym]}: exported preemptible data definitions (default line)."""
    out = {}
    for y in o.syms:
        if y.bind in (1, 2, 10) and y.vis == 0 and 0 < y.shndx < SHN_LORESERVE:
            if not (o.secs[y.shndx].flags & SHF_EXEC):
                out.setdefault(y.shndx, []).append(y)
    return out


def u32(o, sec, off):
    b = o.data[sec.off + off: sec.off + off + 4]
    return int.from_bytes(b, 'little') if len(b) == 4 else None


def sx(v, bits):
    return v - (1 << bits) if v & (1 << (bits - 1)) else v


# ---------------------------------------------------------------- decoding
# Each word is classified into one of these forms (a tuple: kind, fields...).

def decode(w):
    op0 = (w >> 25) & 0xf
    # --- branches
    if (w >> 26) & 0x1f == 0b00101:                       # B / BL
        off = sx(w & 0x3ffffff, 26) * 4
        return ('bl', off) if w >> 31 else ('b', off)
    if (w & 0xff000000) == 0x54000000:                     # B.cond / BC.cond
        return ('bcond', sx((w >> 5) & 0x7ffff, 19) * 4)
    if (w >> 25) & 0x3f == 0b011010:                       # CBZ / CBNZ
        return ('bcond', sx((w >> 5) & 0x7ffff, 19) * 4)
    if (w >> 25) & 0x3f == 0b011011:                       # TBZ / TBNZ
        return ('bcond', sx((w >> 5) & 0x3fff, 14) * 4)
    if (w >> 25) == 0b1101011:                             # branch (register)
        opc = (w >> 21) & 0xf
        if opc in (0b0000, 0b1000):
            return ('br',)
        if opc in (0b0001, 0b1001):
            return ('blr',)
        if opc in (0b0010, 0b0100, 0b0101):
            return ('ret',)
        return ('other_sys', w & 31)
    # --- data processing, immediate
    if (w >> 23) & 0x3f == 0b100010:                       # ADD/SUB (immediate)
        sf, op, s, sh = w >> 31, (w >> 30) & 1, (w >> 29) & 1, (w >> 22) & 1
        imm = ((w >> 10) & 0xfff) << (12 if sh else 0)
        return ('addsub', sf, -imm if op else imm, s, (w >> 5) & 31, w & 31)
    if (w >> 24) & 0x1f == 0b10000:                        # ADR / ADRP
        return ('adrp' if w >> 31 else 'adr', w & 31)
    # --- MOV (register): ORR Xd, XZR, Xm (64-bit, no shift)
    if (w & 0xffe0ffe0) == 0xaa0003e0:
        return ('mov', (w >> 16) & 31, w & 31)
    # --- loads and stores
    if op0 & 0b0101 == 0b0100:
        size, v = w >> 30, (w >> 26) & 1
        rn, rt = (w >> 5) & 31, w & 31
        b2924 = (w >> 24) & 0x3b                            # bits 29..24 with V masked
        if b2924 == 0x39:                                   # unsigned immediate
            opc = (w >> 22) & 3
            if v:
                scale = 4 if (size == 0 and opc >> 1) else size
                load = opc & 1
                prfm = False
            else:
                scale = size
                prfm = size == 3 and opc == 2
                load = opc != 0 and not prfm
            return ('ldst', ((w >> 10) & 0xfff) << scale, 'off', rn, [rt] if (load and not v) else [], load, prfm, v, size, opc)
        if b2924 == 0x38 and not (w >> 21) & 1:             # unscaled / post / unpriv / pre (imm9)
            opc = (w >> 22) & 3
            imm = sx((w >> 12) & 0x1ff, 9)
            mode = {0: 'off', 1: 'post', 2: 'off', 3: 'pre'}[(w >> 10) & 3]
            if v:
                load = opc & 1
                prfm = False
            else:
                prfm = size == 3 and opc == 2 and mode == 'off'
                load = opc != 0 and not prfm
            return ('ldst', imm, mode, rn, [rt] if (load and not v) else [], load, prfm, v, size, opc)
        if (w >> 27) & 7 == 0b101 and not (w >> 25) & 1:    # load/store pair
            opc, op2, load = w >> 30, (w >> 23) & 3, (w >> 22) & 1
            if v:
                scale = 2 + opc
            else:
                scale = 3 if opc == 2 else (4 if (opc == 1 and not load) else 2)
            imm = sx((w >> 15) & 0x7f, 7) << scale
            mode = {0: 'off', 1: 'post', 2: 'off', 3: 'pre'}[op2]
            rt2 = (w >> 10) & 31
            return ('ldst', imm, mode, rn, [rt, rt2] if (load and not v) else [], load, False, v, None, None)
        if (w >> 24) & 0x3f == 0b001000:                    # exclusive / ordered / CAS
            rs, rt2 = (w >> 16) & 31, (w >> 10) & 31
            return ('ldst', 0, 'off', rn, [rt, rt2, rs, (rs + 1) & 31], 1, False, 0, None, None)
        if b2924 == 0x38 and (w >> 21) & 1 and (w >> 10) & 3 == 0:   # atomic memory operations, LDAPR
            return ('ldst', 0, 'off', rn, [] if v else [rt], 1, False, v, None, None)
        if (w >> 24) & 0x3b == 0x18:                        # LDR (literal)
            opc = w >> 30
            return ('ldlit', sx((w >> 5) & 0x7ffff, 19) * 4, rt if not v else None, opc == 1 and not v)
        return ('ldst_other',)
    # --- branches/exception/system group, other
    if op0 & 0b1110 == 0b1010:
        return ('other_sys', w & 31)
    if op0 in (0b0000, 0b0001, 0b0011):
        return ('unalloc',)
    # data processing (immediate, other), (register), SIMD&FP, SVE: writes at most Rd
    return ('other', w & 31)


# ---------------------------------------------------------------- the analysis

class Ctx:
    pass


def analyse(o, mode='flow', calls='keep', boundary='refined', hits=None, anabnd='ruled'):
    c = Ctx()
    pd = preemptible_data(o)
    res = dict(pre=0, rule=0, ana=0, words=0, funcs=0, selfgot=0, undec=0, x86_code_pc=0, x86_tls=0)
    own = {y.name for v in pd.values() for y in v}
    if not pd:
        return res
    # local-object ends per section (m-1)
    local_ends = {}
    for y in o.syms:
        if y.bind == STB_LOCAL and 0 < y.shndx < SHN_LORESERVE and y.size:
            local_ends.setdefault(y.shndx, set()).add(y.value + y.size)
    rels_by_sec = {}
    for tgt, rels in o.relsecs():
        rels_by_sec[tgt.idx] = {r.off: r for r in rels}
    # self-GOT: a GOT relocation against one of the object's own exported data
    for tgt, rels in o.relsecs():
        for r in rels:
            if r.type in GOT_BY_MACH.get(o.mach, GOT_KINDS) and r.sym < len(o.syms) and o.syms[r.sym].bind != STB_LOCAL and o.syms[r.sym].name in own:
                res['selfgot'] = 1

    def local_in_T(symidx):
        if symidx >= len(o.syms):
            return None
        y = o.syms[symidx]
        if y.bind != STB_LOCAL or not (0 < y.shndx < SHN_LORESERVE) or y.shndx not in pd:
            return None
        return y

    def judge(T, a, at_access, where, form, kind):
        for d in pd[T]:
            if d.value <= a < d.value + max(d.size, 1):
                bnd = a == d.value and a in local_ends.get(T, ())
                if bnd:
                    if boundary == 'never':
                        return
                    if boundary == 'refined' and not at_access and res['selfgot']:
                        return
                    if kind == 'ana' and anabnd == 'access' and not at_access:
                        return
                res[kind] += 1
                if hits is not None:
                    hits.append((kind.upper(), where, form, '%s+%#x' % (o.secs[T].name, a), d.name, 'boundary' if bnd else ''))
                return

    # pre-filter: a relocation of an alloc exec section whose symbol is local and defined in some T
    prefilter = False
    for tgt, rels in o.relsecs():
        if (tgt.flags & SHF_EXEC) and (tgt.flags & SHF_ALLOC):
            if any(local_in_T(r.sym) for r in rels):
                prefilter = True
                break
    # the relocation-level address rule (D-117), for comparison
    for tgt, rels in o.relsecs():
        if not tgt.flags & SHF_ALLOC or tgt.name == '.eh_frame' or tgt.name.startswith('.gcc_except_table'):
            continue
        for r in rels:
            if r.type == 0 or r.type in X86_SIZE.get(o.mach, ()):
                continue
            y = local_in_T(r.sym)
            if not y:
                continue
            if o.mach != EM_AARCH64 and o.secs[y.shndx].flags & SHF_TLS:
                res['x86_tls'] += 1          # a TLS form names a module and offset, not an address (X-62)
                continue
            if o.mach != EM_AARCH64 and (tgt.flags & SHF_EXEC) and r.type in X86_PC.get(o.mach, ()):
                res['x86_code_pc'] += 1      # outside the rule on x86 (M-1 ruling): not judged
                continue
            at = o.mach == EM_AARCH64 and (r.type in LDST_LO12 or r.type == R_LD_PREL_LO19)
            judge(y.shndx, y.value + r.addend, at, '%s+%#x' % (tgt.name, r.off), 'reloc%d' % r.type, 'rule')
    if not prefilter or o.mach != EM_AARCH64:
        return res
    res['pre'] = 1

    # functions: distinct (section, start, end) of STT_FUNC symbols with a size, in exec sections
    funcs = sorted({(y.shndx, y.value, y.value + y.size, ) for y in o.syms
                    if y.type == STT_FUNC and y.size and 0 < y.shndx < SHN_LORESERVE and o.secs[y.shndx].flags & SHF_EXEC})
    fname = {}
    for y in o.syms:
        if y.type == STT_FUNC and y.size and 0 < y.shndx < SHN_LORESERVE:
            fname.setdefault((y.shndx, y.value, y.value + y.size), y.name)
    # mapping symbols: $d starts data, $x code
    maps = {}
    for y in o.syms:
        if y.bind == STB_LOCAL and 0 < y.shndx < SHN_LORESERVE and (y.name == '$d' or y.name.startswith('$d.') or y.name == '$x' or y.name.startswith('$x.')):
            maps.setdefault(y.shndx, []).append((y.value, y.name[1]))
    for k in maps:
        maps[k].sort()

    def is_data(sh, off):
        st = 'x'
        for v, t in maps.get(sh, ()):
            if v <= off:
                st = t
            else:
                break
        return st == 'd'

    def literal_target(sec_idx, off):
        """the ABS64 relocation at (section, off), against a local of T: (T, value)."""
        r = rels_by_sec.get(sec_idx, {}).get(off)
        if r is None or r.type != R_ABS64:
            return None
        y = local_in_T(r.sym)
        if not y:
            return None
        return (y.shndx, y.value + r.addend)

    for (sh, start, end) in funcs:
        res['funcs'] += 1
        sec = o.secs[sh]
        if sec.type == 8:
            continue
        rels = rels_by_sec.get(sh, {})
        n = (end - start) // 4
        offs = [start + 4 * i for i in range(n)]
        insns = {}
        for off in offs:
            res['words'] += 1
            if is_data(sh, off):
                insns[off] = ('data',)
            else:
                w = u32(o, sec, off)
                insns[off] = decode(w) if w is not None else ('unalloc',)
                if insns[off][0] in ('unalloc', 'ldst_other'):
                    res['undec'] += 1
        # blocks: successors of each instruction
        has_br = any(insns[x][0] == 'br' for x in offs)
        leaders = {start}
        succ = {}
        for off in offs:
            f = insns[off]
            k = f[0]
            r = rels.get(off)
            nxt = [off + 4] if off + 4 < end else []
            if k == 'b':
                tgt = None if r is not None else off + f[1]
                succ[off] = [tgt] if tgt is not None and start <= tgt < end else []
                leaders.update(succ[off])
                if off + 4 < end:
                    leaders.add(off + 4)
            elif k == 'bcond':
                tgt = None if r is not None else off + f[1]
                s = list(nxt)
                if tgt is not None and start <= tgt < end:
                    s.append(tgt)
                    leaders.add(tgt)
                succ[off] = s
                if off + 4 < end:
                    leaders.add(off + 4)
            elif k in ('br', 'ret'):
                succ[off] = []
                if off + 4 < end:
                    leaders.add(off + 4)
            else:
                succ[off] = nxt
        preds = {x: [] for x in offs}
        for x in offs:
            for s in succ[x]:
                preds[s].append(x)

        def transfer(off, st):
            st = dict(st)
            f = insns[off]
            k = f[0]
            r = rels.get(off)
            where = '%s+%#x' % (fname[(sh, start, end)], off - start)
            if k == 'data' or k == 'unalloc' or k == 'ldst_other':
                return {}
            if k in ('bl', 'blr'):
                for reg in list(st):
                    if calls == 'kill' or reg <= 18 or reg == 30:
                        del st[reg]
                return st
            if k in ('b', 'bcond', 'br', 'ret'):
                return st
            if k == 'addsub':
                _, sf, imm, s, rn, rd = f
                if r is not None:
                    if r.type == R_ADD_ABS_LO12_NC and sf and not s:
                        y = local_in_T(r.sym)
                        if y and rd != 31:
                            st[rd] = (y.shndx, y.value + r.addend)
                            return st
                    st.pop(rd, None)
                    return st
                if sf and rn != 31 and rn in st and rd != 31:
                    T, a = st[rn]
                    st[rd] = (T, a + imm)
                    judge(T, a + imm, False, where, 'add', 'ana')
                    return st
                st.pop(rd, None)
                return st
            if k == 'adr':
                rd = f[1]
                if r is not None and r.type == R_ADR_PREL_LO21:
                    y = local_in_T(r.sym)
                    if y and rd != 31:
                        st[rd] = (y.shndx, y.value + r.addend)
                        return st
                st.pop(rd, None)
                return st
            if k == 'adrp':
                st.pop(f[1], None)
                return st
            if k == 'mov':
                _, rm, rd = f
                if rm in st and rm != 31:
                    st[rd] = st[rm]
                else:
                    st.pop(rd, None)
                return st
            if k == 'ldst':
                _, imm, md, rn, written, load, prfm, v, size, opc = f
                if r is not None:
                    # a relocated load/store: the address is S+A (the address rule judges it);
                    # the literal form: a 64-bit load of an ABS64 word against T
                    newv = None
                    if r.type in LDST_LO12 and load and not v and size == 3 and opc == 1 and r.sym < len(o.syms):
                        y = o.syms[r.sym]
                        if 0 < y.shndx < SHN_LORESERVE:
                            newv = literal_target(y.shndx, y.value + r.addend)
                    for x in written:
                        st.pop(x, None)
                    if newv is not None and written:
                        st[written[0]] = newv
                    return st
                if rn != 31 and rn in st:
                    T, a = st[rn]
                    addr = a + imm if md in ('off', 'pre') else a
                    if not prfm:
                        judge(T, addr, True, where, 'ldst', 'ana')
                    if md in ('pre', 'post'):
                        st[rn] = (T, a + imm)
                        judge(T, a + imm, False, where, 'wb', 'ana')
                elif md in ('pre', 'post'):
                    st.pop(rn, None)
                for x in written:
                    st.pop(x, None)
                return st
            if k == 'ldlit':
                _, imm, rt, is64x = f
                if rt is None:
                    return st
                st.pop(rt, None)
                if is64x:
                    if r is not None and r.type == R_LD_PREL_LO19 and r.sym < len(o.syms):
                        y = o.syms[r.sym]
                        tv = literal_target(y.shndx, y.value + r.addend) if 0 < y.shndx < SHN_LORESERVE else None
                    elif r is None:
                        tv = literal_target(sh, off + imm)
                    else:
                        tv = None
                    if tv is not None:
                        st[rt] = tv
                return st
            if k == 'other_sys':
                st.pop(f[1], None)
                st.pop(30, None)
                return st
            if k == 'other':
                st.pop(f[1], None)
                return st
            raise AssertionError(k)

        if mode == 'linear':
            st = {}
            for off in offs:
                st = transfer(off, st)
            continue
        if mode == 'reset':
            st = {}
            for off in offs:
                if off in leaders:
                    st = {} if off != start else {}
                    # keep the fall-through state only when the leader is not a branch target
                    pass
                st = transfer(off, st)
            continue
        # flow: entry state of an instruction = the meet (agreement) of its predecessors'
        # exit states; the function's first instruction starts empty; computed to the
        # greatest fixpoint (unvisited predecessors are ignored = TOP); in a function
        # containing BR every leader but the first starts empty.
        TOP = None
        out = {x: TOP for x in offs}
        # iterate in address order until stable; record hits only in the final pass
        saved = hits
        for it in range(1000):
            changed = False
            for off in offs:
                if off == start:
                    ins = {}
                elif has_br and off in leaders:
                    ins = {}
                else:
                    ps = [out[p] for p in preds[off] if out[p] is not TOP]
                    if not preds[off]:
                        ins = {}
                    elif not ps:
                        continue
                    else:
                        ins = dict(ps[0])
                        for q in ps[1:]:
                            for reg in list(ins):
                                if q.get(reg) != ins[reg]:
                                    del ins[reg]
                hits = None
                save_counts = (res['ana'],)
                newo = transfer(off, ins)
                res['ana'] = save_counts[0]
                if newo != out[off]:
                    out[off] = newo
                    changed = True
            if not changed:
                break
        else:
            raise AssertionError('no fixpoint')
        hits = saved
        # final pass with the fixed states, recording the hits
        for off in offs:
            if off == start or (has_br and off in leaders) or not preds[off]:
                ins = {}
            else:
                ps = [out[p] for p in preds[off] if out[p] is not TOP]
                if not ps:
                    continue
                ins = dict(ps[0])
                for q in ps[1:]:
                    for reg in list(ins):
                        if q.get(reg) != ins[reg]:
                            del ins[reg]
            transfer_with_hits(transfer, off, ins)
    return res


def transfer_with_hits(transfer, off, ins):
    transfer(off, ins)


def main():
    mode, calls, show, boundary, anabnd = 'flow', 'keep', False, 'refined', 'ruled'
    files = []
    x86 = '--x86' in sys.argv
    for a in sys.argv[1:]:
        if a == '--x86':
            continue
        if a.startswith('--mode='):
            mode = a[7:]
        elif a.startswith('--calls='):
            calls = a[8:]
        elif a.startswith('--boundary='):
            boundary = a[11:]
        elif a.startswith('--anabnd='):
            anabnd = a[9:]
        elif a == '--hits':
            show = True
        else:
            files.append(a)
    tot = dict(objs=0, pre=0, rule_objs=0, ana_objs=0, ana_only_objs=0, rule=0, ana=0, words=0, undec=0)
    for f in files:
        for name, o in load_any(f):
            if o.mach != EM_AARCH64 and not x86:
                continue
            tot['objs'] += 1
            h = []
            r = analyse(o, mode, calls, boundary, h, anabnd)
            for k in ('pre', 'rule', 'ana', 'words', 'undec'):
                tot[k] += r[k]
            tot['rule_objs'] += r['rule'] > 0
            tot['ana_objs'] += r['ana'] > 0
            tot['ana_only_objs'] += r['ana'] > 0 and r['rule'] == 0
            if r['rule'] or r['ana'] or show:
                print(name, 'pre=%d rule=%d ana=%d words=%d funcs=%d%s' % (r['pre'], r['rule'], r['ana'], r['words'], r['funcs'], ' selfgot' if r['selfgot'] else ''))
                if show:
                    seen = set()
                    for x in h:
                        if x in seen:
                            continue
                        seen.add(x)
                        print('   ', *x)
    print('TOTAL', ' '.join('%s=%d' % kv for kv in tot.items()))


if __name__ == '__main__':
    main()
