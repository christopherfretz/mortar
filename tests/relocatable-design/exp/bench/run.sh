#!/bin/bash
# bench/run.sh WORK [N]: time and peak RSS of each reference's ld -r over the
# first N (default 400, sorted by path) objects of the cxx-large-g build
# (read-only), three runs each; sizes of the inputs and outputs.
ulimit -c 0
W=$1; N=${2:-400}
B=/Volumes/AgentWork/mortar-helper/.build/bench-work/workloads/cxx-large-g/build
mkdir -p "$W"; cd "$W" || exit 1
( cd "$B" && find lib -name '*.o' | LC_ALL=C sort | head -n "$N" | sed "s|^|$B/|" ) > objs.txt
echo "objects: $(wc -l < objs.txt); bytes: $(xargs cat < objs.txt | wc -c)"
for L in bfd lld mold; do
  case $L in bfd) LD=ld.bfd;; lld) LD=ld.lld-22;; mold) LD=mold;; esac
  for k in 1 2 3; do
    /usr/bin/time -f "$L run$k: %e s, %M KB" -o t.$L.$k $LD -r -o r.$L.o @objs.txt 2> err.$L
    rc=$?; cat t.$L.$k; [ $rc = 0 ] || { echo "$L rc=$rc"; head -3 err.$L; }
  done
  python3 - r.$L.o <<'PY'
import sys; sys.path.insert(0,"/Volumes/AgentWork/mortar-helper/.build/target-wp-154/exp/tools")
from relf import *
e=load(sys.argv[1]); import collections
c=collections.Counter()
for s in e.secs:
    k={SHT_SYMTAB:"symtab",SHT_STRTAB:"strtab",SHT_RELA:"rela",SHT_GROUP:"group"}.get(s.type,"other")
    c[k]+=s.size; c[k+"#"]+=1
print("  %s: file %d B, sections %d, symbols %d; " % (sys.argv[1], len(e.data), len(e.secs), len(e.syms)) + ", ".join("%s %d"%(k,v) for k,v in sorted(c.items())))
PY
done
echo "bench done"
