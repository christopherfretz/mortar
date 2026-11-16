import sys
gt = {}
for l in open(sys.argv[1]):
    p = l.split()
    if p and p[0] in ('REFUSED','ESCAPES'):
        i = p.index('unseen'); gt[p[1]] = (p[0], set(p[i+1:]))
dec = {}
for l in open(sys.argv[2]):
    p = l.split()
    if 'DEC-block' in p:
        b = p[p.index('DEC-block')+1]; f = p[p.index('DEC-func')+1]
        dec[p[0]] = (set() if b=='-' else set(b.split(',')), set() if f=='-' else set(f.split(',')))
tot=cb=cf=0
for o, (v, un) in gt.items():
    for d in un:
        tot+=1
        b,f = dec.get(o,(set(),set()))
        cb += d in b; cf += d in f
        print(v, o.split('/')[-1][:58], d[:60], 'block' if d in b else '-', 'func' if d in f else '-')
print('unseen', tot, 'decoded-block', cb, 'decoded-func', cf)
