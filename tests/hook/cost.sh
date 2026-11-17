#!/bin/bash
# WP-077: the cost tables of spec/odr.md O-54 for prototype 3, the hod -O0
# runtime beside GCC -O2's object of the same source.
# usage: cost.sh WORK TARGET   (needs MORTAR and XTASK, as run.sh)
# In-process time: the sum of every entry's MASON_ODR_TIME (clock_gettime
# around the check), three runs; wall time per start with MASON_ODR=off and
# =report, five runs each. Under qemu the numbers are emulation, not costs.
set -u
W=${1:?WORK}; T=${2:?TARGET}
H=$(cd "$(dirname "$0")" && pwd)
: "${MORTAR:?}" "${XTASK:?}"
mkdir -p "$W"; W=$(cd "$W" && pwd)
RT_FLAVOR=hod
. "$H/lib.sh"
build_rt || exit 1
wall() { # wall DIR: median-free mean of 5 starts, off and report
  local s o on k
  s=$(date +%s%N); for k in 1 2 3 4 5; do (cd "$1" && xrun MASON_ODR=off -- ./many); done; o=$(( ($(date +%s%N) - s) / 5000 ))
  s=$(date +%s%N); for k in 1 2 3 4 5; do (cd "$1" && xrun MASON_ODR=report -- ./many 2>/dev/null); done; on=$(( ($(date +%s%N) - s) / 5000 ))
  echo "    wall per start: off ${o} us, report ${on} us"
}
for fl in hod gcc; do
  RT=$W/rt-$fl.o; RT_FLAVOR=$fl
  echo "== $fl: load $(cut -d' ' -f1-3 /proc/loadavg)"
  for p in 1000 10000 100000; do
    many 4 $p good $fl-4-$p || exit 1
    timeit "$W/many-$fl-4-$p" "COST $T $fl 4 x $((p + p / 4)) clean"; wall "$W/many-$fl-4-$p"
  done
  many 300 1000 good $fl-300 || exit 1
  timeit "$W/many-$fl-300" "COST $T $fl 300 x 1250 clean"; wall "$W/many-$fl-300"
  many 300 1000 bad $fl-300bad || exit 1
  timeit "$W/many-$fl-300bad" "COST $T $fl 300 x 1250 one violating key"
  W0=$W; W=$W/quad-$fl; mkdir -p "$W"; cp "$W0/note-report.o" "$W/"
  quad 300 10 || exit 1
  $CC -O2 -c "$H/host.c" -o "$W/host.o"
  mlink $CC "$W/host.o" "$W0/rtexe-$fl.o" "$W0/note-fatal.o" -Wl,-z,now -o "$W/mhostf" -ldl
  L=(); for i in $(seq 1 300); do L+=("o:$W/q/p$i.so:"); done
  for k in 1 2 3; do
    out=$(cd "$W" && xrun MASON_ODR_TIME=1 -- ./mhostf "${L[@]}" 2>&1)
    echo "$out" | awk -v l="COST $T $fl quadratic 300 plugins K=10" '/mason-time/{s+=$2; n++} /odr-violation-at-load/{v++} END{printf "    %s: entries=%d summed check time=%.2f ms reports=%d\n", l, n, s/1e6, v}'
  done
  W=$W0
done
echo "== end: load $(cut -d' ' -f1-3 /proc/loadavg)"
