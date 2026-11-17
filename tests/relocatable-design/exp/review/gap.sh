#!/bin/bash
# review/gap.sh WORK [N]: the size gap between GNU ld's and lld's -r of the bench's
# first N objects (bench/run.sh's list), split by output section name family.
# The two outputs (~570 MB) are deleted at the end, by literal name.
set -u; ulimit -c 0
W=$(realpath -m "$1"); N=${2:-400}; mkdir -p "$W"; cd "$W" || exit 1
B=/Volumes/AgentWork/mortar-helper/.build/bench-work/workloads/cxx-large-g/build
( cd "$B" && find lib -name '*.o' | LC_ALL=C sort | head -n "$N" | sed "s|^|$B/|" ) > objs.txt
ld.bfd -r -o gap.bfd.o @objs.txt; ld.lld-22 -r -o gap.lld.o @objs.txt
python3 -I - gap.bfd.o gap.lld.o <<'PY'
import sys, subprocess, re, collections
def fam(n):
    for p in ('.rela.debug', '.rela.eh_frame', '.rela.text', '.rela', '.debug_str_offsets', '.debug_str', '.debug_line_str', '.debug_info', '.debug_line', '.debug_', '.rodata.str', '.rodata', '.text', '.eh_frame', '.gcc_except_table', '.comment', '.group', '.symtab', '.strtab', '.shstrtab', '.data', '.bss'):
        if n.startswith(p): return p + ('*' if n != p else '')
    return 'other'
t = {}
for f in sys.argv[1:]:
    c = collections.Counter(); out = subprocess.run(['readelf', '-SW', f], capture_output=True, text=True).stdout
    for m in re.finditer(r'^\s*\[\s*\d+\]\s+(\S+)\s+(\S+)\s+[0-9a-f]+\s+[0-9a-f]+\s+([0-9a-f]+)', out, re.M):
        if m.group(2) != 'NOBITS': c[fam(m.group(1))] += int(m.group(3), 16)
    import os; c['(file)'] = os.path.getsize(f); t[f] = c
a, b = t[sys.argv[1]], t[sys.argv[2]]; gap = a['(file)'] - b['(file)']
print('%-22s %14s %14s %14s %7s' % ('family', 'GNU ld', 'lld', 'GNU ld - lld', '% gap'))
for k in sorted(set(a) | set(b), key=lambda k: -(a[k] - b[k])):
    d = a[k] - b[k]
    if abs(d) >= 100000 or k == '(file)': print('%-22s %14d %14d %14d %6.1f%%' % (k, a[k], b[k], d, 100.0 * d / gap))
PY
rm -f gap.bfd.o gap.lld.o
echo "gap done"
