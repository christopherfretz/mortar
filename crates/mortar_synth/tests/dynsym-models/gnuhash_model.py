# model.py OUT...: D-22 + D-26 written from the rule text alone, compared with each output.
# For every output: hashed set = defined entries + undefined with st_value != 0 (D-22);
# n = |hashed|; nbuckets = max(1, n//4); maskwords = least power of two > (12n)//W (1 if 0);
# shift 26; symoffset = index of first hashed entry (dynsym count if n = 0); hashed entries
# are the tail, sorted by bucket; bloom/buckets/chain recomputed and compared word for word.
import sys, os; sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
from elfdyn import Elf, gnu_hash
def model(e):
    syms = e.dynsyms(); W = 64 if e.c64 else 32
    hashed_idx = [i for i, s in enumerate(syms) if i and (s['shndx'] != 0 or s['value'] != 0)]
    n = len(hashed_idx)
    nb = max(1, n // 4); q = (12 * n) // W; mw = 1
    while mw <= q: mw *= 2
    so = len(syms) - n if n else len(syms)
    tail_ok = hashed_idx == list(range(len(syms) - n, len(syms)))
    buckets_seq = [gnu_hash(syms[i]['name']) % nb for i in hashed_idx]
    sorted_ok = buckets_seq == sorted(buckets_seq)
    bloom = [0] * mw; buckets = [0] * nb; chain = []
    for k, i in enumerate(hashed_idx):
        h = gnu_hash(syms[i]['name']); b = h % nb
        bloom[(h // W) % mw] |= (1 << (h % W)) | (1 << ((h >> 26) % W))
        if not buckets[b]: buckets[b] = i
        last = k + 1 == n or gnu_hash(syms[hashed_idx[k + 1]]['name']) % nb != b
        chain.append((h & ~1) | (1 if last else 0))
    return dict(n=n, nbuckets=nb, symoffset=so, maskwords=mw, shift=26, bloom=tuple(bloom), buckets=tuple(buckets), chain=tuple(chain), tail_ok=tail_ok, sorted_ok=sorted_ok)
for p in sys.argv[1:]:
    e = Elf(p); g = e.gnuhash(); m = model(e)
    diffs = [k for k in ('nbuckets', 'symoffset', 'maskwords', 'shift', 'bloom', 'buckets', 'chain') if g[k] != m[k]]
    print('%s n=%d ref(nb=%d so=%d mw=%d sh=%d) model(nb=%d so=%d mw=%d) tail=%s sorted=%s %s' % (
        os.path.basename(p), m['n'], g['nbuckets'], g['symoffset'], g['maskwords'], g['shift'],
        m['nbuckets'], m['symoffset'], m['maskwords'], m['tail_ok'], m['sorted_ok'],
        'EQUAL' if not diffs else 'DIFF:' + ','.join(diffs)))
