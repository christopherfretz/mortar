# Part D table. Usage: python3 -I check_d.py DDIR
# Values: f = which f L's own call reached (1 L, 2 exe, 3 M, 4 preload P);
# v = which v L read (10 L, 20 exe, 30 M, 40 P, 99 the exe's write to L's v).
import sys, re, collections
D = sys.argv[1]
LK = ["bfd", "lld22", "lld19", "mold"]
res = collections.defaultdict(dict)   # (arch, variant, case) -> lk -> value
cur = None
for line in open(f"{D}/results.txt"):
    m = re.match(r"## (\S+) (\S+) L=(\S+)", line)
    if m: cur = m.groups(); continue
    lab, rest = line[:44].strip(), line[44:].strip()
    r = re.search(r"RESULT L\.f\(\)->(\d+) L\.v->(\d+) (\S+):(\d) (\S+):(\d)", rest)
    if r and r.group(3).startswith("exe"):
        val = f"f{r.group(1)} v{r.group(2)} exe&f{'==' if r.group(4) == '1' else '!='}L&f exe&v{'==' if r.group(6) == '1' else '!='}L&v"
    elif r:
        val = f"f{r.group(1)} v{r.group(2)} dlsym(DEFAULT,f){'==' if r.group(4) == '1' else '!='}L&f dlsym(L,f){'==' if r.group(6) == '1' else '!='}L&f"
    else:
        val = "ERR:" + rest[:60]
    a, k, v = cur
    res[(a, v, lab)][k] = val
cases = []
for key in res:
    if key[2] not in cases: cases.append(key[2])
for a in ["aa", "x64"]:
    for v in ["def", "sym", "symf"]:
        print(f"#### {a}  L variant: {v}   (L&f, L&v: the addresses L's own code takes)")
        for c in cases:
            row = res.get((a, v, c), {})
            vals = [row.get(k, "-") for k in LK]
            if len(set(vals)) == 1:
                print(f"  {c:42} all: {vals[0]}")
            else:
                print(f"  {c:42} DIFFER: " + "  ".join(f"{k}={x}" for k, x in zip(LK, vals)))
print()
print("relocation/flag facts: see relocs.txt")
