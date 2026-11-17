# secdiff.py BASE OUT: what OUT has that BASE lacks: sections (name flags size), the
# .note.gnu.property and .note.GNU-stack contents, symbol count.
import sys, subprocess, re
def info(f):
    s = subprocess.run(["readelf", "-SWsn", f], capture_output=True, text=True).stdout
    secs = {}
    for m in re.finditer(r"^\s*\[\s*\d+\]\s+(\S+)\s+\S+\s+[0-9a-f]+\s+[0-9a-f]+\s+([0-9a-f]+)\s+[0-9a-f]+\s+([A-Z]*)\s", s, re.M):
        secs[m.group(1)] = (m.group(3), int(m.group(2), 16))
    props = sorted(set(l.strip() for l in s.splitlines() if re.search(r"(feature|ISA|stack size|x86|AArch64|GCS|BTI|memory seal|needed|used):", l, re.I)))
    nsym = len(re.findall(r"^\s+\d+: ", s, re.M))
    return secs, props, nsym
b, o = info(sys.argv[1]), info(sys.argv[2])
out = []
for n, v in o[0].items():
    if n not in b[0]: out.append("+%s(%s,%d)" % (n, v[0], v[1]))
    elif b[0][n] != v and not n.startswith((".symtab", ".strtab", ".shstrtab")): out.append("%s %s->%s" % (n, b[0][n], v))
for n in b[0]:
    if n not in o[0]: out.append("-" + n)
if o[1] != b[1]: out.append("notes: " + " | ".join(o[1]))
if o[2] != b[2]: out.append("symbols %d->%d" % (b[2], o[2]))
print("; ".join(out) or "differs in bytes only")
