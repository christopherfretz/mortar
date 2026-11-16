#!/usr/bin/env python3
"""relscan.py: the B-1 measurement (WP-110 second pass).

For every relocatable object (or archive member) given, find the uses whose
target is a LOCAL symbol (STT_SECTION or a named local) plus addend that name
an address inside an exported, preemptible definition of the same input
section: a definition with STB_GLOBAL/STB_WEAK/STB_GNU_UNIQUE binding,
STV_DEFAULT visibility, in a section of the object, whose interval is
[st_value, st_value + max(st_size, 1)).

The address a use names is  sym.st_value + A + k  where A is the addend
(RELA field, or the implicit addend read from the place for REL) and k is the
PC bias of the class: 4 for x86-64 / i386 PC-relative fields in executable
sections (the field ends the instruction in every compiler form measured; the
'bias' column reports the uses where another k would change the answer), else 0.

Classes (per relocation type, per target): GOT, BRANCH, PC, LO12 (AArch64
absolute low halves paired with ADRP), ABS (address-sized word), NARROW,
GOTOFF (i386 GOTOFF / x86-64 GOTOFF64 / a64 GOTREL), TLS, OTHER.
Place kinds: CODE (alloc+exec), DATA (alloc, not exec, not unwind), EH
(.eh_frame), LSDA (.gcc_except_table*), NONALLOC (no SHF_ALLOC).

Output: one line per hit (with --hits) and a summary keyed by
(class, place, target kind, how: inside|start|start-and-end-of-local).
Target kind: DATA (definition in a section without SHF_EXECINSTR), FUNC
(definition in an executable section).
Also the per-object 'anchor' facts used by the over-approximation:
  sectionuse: the object has an observing (CODE/DATA place) non-GOT use of a
  local symbol in a section T that holds an exported preemptible DATA
  definition; gotnamed: the set of such definitions named by a GOT-class use.
"""
import struct, sys, os, collections, argparse

SHF_WRITE, SHF_ALLOC, SHF_EXEC, SHF_TLS = 1, 2, 4, 0x400
SHT_SYMTAB, SHT_RELA, SHT_REL, SHT_NOBITS = 2, 4, 9, 8

def a64_class(t):
    if 300 <= t <= 306 or 309 <= t <= 313: return 'GOT'     # MOVW_GOTOFF_G*, GOT_LD_PREL19, LD64_GOTOFF_LO15, ADR_GOT_PAGE, LD64_GOT_LO12_NC, LD64_GOTPAGE_LO15
    if t in (307, 308): return 'GOTOFF'                       # GOTREL64/32: S+A-GOT
    if t in (279, 280, 282, 283): return 'BRANCH'
    if t in (260, 261, 262, 273, 274, 275, 276) or 287 <= t <= 294: return 'PC'
    if t in (277, 278, 284, 285, 286, 299): return 'LO12'
    if t == 257: return 'ABS'
    if t in (258, 259) or 263 <= t <= 272: return 'NARROW'
    if 512 <= t <= 573: return 'TLS'
    if t == 0: return 'NONE'
    return 'OTHER'

def x64_class(t):
    if t in (3, 9, 27, 28, 41, 42, 43, 26, 29, 30): return 'GOT'
    if t == 4: return 'BRANCH'
    if t in (2, 13, 15, 24): return 'PC'
    if t == 1: return 'ABS'
    if t in (10, 11, 12, 14): return 'NARROW'
    if t in (25, 31): return 'GOTOFF'
    if 16 <= t <= 23 or t in (34, 35, 36): return 'TLS'
    if t in (32, 33): return 'SIZE'
    if t == 0: return 'NONE'
    return 'OTHER'

def i386_class(t):
    if t in (3, 43, 10): return 'GOT'
    if t == 4: return 'BRANCH'
    if t in (2, 21, 23): return 'PC'
    if t == 1: return 'ABS'
    if t in (20, 22): return 'NARROW'
    if t == 9: return 'GOTOFF'
    if 14 <= t <= 19 or 24 <= t <= 37 or t in (39, 40, 41): return 'TLS'
    if t == 0: return 'NONE'
    return 'OTHER'

MACH = {183: ('a64', a64_class), 62: ('x64', x64_class), 3: ('i386', i386_class)}

def members(path):
    data = open(path, 'rb').read()
    if not data.startswith(b'!<arch>\n'):
        yield path, data; return
    off, longnames = 8, b''
    while off + 60 <= len(data):
        hdr = data[off:off + 60]; name = hdr[:16].decode('latin1').strip(); size = int(hdr[48:58])
        body = data[off + 60: off + 60 + size]
        if name == '//': longnames = body
        elif name in ('/', '/SYM64/'): pass
        else:
            if name.startswith('/') and name[1:].isdigit():
                i = int(name[1:]); name = longnames[i:longnames.index(b'/\n', i)].decode('latin1')
            yield path + '(' + name.rstrip('/') + ')', body
        off += 60 + size + (size & 1)

