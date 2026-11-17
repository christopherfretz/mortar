# anchors.py ORIG OUT: MT-16 section anchors of the units of ORIG (a kiln object) read
# back in OUT (ORIG after a -r or an objcopy). For each anchored section S of ORIG
# (not .symtab/.strtab/.shstrtab/.rela*/.mason.meta/.group) the triple (name, group
# signature, ordinal among ORIG's sections of that name and signature) is resolved in
# OUT as a reader of OUT must: the ordinal-th section of that name and signature.
# Truth: where S's first function symbol (or, without one, S's bytes) landed.
# Readers: D0 today's MT-16 (more than one unit => unanchored); D1 each anchored
# section's size recorded (holds iff the resolved section has that size); D2 a hash of
# the unit's object's section list recorded (holds iff OUT's recomputed list hashes the
# same); D3 an anchor symbol at offset 0 of S (here the function symbol stands in for it):
# holds iff present, and it gives section and offset.
import sys, subprocess, re, hashlib
def elf(f):
    out = subprocess.run(['readelf', '-SWsg', '-x', '.mason.meta', f], capture_output=True, text=True).stdout
    secs = []
    for m in re.finditer(r'^\s*\[\s*(\d+)\]\s+(\S+)\s+(\S+)\s+[0-9a-f]+\s+([0-9a-f]+)\s+([0-9a-f]+)\s+\S+\s+(\S*)\s', out, re.M):
        secs.append(dict(i=int(m.group(1)), name=m.group(2), type=m.group(3), off=int(m.group(4), 16), size=int(m.group(5), 16), flags=m.group(6), sig=''))
    for m in re.finditer(r'^(?:COMDAT )?group section \[\s*\d+\] `[^\']*\' \[([^\]]*)\][^\n]*\n((?:\s+\[\s*\d+\][^\n]*\n)+)', out, re.M):
        for i in re.findall(r'\[\s*(\d+)\]', m.group(2)):
            for s in secs:
                if s['i'] == int(i): s['sig'] = m.group(1)
    syms = [(int(m.group(1), 16), m.group(2), int(m.group(3)), m.group(4)) for m in re.finditer(r'^\s+\d+: ([0-9a-f]+)\s+\d+\s+(\w+)\s+\w+\s+\w+\s+(\d+)\s+(\S+)$', out, re.M)]
    data = open(f, 'rb').read()
    units = len(re.findall(rb'MASN', b''.join(data[s['off']:s['off'] + s['size']] for s in secs if s['name'] == '.mason.meta')))
    return secs, syms, data, units
SKIP = ('NULL', '.symtab', '.strtab', '.shstrtab', '.mason.meta', '.group', '.symtab_shndx')
def anchored(secs): return [s for s in secs if s['type'] not in ('RELA', 'REL', 'NULL', 'GROUP') and s['name'] not in SKIP]
def listhash(secs): return hashlib.sha1(repr([(s['name'], s['sig'], s['type'], s['flags'].replace('G', ''), s['size']) for s in anchored(secs)]).encode()).hexdigest()
o, x = elf(sys.argv[1]), elf(sys.argv[2])
fp_orig = listhash(o[0])
for s in anchored(o[0]):
    same = [t for t in anchored(o[0]) if (t['name'], t['sig']) == (s['name'], s['sig'])]
    ordn = same.index(s); triple = (s['name'], s['sig'], ordn)
    cand = [t for t in anchored(x[0]) if (t['name'], t['sig']) == (s['name'], s['sig'])]
    res = cand[ordn] if ordn < len(cand) else None
    fs = [y for y in o[1] if y[2] == s['i'] and y[1] in ('FUNC', 'OBJECT')]
    if fs:
        v, _, _, nm = fs[0]; hit = [y for y in x[1] if y[3] == nm]
        where = (hit[0][2], hit[0][0] - v) if hit else None; how = 'symbol ' + nm
    else:
        b = o[2][s['off']:s['off'] + s['size']]; where = None; how = 'bytes'
        if s['type'] == 'NOBITS' or not b: where = ('?', 0) if res else None
        else:
            for t in anchored(x[0]):
                k = x[2][t['off']:t['off'] + t['size']].find(b)
                if k >= 0 and (t['name'], t['sig']) == (s['name'], s['sig']): where = (t['i'], k); break
    truth = 'holds' if (res and where and (where[0] == res['i'] or where[0] == '?') and where[1] == 0) else ('MOVED to offset %d of the resolved section' % where[1] if res and where and where[0] == res['i'] else 'BROKEN')
    d0 = 'unanchored' if x[3] > 1 else 'holds'
    d1 = 'holds' if res and res['size'] == s['size'] else 'unanchored'
    d2 = 'holds' if listhash(x[0]) == fp_orig else 'unanchored'
    d3 = ('holds (offset %d)' % where[1]) if fs and where else ('n/a' if not fs else 'unanchored')
    bad = lambda d: ' (WRONG)' if d == 'holds' and truth != 'holds' else ''
    print('  %-34s units %d truth %-12s | D0 %s%s | D1 %s%s | D2 %s%s | D3 %s' % (str(triple), x[3], truth, d0, bad(d0), d1, bad(d1), d2, bad(d2), d3))
