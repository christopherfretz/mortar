# model.py OUT LIBDIR...: D-27 from the rule text vs an output's .gnu.version_r (parsed from readelf -V).
# D-27: Verneed per needed object in DT_NEEDED (Slots.needed) order; Vernaux in the library's own
# verdef order; vna_other numbered 2.. in output order; vna_flags 0.
import re, subprocess, sys, os
out = sys.argv[1]
def rd(args): return subprocess.run(['readelf'] + args, capture_output=True, text=True).stdout
needed = re.findall(r'\(NEEDED\)\s+Shared library: \[(.*?)\]', rd(['-dW', out]))
vr = rd(['-VW', out]).split("Version needs section")[1] if "Version needs section" in rd(['-VW', out]) else ''
files = []
for line in vr.splitlines():
    m = re.search(r'File: (\S+)\s+Cnt: (\d+)', line)
    if m: files.append([m.group(1), []]); continue
    m = re.search(r'Name: (\S+)\s+Flags: (\S+)\s+Version: (\d+)', line)
    if m: files[-1][1].append((m.group(1), m.group(2), int(m.group(3))))
def find(soname):
    for d in sys.argv[2:]:
        p = os.path.join(d, soname)
        if os.path.exists(p): return p
def verdefs(path):
    t = rd(['-VW', path]); t = t.split('Version definition section')[1].split('Version needs section')[0] if 'Version definition section' in t else ''
    return [m.group(2) for m in re.finditer(r'Index: (\d+)\s+Cnt: \d+\s+Name: (\S+)', t)]
ok_order = [f for f, _ in files] == [n for n in needed if n in [f for f, _ in files]]
ok_aux = True; others = []
for f, aux in files:
    p = find(f); vd = verdefs(p) if p else None
    if vd is None: ok_aux = None; continue
    names = [a[0] for a in aux]
    if names != sorted(names, key=lambda n: vd.index(n) if n in vd else 1e9): ok_aux = False
    others += [a[2] for a in aux]
ok_other = others == list(range(2, 2 + len(others)))
flags = set(a[1] for _, aux in files for a in aux)
print('%-24s verneed-in-needed-order=%s vernaux-in-verdef-order=%s vna_other=2..=%s flags=%s  %s' % (
    os.path.basename(out), ok_order, ok_aux, ok_other, ','.join(sorted(flags)),
    ' | '.join('%s:%s' % (f, ','.join('%s/%d' % (a[0], a[2]) for a in aux)) for f, aux in files)))