def scan(name, d, hits_out, summ, objfacts, args):
    if d[:4] != b'\x7fELF' or struct.unpack_from('<H', d, 16)[0] != 1: return
    c64 = d[4] == 2
    e_machine = struct.unpack_from('<H', d, 18)[0]
    if e_machine not in MACH: return
    arch, klass = MACH[e_machine]
    if c64:
        shoff, = struct.unpack_from('<Q', d, 0x28); shentsize, shnum, shstrndx = struct.unpack_from('<HHH', d, 0x3a)
    else:
        shoff, = struct.unpack_from('<I', d, 0x20); shentsize, shnum, shstrndx = struct.unpack_from('<HHH', d, 0x2e)
    secs = []
    for i in range(shnum):
        o = shoff + i * shentsize
        if c64:
            nm, ty, fl, ad, of, sz, ln, inf, al, es = struct.unpack_from('<IIQQQQIIQQ', d, o)
        else:
            nm, ty, fl, ad, of, sz, ln, inf, al, es = struct.unpack_from('<IIIIIIIIII', d, o)
        secs.append(dict(nm=nm, ty=ty, fl=fl, of=of, sz=sz, ln=ln, inf=inf, es=es))
    shstr = secs[shstrndx]
    def sname(s):
        b = shstr['of'] + s['nm']; return d[b:d.index(b'\0', b)].decode('latin1')
    for s in secs: s['name'] = sname(s)
    symtab = next((s for s in secs if s['ty'] == SHT_SYMTAB), None)
    if symtab is None: return
    strt = secs[symtab['ln']]
    syms = []
    ent = 24 if c64 else 16
    for i in range(symtab['sz'] // ent):
        o = symtab['of'] + i * ent
        if c64:
            nm, info, other, shndx, val, size = struct.unpack_from('<IBBHQQ', d, o)
        else:
            nm, val, size, info, other, shndx = struct.unpack_from('<IIIBBH', d, o)
        b = strt['of'] + nm; n = d[b:d.index(b'\0', b)].decode('latin1')
        syms.append(dict(name=n, val=val, size=size, bind=info >> 4, type=info & 15, vis=other & 3, shndx=shndx))
    # exported preemptible definitions per section
    exp = collections.defaultdict(list)
    for s in syms:
        if s['bind'] in (1, 2, 10) and s['vis'] == 0 and 0 < s['shndx'] < 0xff00 and s['type'] != 3:
            sec = secs[s['shndx']]
            s['tk'] = 'FUNC' if sec['fl'] & SHF_EXEC else 'DATA'
            exp[s['shndx']].append(s)
    # local objects with a size (for the one-past-the-end boundary)
    locends = collections.defaultdict(set)
    for s in syms:
        if s['bind'] == 0 and s['type'] in (1, 6) and s['size'] > 0 and 0 < s['shndx'] < 0xff00:
            locends[s['shndx']].add(s['val'] + s['size'])
    datasecs = {k for k, v in exp.items() if any(x['tk'] == 'DATA' for x in v)}
    starts = collections.defaultdict(set)   # st_value of every sized symbol, per section (the x86 tail rule)
    for s in syms:
        if s['size'] > 0 and 0 < s['shndx'] < 0xff00 and s['type'] != 3: starts[s['shndx']].add(s['val'])
    facts = objfacts.setdefault(name, dict(arch=arch, sectionuse=set(), gotnamed=set(), datadefs=set(), hit=set(), symnamed=set(), rulehit=set()))
    for k in datasecs:
        for x in exp[k]:
            if x['tk'] == 'DATA': facts['datadefs'].add((k, x['name']))
    for rs in secs:
        if rs['ty'] not in (SHT_RELA, SHT_REL): continue
        tgt = secs[rs['inf']] if rs['inf'] < len(secs) else None
        if tgt is None: continue
        if tgt['fl'] & SHF_ALLOC == 0: place = 'NONALLOC'
        elif tgt['name'] == '.eh_frame': place = 'EH'
        elif tgt['name'].startswith('.gcc_except_table'): place = 'LSDA'
        elif tgt['fl'] & SHF_EXEC: place = 'CODE'
        else: place = 'DATA'
        rela = rs['ty'] == SHT_RELA
        es = rs['es'] or ((24 if c64 else 12) if rela else (16 if c64 else 8))
        for i in range(rs['sz'] // es):
            o = rs['of'] + i * es
            if c64:
                if rela: r_off, r_info, r_add = struct.unpack_from('<QQq', d, o)
                else: (r_off, r_info), r_add = struct.unpack_from('<QQ', d, o), None
                r_sym, r_type = r_info >> 32, r_info & 0xffffffff
            else:
                if rela: r_off, r_info, r_add = struct.unpack_from('<IIi', d, o)
                else: (r_off, r_info), r_add = struct.unpack_from('<II', d, o), None
                r_sym, r_type = r_info >> 8, r_info & 0xff
            cls = klass(r_type)
            if r_sym == 0 or r_sym >= len(syms): continue
            sy = syms[r_sym]
            if cls == 'GOT' and sy['bind'] != 0:
                facts['gotnamed'].add((sy['shndx'], sy['name']))
            if cls not in ('GOT', 'NONE', 'SIZE', 'TLS') and place in ('CODE', 'DATA') and sy['bind'] != 0 and sy['vis'] == 0 and 0 < sy['shndx'] < 0xff00:
                facts['symnamed'].add((sy['shndx'], sy['name']))   # a non-GOT use naming the exported symbol itself: D-117's refusal
            if sy['bind'] != 0 or not (0 < sy['shndx'] < 0xff00): continue
            T = sy['shndx']
            if r_add is None:  # REL: implicit addend, a 32-bit field for every i386 class counted
                if tgt['ty'] == SHT_NOBITS or r_off + 4 > tgt['sz']: continue
                r_add, = struct.unpack_from('<i', d, tgt['of'] + r_off)
            k = 4 if (arch in ('x64', 'i386') and cls in ('PC', 'BRANCH') and place == 'CODE') else 0
            addr = sy['val'] + r_add + k
            # THE RULE's named address (D-117 'bound use'): x86 PC-relative fields in code: S+A+4+t, t the
            # least of 0,1,2,4 at which the address is the start of a sized symbol of the section, else 0
            raddr = addr
            if k:
                for t in (0, 1, 2, 4):
                    if addr + t in starts[T]: raddr = addr + t; break
            if place in ('CODE', 'DATA'):
                for x in exp.get(T, ()):
                    if x['tk'] == 'DATA' and x['val'] <= raddr < x['val'] + max(x['size'], 1):
                        summ[(arch, 'RULE', cls, place, 'start' if raddr == x['val'] else 'inside', 'section' if sy['type'] == 3 else 'named-local')] += 1
                        facts['rulehit'].add((T, x['name']))
                        if args.hits:
                            hits_out.append('%s\tRULE\t%s\t%s\t%s\t%s+%d\t%s\t%s' % (name, arch, cls, place, sy['name'] or secs[T]['name'], r_add, x['name'], 'start=end-of-local' if raddr == x['val'] and raddr in locends[T] else ''))
            if place in ('CODE', 'DATA') and cls not in ('GOT', 'NONE', 'SIZE') and T in datasecs:
                facts['sectionuse'].add(T)
            for x in exp.get(T, ()):
                lo, hi = x['val'], x['val'] + max(x['size'], 1)
                inside = lo <= addr < hi
                # what a bias of k=0 or k up to 12 would have said (x86 only)
                alt = False
                if arch in ('x64', 'i386') and cls in ('PC', 'BRANCH') and place == 'CODE':
                    alt = any(lo <= sy['val'] + r_add + kk < hi for kk in range(0, 13)) and not inside
                if not inside and not alt: continue
                how = 'start' if addr == lo else 'inside'
                if inside and addr == lo and addr in locends[T]: how = 'start=end-of-local'
                if alt: how = 'only-with-other-bias'
                key = (arch, cls, place, x['tk'], how, 'section' if sy['type'] == 3 else 'named-local')
                summ[key] += 1
                if place in ('CODE', 'DATA') and cls != 'GOT' and x['tk'] == 'DATA':
                    facts['hit'].add((T, x['name']))
                if args.hits:
                    hits_out.append('%s\t%s\t%s\t%s\t%s\t%s+%d\t%s\t%s\t%s' % (name, arch, cls, r_type, place, sy['name'] or secs[T]['name'], r_add, x['name'], x['tk'], how))

def main():
    ap = argparse.ArgumentParser()
    ap.add_argument('--hits', action='store_true')
    ap.add_argument('--label', default='')
    ap.add_argument('files', nargs='*')
    args = ap.parse_args()
    files = args.files or [l.strip() for l in sys.stdin if l.strip()]
    summ = collections.Counter(); hits = []; facts = {}
    nobj = 0
    for f in files:
        for n, d in members(f):
            nobj += 1
            scan(n, d, hits, summ, facts, args)
    lab = args.label
    print('%s objects %d' % (lab, nobj))
    for k in sorted(summ): print('%s use\t%s\t%d' % (lab, '\t'.join(k), summ[k]))
    # over-approximation: objects with a data section holding an exported preemptible
    # definition AND an observing non-GOT local use of that section
    ov = [(n, f) for n, f in facts.items() if f['sectionuse']]
    print('%s overapprox-objects %d' % (lab, len(ov)))
    # of those, the sections where some exported data definition is named by no GOT use and hit by no use
    unexpl = 0
    for n, f in ov:
        for T in f['sectionuse']:
            names = [nm for (t, nm) in f['datadefs'] if t == T]
            un = sorted(nm for nm in names if (T, nm) not in f['gotnamed'] and (T, nm) not in f['hit'] and (T, nm) not in f['symnamed'])
            if un:
                unexpl += 1
                if args.hits: hits.append('%s\tOVERAPPROX\tsection %d unnamed %s' % (n, T, ','.join(un)))
                break
    print('%s overapprox-objects-with-unnamed-def %d' % (lab, unexpl))
    print('%s objects-with-data-hit %d' % (lab, sum(1 for f in facts.values() if f['hit'])))
    print('%s RULE-objects-refused %d' % (lab, sum(1 for f in facts.values() if f['rulehit'])))
    for h in hits: print('HIT\t' + h)

if __name__ == '__main__':
    main()
