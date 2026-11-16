# Summarise part B's per-linker logs. Usage: python3 -I check_b.py BDIR
import sys, os, re, collections
B = sys.argv[1]
LK = ["bfd", "lld22", "lld19", "mold"]
for a in ["aa", "x64"]:
    print(f"######## {a}")
    syms = {}
    for k in LK:
        p = os.path.join(B, f"{a}.{k}")
        # elflint: group messages by text, count libraries
        msgs = collections.defaultdict(list); clean = []; cur = None
        for line in open(p + ".elflint"):
            line = line.rstrip("\n")
            m = re.match(r"== (\S+) rc=(\d+)", line)
            if m:
                cur = m.group(1)
                if m.group(2) == "0": clean.append(cur)
                continue
            if line and cur and line != "No errors":
                # group by message shape: symbol/section/entry numbers vary per library
                g = re.sub(r"symbol \d+", "symbol N", line)
                g = re.sub(r"section \[ *\d+\]", "section [N]", g)
                g = re.sub(r"index \d+", "index N", g)
                g = re.sub(r"entry \d+", "entry N", g)
                msgs[g].append(cur)
        print(f"--- {a} {k} eu-elflint --gnu-ld: {len(clean)} clean")
        for msg, libs in sorted(msgs.items()):
            print(f"   [{len(set(libs))} libs, {len(libs)}x, e.g. {libs[0]}] {msg}")
        # readelf warnings
        warn = collections.defaultdict(list); cur = None
        for line in open(p + ".readelf"):
            line = line.rstrip("\n")
            if line.startswith("== "): cur = line[3:]; continue
            if line: warn[line].append(cur)
        print(f"--- {a} {k} readelf -a stderr: {sum(len(v) for v in warn.values())} lines")
        for msg, libs in sorted(warn.items()):
            print(f"   [{len(libs)}] {msg}")
        v = collections.Counter(l.split("verify-rc=")[1].strip() for l in open(p + ".verify"))
        print(f"--- {a} {k} ld.so --verify exit codes: {dict(v)}")
        bad = [l.strip() for l in open(p + ".verify") if not l.strip().endswith("rc=2")]
        for b in bad: print("    not 2:", b)
        txt = open(p + ".ldd").read()
        nf = [l for l in txt.splitlines() if "not found" in l or "error" in l.lower() or "Segmentation" in l]
        rcs = collections.Counter(re.findall(r"rc=(\d+)", txt))
        print(f"--- {a} {k} ldd: exit codes {dict(rcs)}; problem lines {len(nf)}")
        for l in nf[:10]: print("   ", l)
        # symbol tables
        cur = None; s = collections.defaultdict(set); cnt = {}
        for line in open(p + ".syms"):
            line = line.rstrip("\n")
            if line.startswith("== "): cur = line[3:]; continue
            if line.startswith("nm-D="): cnt[cur] = line; continue
            s[cur].add(line)
        syms[k] = (s, cnt)
        mism = [t for t, c in cnt.items() if len(set(re.findall(r"=(\d+)", c))) != 1]
        print(f"--- {a} {k} nm -D vs objdump -T entry counts disagree on {len(mism)} libs: {mism[:3]}")
    base, _ = syms["bfd"]
    for k in LK[1:]:
        s, _ = syms[k]
        for t in sorted(base):
            if base[t] != s.get(t, set()):
                print(f"   DEFINED-DYNSYM DIFF {a} {k} vs bfd {t}: only-bfd={sorted(base[t]-s.get(t,set()))[:8]} only-{k}={sorted(s.get(t,set())-base[t])[:8]}")
print()
print("ld.so --verify controls (exit codes: 0 = dynamic with PT_INTERP, 2 = shared object without one, 1 = not dynamic/not ELF):")
print(open(os.path.join(B, "verify-controls.txt")).read())
