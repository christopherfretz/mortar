#!/bin/sh
# build.sh: kiln++ with the hod back end (-O0: hod is tried first; -v shows whether hod or
# llc wrote the object) over the TUs here, x86-64 and AArch64, -fPIE / -fno-pic / -fPIC.
# Writes <tu>.<arch><model>.o and backend.txt (which back end wrote each).  Prints BUILD-DONE.
ulimit -c 0
cd /Volumes/AgentWork/mortar-helper/.build/target-wp-110/exp3/hod
K=/Volumes/AgentWork/mortar-helper/.build/target-kiln/release/kiln++
H=/Volumes/AgentWork/mortar-helper/.build/target-hod/release/hod
: > backend.txt
for s in *.c *.cc; do for t in x86_64 aarch64; do for m in -fPIE -fno-pic -fPIC; do
  o=${s%.*}.$t$m.o
  be=$(timeout 120 $K -x c++ --target=$t-linux-gnu -fkiln-backend=hod --hod=$H -O0 $m -c $s -o $o -v 2>&1 | grep -oE '/hod"|/llc"' | tr -d '/"' | head -1)
  [ -f $o ] && echo "$o ${be:-?}" >> backend.txt || echo "$o FAILED" >> backend.txt
done; done; done
echo BUILD-DONE
