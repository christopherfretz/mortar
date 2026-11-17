# fillcmp.py DIR SEC1NAME: for the listing tools of fillers.sh whose raw
# output differs between Y0 (no fillers) and Y (fillers), compare again
# with the filler lines removed (an STT_SECTION/LOCAL entry, or a listing
# line naming section 1 as a section symbol), the symbol index column
# dropped, the lines as a multiset, and the lines that state the table's
# own size (entry and local counts, .symtab/.strtab sizes and offsets,
# the section header table's offset) dropped. Prints per tool "same
# modulo fillers (N filler lines)" or the first differing lines.
import sys, re, collections, os
D, SEC1 = sys.argv[1], sys.argv[2]
SIZE_LINES = re.compile(r'contains \d+ entries|local symbols|Start of section headers|\.symtab|\.strtab|Number of section headers|Size of section headers')
def filt(path):
    out, nf = [], 0
    for l in open(path, errors='replace'):
        l = re.sub(r'\bY0?\b', 'F', l.rstrip('\n'))
        f = l.split()
        if ('SECTION' in f and 'LOCAL' in f) or (f and f[-1] == SEC1 and (len(f) >= 3 and f[-2] in ('r', 'n', 'd', 'N') or ' d ' in l or 'l    d' in l)):
            nf += 1; continue
        if SIZE_LINES.search(l): continue
        l = re.sub(r'^\s*\[?\s*\d+\]?:\s', '', l)        # readelf's "Num:" / eu-readelf's index
        out.append(l)
    return collections.Counter(out), nf
for n in sorted({x[3:-4] for x in os.listdir(D) if x.startswith('Y0.') and x.endswith('.out')}):
    a, b = f'{D}/Y0.{n}.out', f'{D}/Y.{n}.out'
    if not os.path.exists(b): continue
    if open(a, errors='replace').read().replace('Y0', 'F') == open(b, errors='replace').read().replace('Y', 'F'): continue
    ca, na = filt(a); cb, nb = filt(b)
    if ca == cb:
        print(f'{n:20s} same modulo fillers ({nb - na} filler lines)')
    else:
        d1 = list((ca - cb).elements())[:2]; d2 = list((cb - ca).elements())[:2]
        print(f'{n:20s} DIFFERS after filtering: only Y0 {d1} only Y {d2}')
