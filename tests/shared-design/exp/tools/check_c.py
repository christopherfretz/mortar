# Part C checks. Usage: python3 -I check_c.py CDIR
import sys, os, re, collections
C = sys.argv[1]
LK = ["bfd", "lld22", "lld19", "mold"]
MODES = ["now-local", "now-global", "lazy-local", "lazy-global"]
def rd(p): return open(p, errors="replace").read() if os.path.exists(p) else ""
def evs(t): return [l for l in t.splitlines() if l.startswith(("EV ", "HOST ", "FAIL", "rc="))]

# GCC 14 -O1 emits p_ia2 before p_ia1 (and p_fa2 before p_fa1) in .init_array/.fini_array,
# so the input order is ctor_attr, ia2, ia1; glibc runs DT_INIT, then the array forwards;
# at dlclose the fini array backwards, then DT_FINI.
C_ROUND = ["HOST dlopen round {r}", "EV DT_INIT lib_init", "EV ctor_prio200", "EV ctor_attr",
           "EV init_array ia2", "EV init_array ia1", "EV cyc_call", "HOST cyc_call returned 1",
           "HOST dlclose", "EV fini_array fa1", "EV fini_array fa2", "EV dtor_attr",
           "EV dtor_prio200", "EV DT_FINI lib_fini", "HOST dlclose returned"]
C_EXPECT = [x.format(r=r) for r in (1, 2) for x in C_ROUND] + ["HOST exit", "rc=0"]

def show(name, grid):
    print(f"{'':6}{name:58}" + "".join(f"{k:>7}" for k in LK))
    for a in ["aa", "x64"]:
        for row, res in grid[a]:
            print(f"{a:6}{row:58}" + "".join(f"{r:>7}" for r in res))

grid = {"aa": [], "x64": []}
ref_seq = {}
for a in ["aa", "x64"]:
    for lib in ["libcyc_c.so", "libcyc_cxx.so", "libdel.so", "libnodel.so"]:
        for m in MODES:
            res = []
            for k in LK:
                s = evs(rd(f"{C}/{a}.{k}.{lib}.{m}.out"))
                ok = True
                if lib == "libcyc_c.so":
                    ok = s == C_EXPECT
                elif lib == "libcyc_cxx.so":
                    cnt = collections.Counter(x for x in s if x.startswith("EV"))
                    ok = all(v == 2 or x == "EV cyc_call" and v == 2 for x, v in cnt.items()) and len(cnt) >= 9
                    # S dtors run inside dlclose (from __cxa_finalize), not at exit
                    for r in (1, 2):
                        try:
                            i = s.index("HOST dlclose", s.index(f"HOST dlopen round {r}")); j = s.index("HOST dlclose returned", i)
                        except ValueError:
                            ok = False; continue
                        inside = s[i:j]
                        ok &= all(f"EV S dtor {n}" in inside for n in ("s1", "s2", "s3-function-local"))
                    ok &= s.count("HOST dso_handle distinct") == 2 and s[-1] == "rc=0"
                    ref = ref_seq.setdefault((a, lib), s)
                    ok &= s == ref
                elif lib == "libdel.so":
                    ok = s.count("EV nodelete ctor") == 2 and s.count("EV nodelete dtor") == 2 and s.count("HOST cyc_call returned 1") == 2
                else:
                    ok = (s.count("EV nodelete ctor") == 1 and s.count("EV nodelete dtor") == 1
                          and "HOST cyc_call returned 2" in s and s[-2:] == ["EV nodelete dtor", "rc=0"])
                res.append("ok" if ok else "FAIL")
            grid[a].append((f"{lib} {m}", res))
    for row, fn in [
        ("tls: 2 threads + main, values, distinct &t", lambda t: "OK tls" in t),
        ("tls: T ctor x3, T dtor x3, main's dtor at exit", lambda t: t.count("EV T ctor") == 3 and t.count("EV T dtor") == 3 and re.search(r"HOST exit\nEV T dtor v=107", t)),
    ]:
        grid[a].append((row, ["ok" if fn(rd(f"{C}/{a}.{k}.tls.out")) else "FAIL" for k in LK]))
    for t in range(8):
        grid[a].append((f"exc t{t} (with .eh_frame_hdr)", ["ok" if re.search(rf"^OK t{t} ", rd(f"{C}/{a}.{k}.exc.out"), re.M) and not re.search(rf"^FAIL t{t} ", rd(f"{C}/{a}.{k}.exc.out"), re.M) else "FAIL" for k in LK]))
    grid[a].append(("exc all, rc 0", ["ok" if "OK exc-all" in rd(f"{C}/{a}.{k}.exc.out") and "rc=0" in rd(f"{C}/{a}.{k}.exc.out") else "FAIL" for k in LK]))
    for t in range(8):
        r = []
        for k in LK:
            x = rd(f"{C}/{a}.{k}.exc-noeh.out")
            m = re.search(rf"t{t} rc=(\d+)", x)
            r.append("ok" if m and m.group(1) == "0" else ("rc" + m.group(1) if m else "?"))
        grid[a].append((f"exc t{t} --no-eh-frame-hdr lib", r))
    grid[a].append(("fortran main: module proc + common", ["ok" if "OK fortran main" in rd(f"{C}/{a}.{k}.fmain.out") else "FAIL" for k in LK]))
    grid[a].append(("fortran main: block data init seen (1110)", ["ok" if "add_twice(5)=1110" in rd(f"{C}/{a}.{k}.fmain.out") else "FAIL" for k in LK]))
    grid[a].append(("C main: bind(c) proc + bind(c) common", ["ok" if "OK C main" in rd(f"{C}/{a}.{k}.fcmain.out") else "FAIL" for k in LK]))
show("check", grid)
print()
for (a, lib), s in ref_seq.items():
    print(f"reference sequence {a} {lib} (bfd, now-local):")
    for x in s: print("   ", x)
print("C library expected sequence (all linkers must match):")
for x in C_EXPECT[:16]: print("   ", x)
for a in ["aa", "x64"]:
    for k in LK:
        x = rd(f"{C}/{a}.{k}.exc-noeh.out")
        msgs = sorted(set(l for l in x.splitlines() if l.startswith("terminate")))
        print(f"no-eh-frame-hdr {a} {k}: " + " | ".join(msgs))
